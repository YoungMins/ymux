//! ECDSA P-256 keys and the pairing proof. Pure.
//!
//! Wire forms (PROTOCOL.md section 1): a public key is the raw uncompressed
//! SEC1 point (65 bytes), a signature is IEEE P1363 `r || s` (64 bytes), both
//! base64url without padding. Verification accepts low-S and high-S, since
//! WebCrypto produces either.

use std::fmt;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hkdf::Hkdf;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::DecodePrivateKey;
use p256::{PublicKey, SecretKey};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;

use super::proto::{link_info, pair_proof_msg};
use crate::webhooks::hmac::hmac_sha256;

pub fn b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn b64u_decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.as_bytes()).ok()
}

/// `n` bytes from the operating system's CSPRNG.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    OsRng.fill_bytes(&mut v);
    v
}

pub fn random_b64u(n: usize) -> String {
    b64u(&random_bytes(n))
}

/// A uniform value in `[0, 1)`, for backoff jitter.
pub fn random_unit() -> f64 {
    let mut b = [0u8; 4];
    OsRng.fill_bytes(&mut b);
    f64::from(u32::from_le_bytes(b)) / (f64::from(u32::MAX) + 1.0)
}

/// Equal in time that does not depend on where the inputs first differ
/// (only on their lengths).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    d == 0
}

/// The desktop's signing key. Never printed.
pub struct Keypair {
    sk: SigningKey,
}

impl fmt::Debug for Keypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Keypair(<redacted>)")
    }
}

impl Keypair {
    pub fn generate() -> Self {
        Self {
            sk: SigningKey::random(&mut OsRng),
        }
    }

    /// Accepts a raw 32-byte scalar or a PKCS#8 DER document, base64url.
    pub fn from_b64u(s: &str) -> Option<Self> {
        let bytes = b64u_decode(s.trim())?;
        let sk = if bytes.len() == 32 {
            SigningKey::from_slice(&bytes).ok()?
        } else {
            SigningKey::from_pkcs8_der(&bytes).ok()?
        };
        Some(Self { sk })
    }

    /// The raw scalar, base64url: what goes into the secrets store.
    pub fn secret_b64u(&self) -> String {
        b64u(&self.sk.to_bytes())
    }

    pub fn public_b64u(&self) -> String {
        b64u(self.sk.verifying_key().to_encoded_point(false).as_bytes())
    }

    /// P1363 signature, base64url.
    pub fn sign(&self, msg: &[u8]) -> String {
        let sig: Signature = self.sk.sign(msg);
        b64u(&sig.to_bytes())
    }
}

fn parse_pubkey(pubkey_b64u: &str) -> Option<VerifyingKey> {
    let bytes = b64u_decode(pubkey_b64u)?;
    if bytes.len() != 65 || bytes[0] != 4 {
        return None;
    }
    VerifyingKey::from_sec1_bytes(&bytes).ok()
}

/// Is this a well-formed uncompressed P-256 point (on the curve)?
pub fn valid_pubkey(pubkey_b64u: &str) -> bool {
    parse_pubkey(pubkey_b64u).is_some()
}

/// Verify a P1363 signature over `msg`. Anything malformed is `false`.
pub fn verify(pubkey_b64u: &str, msg: &[u8], sig_b64u: &str) -> bool {
    let Some(vk) = parse_pubkey(pubkey_b64u) else {
        return false;
    };
    let Some(raw) = b64u_decode(sig_b64u) else {
        return false;
    };
    if raw.len() != 64 {
        return false;
    }
    let Ok(sig) = Signature::from_slice(&raw) else {
        return false;
    };
    // A high-S signature is as valid as its low-S twin; fold it first.
    let sig = sig.normalize_s().unwrap_or(sig);
    vk.verify(msg, &sig).is_ok()
}

/// HMAC-SHA256 pairing proof, base64url.
pub fn pair_proof(
    code: &[u8],
    desktop_id: &str,
    device_id: &str,
    pubkey: &str,
    ek: &str,
) -> String {
    b64u(&hmac_sha256(
        code,
        pair_proof_msg(desktop_id, device_id, pubkey, ek).as_bytes(),
    ))
}

/// An ECDH P-256 key (PROTOCOL.md section 1): `dk` is the desktop's, made
/// fresh for each pairing window and held in memory only; a device's `ek` is
/// the same shape. The private scalar is zeroised when this is dropped and is
/// never printed.
pub struct EcdhKey {
    sk: SecretKey,
}

impl fmt::Debug for EcdhKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EcdhKey(<redacted>)")
    }
}

/// The raw uncompressed SEC1 point (65 bytes, leading 0x04) of a key-agreement
/// public key; anything else, or a point off the curve, is `None`.
fn parse_ecdh_pubkey(b64u_key: &str) -> Option<PublicKey> {
    let bytes = b64u_decode(b64u_key)?;
    if bytes.len() != 65 || bytes[0] != 4 {
        return None;
    }
    PublicKey::from_sec1_bytes(&bytes).ok()
}

/// Is this a well-formed key-agreement public key (on the curve)?
pub fn valid_ecdh_pubkey(b64u_key: &str) -> bool {
    parse_ecdh_pubkey(b64u_key).is_some()
}

impl EcdhKey {
    pub fn generate() -> Self {
        Self {
            sk: SecretKey::random(&mut OsRng),
        }
    }

    /// A key from a known 32-byte scalar, to reproduce a vector.
    #[cfg(test)]
    pub fn from_scalar_for_test(scalar: &[u8; 32]) -> Self {
        Self {
            sk: SecretKey::from_slice(scalar).expect("a valid scalar"),
        }
    }

    pub fn public_b64u(&self) -> String {
        b64u(self.sk.public_key().to_encoded_point(false).as_bytes())
    }

    /// The x coordinate of the shared point; `None` for a bad peer key.
    pub(super) fn shared_x(&self, peer_b64u: &str) -> Option<[u8; 32]> {
        let peer = parse_ecdh_pubkey(peer_b64u)?;
        let shared = p256::ecdh::diffie_hellman(self.sk.to_nonzero_scalar(), peer.as_affine());
        let mut out = [0u8; 32];
        out.copy_from_slice(shared.raw_secret_bytes());
        Some(out)
    }
}

/// The strings the link key is bound to (all as they travelled).
pub struct LinkParams<'a> {
    pub desktop_id: &'a str,
    pub device_id: &'a str,
    /// The device's signing key.
    pub pubkey: &'a str,
    /// The device's key-agreement key.
    pub ek: &'a str,
    /// This window's key-agreement key.
    pub dk: &'a str,
}

/// The per-device link key (PROTOCOL.md section 1): AES-256-GCM key shared
/// by this desktop and one device only. Never printed, never in config.
#[derive(Clone)]
pub struct LinkKey([u8; 32]);

impl fmt::Debug for LinkKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinkKey(<redacted>)")
    }
}

impl PartialEq for LinkKey {
    fn eq(&self, other: &Self) -> bool {
        ct_eq(&self.0, &other.0)
    }
}

impl Eq for LinkKey {}

impl LinkKey {
    /// `HKDF-SHA256(ikm = x of ECDH(own, peer), salt = code, info = "ymux-
    /// remote-v1|link|<desktop>|<device>|<pubkey>|<ek>|<dk>")`. `own` is `dk`
    /// on the desktop and `ek` on the device; `peer` is the other's public
    /// key. `None` when `peer` is not a valid point.
    pub fn derive(own: &EcdhKey, peer: &str, code: &[u8], p: &LinkParams<'_>) -> Option<Self> {
        let ikm = own.shared_x(peer)?;
        let info = link_info(p.desktop_id, p.device_id, p.pubkey, p.ek, p.dk);
        let mut okm = [0u8; 32];
        Hkdf::<Sha256>::new(Some(code), &ikm)
            .expand(info.as_bytes(), &mut okm)
            .ok()?;
        Some(Self(okm))
    }

    /// From the secrets-store spelling (base64url of the 32 bytes).
    pub fn from_b64u(s: &str) -> Option<Self> {
        let bytes: [u8; 32] = b64u_decode(s.trim())?.try_into().ok()?;
        Some(Self(bytes))
    }

    /// What goes into the secrets store.
    pub fn to_b64u(&self) -> String {
        b64u(&self.0)
    }

    /// `b64u(nonce || ciphertext || tag)` with a fresh random 96-bit nonce.
    pub fn seal(&self, aad: &str, plain: &[u8]) -> Option<String> {
        self.seal_with(aad, plain, random_bytes(12))
    }

    /// The nonce is the caller's: only [`LinkKey::seal`] (fresh, random) and
    /// the known-answer tests may call this. A nonce used twice under one key
    /// breaks AES-GCM.
    fn seal_with(&self, aad: &str, plain: &[u8], nonce: Vec<u8>) -> Option<String> {
        let cipher = Aes256Gcm::new_from_slice(&self.0).ok()?;
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: aad.as_bytes(),
                },
            )
            .ok()?;
        let mut out = nonce;
        out.extend_from_slice(&ct);
        Some(b64u(&out))
    }

    /// The plaintext, or `None` for anything that does not open under this
    /// key and `aad` (wrong key, wrong additional data, tampering, garbage).
    pub fn open(&self, aad: &str, sealed: &str) -> Option<Vec<u8>> {
        let raw = b64u_decode(sealed)?;
        if raw.len() < 12 + 16 {
            return None;
        }
        let (nonce, ct) = raw.split_at(12);
        let cipher = Aes256Gcm::new_from_slice(&self.0).ok()?;
        cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: aad.as_bytes(),
                },
            )
            .ok()
    }

    #[cfg(test)]
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    /// Seal with a recorded nonce, to reproduce a known-answer vector.
    #[cfg(test)]
    pub fn seal_with_nonce(&self, aad: &str, plain: &[u8], nonce: &[u8]) -> Option<String> {
        self.seal_with(aad, plain, nonce.to_vec())
    }
}

/// Draws at or above this are thrown away: it is the largest multiple of
/// 10^6 that fits a `u32` (4294 * 10^6), so `draw % 10^6` of what is kept is
/// uniform over 000000..=999999 with no modulo bias.
pub const CHECK_DRAW_LIMIT: u32 = 4_294_000_000;
const _: () = assert!(
    CHECK_DRAW_LIMIT % 1_000_000 == 0 && u32::MAX - CHECK_DRAW_LIMIT < 1_000_000,
    "the limit must be the largest multiple of 10^6 in a u32"
);

/// The six digits the phone shows and the user types (section 4). Random from
/// the OS source, unrelated to anything the phone or the relay sends. It is
/// sealed to the phone and compared with what the user typed; it is never put
/// in an event, an IPC answer, a log or an audit line, and its `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct Check(String);

impl fmt::Debug for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Check(<redacted>)")
    }
}

impl Check {
    pub fn random() -> Self {
        Self::from_source(|| OsRng.next_u32())
    }

    /// Rejection sampling over a source of `u32`s (the OS CSPRNG in
    /// [`Check::random`]).
    pub fn from_source(mut next: impl FnMut() -> u32) -> Self {
        loop {
            let v = next();
            if v < CHECK_DRAW_LIMIT {
                return Self(format!("{:06}", v % 1_000_000));
            }
        }
    }

    /// For sealing into the `pair_challenge` body, nothing else.
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    /// Does what the user typed equal it? Constant time; anything that is not
    /// byte-equal (wrong length, letters, spaces) is simply a mismatch.
    pub fn matches(&self, typed: &str) -> bool {
        ct_eq(self.0.as_bytes(), typed.as_bytes())
    }
}

/// Constant-time check of a presented proof.
pub fn verify_pair_proof(
    code: &[u8],
    desktop_id: &str,
    device_id: &str,
    pubkey: &str,
    ek: &str,
    proof_b64u: &str,
) -> bool {
    let Some(got) = b64u_decode(proof_b64u) else {
        return false;
    };
    let want = hmac_sha256(
        code,
        pair_proof_msg(desktop_id, device_id, pubkey, ek).as_bytes(),
    );
    ct_eq(&got, &want)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::Signature;

    #[test]
    fn sign_and_verify_round_trip() {
        let k = Keypair::generate();
        let pk = k.public_b64u();
        assert_eq!(b64u_decode(&pk).unwrap().len(), 65);
        let sig = k.sign(b"hello");
        assert_eq!(b64u_decode(&sig).unwrap().len(), 64);
        assert!(verify(&pk, b"hello", &sig));
        assert!(!verify(&pk, b"hellO", &sig));
        let other = Keypair::generate();
        assert!(!verify(&other.public_b64u(), b"hello", &sig));
    }

    #[test]
    fn high_s_signatures_verify_too() {
        let k = Keypair::generate();
        let pk = k.public_b64u();
        let raw = b64u_decode(&k.sign(b"msg")).unwrap();
        let sig = Signature::from_slice(&raw).unwrap();
        // The signer may emit either form: take the low-S one, then build
        // its high-S twin (r, n - s).
        let low = sig.normalize_s().unwrap_or(sig);
        assert!(low.normalize_s().is_none(), "must be low-S");
        let high = Signature::from_scalars(low.r().to_bytes(), (-*low.s()).to_bytes()).unwrap();
        assert!(
            high.normalize_s().is_some(),
            "the twin must really be high-S"
        );
        assert_ne!(high.to_bytes(), low.to_bytes());
        assert!(verify(&pk, b"msg", &b64u(&high.to_bytes())));
        assert!(verify(&pk, b"msg", &b64u(&low.to_bytes())));
    }

    #[test]
    fn malformed_inputs_never_verify() {
        let k = Keypair::generate();
        let pk = k.public_b64u();
        let sig = k.sign(b"m");
        assert!(!verify("", b"m", &sig));
        assert!(!verify(&pk, b"m", ""));
        assert!(!verify(&pk, b"m", "!!!"));
        assert!(!verify(&pk, b"m", &b64u(&[0u8; 64]))); // r = s = 0
        assert!(!verify(&pk, b"m", &b64u(&[1u8; 63])));
        assert!(!verify(&b64u(&[4u8; 65]), b"m", &sig)); // not on the curve
                                                         // A compressed point is not the wire form.
        let compressed = k.sk.verifying_key().to_encoded_point(true);
        assert!(!verify(&b64u(compressed.as_bytes()), b"m", &sig));
        assert!(!valid_pubkey(&b64u(compressed.as_bytes())));
        assert!(valid_pubkey(&pk));
    }

    #[test]
    fn secret_survives_a_round_trip_and_debug_hides_it() {
        let k = Keypair::generate();
        let again = Keypair::from_b64u(&k.secret_b64u()).unwrap();
        assert_eq!(k.public_b64u(), again.public_b64u());
        assert!(!format!("{k:?}").contains(&k.secret_b64u()));
        assert!(Keypair::from_b64u("").is_none());
        assert!(Keypair::from_b64u("AAAA").is_none());
        assert!(Keypair::from_b64u(&b64u(&[0u8; 32])).is_none()); // zero scalar
    }

    #[test]
    fn pkcs8_secret_is_accepted() {
        use p256::pkcs8::EncodePrivateKey;
        let k = Keypair::generate();
        let secret = p256::SecretKey::from_slice(&k.sk.to_bytes()).unwrap();
        let der = secret.to_pkcs8_der().unwrap();
        let loaded = Keypair::from_b64u(&b64u(der.as_bytes())).unwrap();
        assert_eq!(loaded.public_b64u(), k.public_b64u());
    }

    #[test]
    fn pair_proof_good_and_bad() {
        let code = random_bytes(16);
        let proof = pair_proof(&code, "desk", "dev", "pk", "ek");
        assert!(verify_pair_proof(&code, "desk", "dev", "pk", "ek", &proof));
        // Every field the proof covers matters, the new `ek` included.
        assert!(!verify_pair_proof(
            &code, "desk", "dev", "pk2", "ek", &proof
        ));
        assert!(!verify_pair_proof(
            &code, "desk", "dev", "pk", "ek2", &proof
        ));
        assert!(!verify_pair_proof(
            &code, "desk2", "dev", "pk", "ek", &proof
        ));
        assert!(!verify_pair_proof(
            &code, "desk", "dev2", "pk", "ek", &proof
        ));
        assert!(!verify_pair_proof(
            &random_bytes(16),
            "desk",
            "dev",
            "pk",
            "ek",
            &proof
        ));
        assert!(!verify_pair_proof(&code, "desk", "dev", "pk", "ek", ""));
        assert!(!verify_pair_proof(&code, "desk", "dev", "pk", "ek", "%%%"));
        // The documented message.
        let want = hmac_sha256(&code, b"ymux-remote-v1|pair|desk|dev|pk|ek");
        assert_eq!(proof, b64u(&want));
    }

    fn key(b: u8) -> LinkKey {
        LinkKey::from_bytes([b; 32])
    }

    #[test]
    fn seal_and_open_round_trip() {
        let k = key(1);
        let sealed = k.seal("aad", b"{\"k\":\"open\"}").unwrap();
        // nonce (12) + ciphertext (12) + tag (16), base64url
        assert_eq!(b64u_decode(&sealed).unwrap().len(), 12 + 12 + 16);
        assert_eq!(k.open("aad", &sealed).unwrap(), b"{\"k\":\"open\"}");
        // Empty and large plaintexts too.
        let empty = k.seal("a", b"").unwrap();
        assert_eq!(k.open("a", &empty).unwrap(), b"");
        let big = vec![0xABu8; 60_000];
        assert_eq!(k.open("a", &k.seal("a", &big).unwrap()).unwrap(), big);
    }

    #[test]
    fn a_fresh_nonce_every_time() {
        let k = key(1);
        let a = k.seal("aad", b"same").unwrap();
        let b = k.seal("aad", b"same").unwrap();
        assert_ne!(a, b);
        assert_ne!(
            b64u_decode(&a).unwrap()[..12],
            b64u_decode(&b).unwrap()[..12]
        );
    }

    #[test]
    fn wrong_key_aad_or_tampering_does_not_open() {
        let k = key(1);
        let sealed = k.seal("ymux-remote-v1|p2d|d|e", b"secret").unwrap();
        assert!(key(2).open("ymux-remote-v1|p2d|d|e", &sealed).is_none());
        assert!(k.open("ymux-remote-v1|d2p|d|e", &sealed).is_none());
        assert!(k.open("ymux-remote-v1|p2d|d|x", &sealed).is_none());
        // Every single byte flipped (nonce, ciphertext, tag) is refused.
        let raw = b64u_decode(&sealed).unwrap();
        for i in 0..raw.len() {
            let mut bad = raw.clone();
            bad[i] ^= 1;
            assert!(
                k.open("ymux-remote-v1|p2d|d|e", &b64u(&bad)).is_none(),
                "byte {i}"
            );
        }
        // Truncated, empty, not base64.
        assert!(k.open("a", &b64u(&raw[..raw.len() - 1])).is_none());
        assert!(k.open("a", &b64u(&raw[..27])).is_none());
        assert!(k.open("a", "").is_none());
        assert!(k.open("a", "!!!").is_none());
    }

    fn params<'a>(ek: &'a str, dk: &'a str) -> LinkParams<'a> {
        LinkParams {
            desktop_id: "desk",
            device_id: "dev",
            pubkey: "pk",
            ek,
            dk,
        }
    }

    /// Fixed scalars so the vector is reproducible: 0x11.. for the desktop's
    /// `dk`, 0x22.. for the device's `ek`.
    fn fixed_pair() -> (EcdhKey, EcdhKey) {
        (
            EcdhKey::from_scalar_for_test(&[0x11; 32]),
            EcdhKey::from_scalar_for_test(&[0x22; 32]),
        )
    }

    #[test]
    fn link_key_is_hkdf_over_the_ecdh_secret_and_both_sides_agree() {
        let (dk, ek) = fixed_pair();
        let (dk_pub, ek_pub) = (dk.public_b64u(), ek.public_b64u());
        let code = [5u8; 16];
        let desktop = LinkKey::derive(&dk, &ek_pub, &code, &params(&ek_pub, &dk_pub)).unwrap();
        // The phone derives it from its private half and the QR's `dk`.
        let phone = LinkKey::derive(&ek, &dk_pub, &code, &params(&ek_pub, &dk_pub)).unwrap();
        assert_eq!(desktop.to_b64u(), phone.to_b64u());
        // Cross-check the HKDF assembly by hand (one 32-byte block):
        // PRK = HMAC(salt = code, ikm = x), T1 = HMAC(PRK, info || 0x01).
        let x = dk.shared_x(&ek_pub).unwrap();
        assert_eq!(x, ek.shared_x(&dk_pub).unwrap());
        let prk = hmac_sha256(&code, &x);
        let mut msg = format!("ymux-remote-v1|link|desk|dev|pk|{ek_pub}|{dk_pub}").into_bytes();
        msg.push(1);
        assert_eq!(desktop.to_b64u(), b64u(&hmac_sha256(&prk, &msg)));
    }

    #[test]
    fn every_input_of_the_link_key_matters() {
        let (dk, ek) = fixed_pair();
        let (dk_pub, ek_pub) = (dk.public_b64u(), ek.public_b64u());
        let code = [5u8; 16];
        let base = LinkKey::derive(&dk, &ek_pub, &code, &params(&ek_pub, &dk_pub)).unwrap();
        // A different code (what a photo of the QR plus the relay's view lacks
        // is the private half, not the code: but the salt still counts).
        let other_code = [6u8; 16];
        assert_ne!(
            base,
            LinkKey::derive(&dk, &ek_pub, &other_code, &params(&ek_pub, &dk_pub)).unwrap()
        );
        // A third party that knows the code and both public keys but not a
        // private half derives something else.
        let eve = EcdhKey::generate();
        assert_ne!(
            base,
            LinkKey::derive(&eve, &ek_pub, &code, &params(&ek_pub, &dk_pub)).unwrap()
        );
        assert_ne!(
            base,
            LinkKey::derive(&eve, &dk_pub, &code, &params(&ek_pub, &dk_pub)).unwrap()
        );
        // The strings in `info`.
        for p in [
            LinkParams {
                desktop_id: "desk2",
                ..params(&ek_pub, &dk_pub)
            },
            LinkParams {
                device_id: "dev2",
                ..params(&ek_pub, &dk_pub)
            },
            LinkParams {
                pubkey: "pk2",
                ..params(&ek_pub, &dk_pub)
            },
            params("ek2", &dk_pub),
            params(&ek_pub, "dk2"),
        ] {
            assert_ne!(base, LinkKey::derive(&dk, &ek_pub, &code, &p).unwrap());
        }
    }

    #[test]
    fn an_ek_that_is_not_a_point_is_refused() {
        let (dk, ek) = fixed_pair();
        let dk_pub = dk.public_b64u();
        let good = ek.public_b64u();
        assert!(valid_ecdh_pubkey(&good));
        let mut off_curve = b64u_decode(&good).unwrap();
        off_curve[64] ^= 1;
        let compressed = b64u(ek.sk.public_key().to_encoded_point(true).as_bytes());
        let mut wrong_tag = b64u_decode(&good).unwrap();
        wrong_tag[0] = 2;
        for bad in [
            String::new(),
            "AAAA".to_string(),
            "!!!".to_string(),
            b64u(&[4u8; 65]),
            b64u(&[0u8; 65]),
            b64u(&off_curve),
            b64u(&wrong_tag),
            b64u(&b64u_decode(&good).unwrap()[..64]),
            compressed,
        ] {
            assert!(!valid_ecdh_pubkey(&bad), "{bad}");
            assert!(
                LinkKey::derive(&dk, &bad, &[1u8; 16], &params(&bad, &dk_pub)).is_none(),
                "{bad}"
            );
        }
    }

    #[test]
    fn ecdh_keys_hide_in_debug_and_differ_per_generation() {
        let a = EcdhKey::generate();
        let b = EcdhKey::generate();
        assert_ne!(a.public_b64u(), b.public_b64u());
        assert_eq!(format!("{a:?}"), "EcdhKey(<redacted>)");
        assert_eq!(b64u_decode(&a.public_b64u()).unwrap().len(), 65);
    }

    #[test]
    fn link_key_survives_the_secret_store_spelling_and_hides_in_debug() {
        let k = key(3);
        let again = LinkKey::from_b64u(&k.to_b64u()).unwrap();
        assert_eq!(k.to_b64u(), again.to_b64u());
        assert!(!format!("{k:?}").contains(&k.to_b64u()));
        assert_eq!(format!("{k:?}"), "LinkKey(<redacted>)");
        assert!(LinkKey::from_b64u("").is_none());
        assert!(LinkKey::from_b64u("AAAA").is_none());
        assert!(LinkKey::from_b64u(&b64u(&[1u8; 31])).is_none());
        assert!(LinkKey::from_b64u(&b64u(&[1u8; 33])).is_none());
    }

    #[test]
    fn the_draw_limit_is_the_largest_multiple_of_a_million_in_a_u32() {
        // (Checked at compile time too, next to the constant.)
        assert_eq!(CHECK_DRAW_LIMIT % 1_000_000, 0);
        assert_eq!(u32::MAX / 1_000_000 * 1_000_000, CHECK_DRAW_LIMIT);
        // So every residue below 10^6 has exactly the same number of draws.
        assert_eq!(CHECK_DRAW_LIMIT / 1_000_000, 4294);
    }

    #[test]
    fn check_number_is_six_digits_and_rejection_sampled() {
        // Draws at or above the limit are thrown away, not folded in.
        let mut feed = vec![u32::MAX, CHECK_DRAW_LIMIT, CHECK_DRAW_LIMIT - 1].into_iter();
        let c = Check::from_source(|| feed.next().unwrap());
        assert_eq!(c.as_str(), "999999");
        assert_eq!(feed.next(), None, "it stopped at the first kept draw");
        // Boundaries and leading zeros.
        assert_eq!(Check::from_source(|| 0).as_str(), "000000");
        assert_eq!(Check::from_source(|| 42).as_str(), "000042");
        assert_eq!(Check::from_source(|| 1_000_000).as_str(), "000000");
        assert_eq!(Check::from_source(|| 123_456_789).as_str(), "456789");
        // Real draws are always six digits.
        for _ in 0..500 {
            let c = Check::random();
            assert_eq!(c.as_str().len(), 6);
            assert!(c.as_str().bytes().all(|b| b.is_ascii_digit()));
        }
        // Nothing derives it from a request: two fresh ones differ (a
        // collision is 1 in 10^6 per pair, so 20 distinct values is certain).
        let many: std::collections::HashSet<String> = (0..20)
            .map(|_| Check::random().as_str().to_string())
            .collect();
        assert!(many.len() > 10);
    }

    #[test]
    fn check_compares_exactly_and_hides_in_debug() {
        let c = Check::from_source(|| 123_456);
        assert!(c.matches("123456"));
        for bad in [
            "",
            "12345",
            "1234567",
            "123457",
            " 123456",
            "123456 ",
            "12345a",
            "０１２３４５",
        ] {
            assert!(!c.matches(bad), "{bad:?}");
        }
        assert_eq!(format!("{c:?}"), "Check(<redacted>)");
    }

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn jitter_is_a_unit_fraction() {
        for _ in 0..200 {
            let j = random_unit();
            assert!((0.0..1.0).contains(&j));
        }
    }
}
