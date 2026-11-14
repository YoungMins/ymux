//! The pairing window: a one-time code, a fresh key-agreement key, the check
//! of a phone's proof, the link key and the check number the user types
//! (PROTOCOL.md section 4). Pure.
//!
//! The code never leaves this process except inside the QR link the user
//! scans, and the link key cannot be computed from it: it comes from an ECDH
//! between this window's `dk` (private half in memory only, dropped with the
//! window) and the phone's `ek`. A request whose HMAC proof does not verify is
//! dropped silently: the user is shown nothing, only the phone is told
//! `ok:false`. The six-digit check number is random, sealed to the phone and
//! compared with what the user typed; nothing here ever hands it back out.

use super::keys::{self, random_bytes, Check, EcdhKey, LinkKey, LinkParams};
use super::proto::is_uuid;
use crate::config::model::RemoteDevice;

/// How long a window stays open.
pub const TTL_S: u32 = 120;
/// Requests (good or bad) one window will look at; the server enforces the
/// same number, this is the desktop's own bound.
pub const MAX_REQUESTS: u8 = 5;
/// Devices one desktop may have.
pub const MAX_DEVICES: usize = 16;
/// Longest device name taken from the phone.
pub const NAME_MAX: usize = 40;
/// Wrong check numbers one request may have before it is refused.
pub const MAX_TRIES: u8 = 3;

/// What the engine needs to ask the phone for the number and the user for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub device_id: String,
    pub name: String,
    /// The link key this request would get; sealed bodies for the phone use it
    /// before anything is stored.
    pub link: LinkKey,
    /// Sealed into `pair_challenge`, nothing else.
    pub check: Check,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Answer `pair_result ok:false`, show nothing. The reason is for the
    /// audit log.
    Reject(&'static str),
    /// The proof verified and the key agreement worked: ask the user.
    Ask(Asked),
    /// A second, different device presented a valid proof for this code:
    /// someone else got hold of it. Refuse both and close the window.
    Conflict,
}

/// What the user's answer did.
#[derive(Debug, PartialEq, Eq)]
pub enum Decided {
    /// No such request is waiting.
    Unknown,
    /// The user said no.
    Rejected,
    /// The typed number was right: store the device and the link key.
    Approved {
        device_id: String,
        pubkey: String,
        name: String,
        link: LinkKey,
    },
    /// Wrong number; this many tries remain.
    Wrong { tries_left: u8 },
    /// The last wrong try: the request is gone.
    Refused,
}

#[derive(Debug)]
struct Pending {
    device_id: String,
    pubkey: String,
    name: String,
    check: Check,
    link: LinkKey,
    tries: u8,
}

pub struct Pairing {
    code: Vec<u8>,
    /// This window's key-agreement key; in memory only, gone with the window.
    dk: EcdhKey,
    expires_ms: u64,
    requests: u8,
    pending: Vec<Pending>,
    /// `(device_id, pubkey, ek)` of every request whose proof verified,
    /// answered or not.
    proven: Vec<(String, String, String)>,
}

/// The code is the secret the QR carries; it never reaches a log.
impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pairing")
            .field("code", &"<redacted>")
            .field("dk", &self.dk)
            .field("expires_ms", &self.expires_ms)
            .field("requests", &self.requests)
            .field("pending", &self.pending)
            .field("proven", &self.proven.len())
            .finish()
    }
}

impl Pairing {
    pub fn open(now_ms: u64) -> Self {
        Self {
            code: random_bytes(16),
            dk: EcdhKey::generate(),
            expires_ms: now_ms.saturating_add(u64::from(TTL_S) * 1000),
            requests: 0,
            pending: Vec::new(),
            proven: Vec::new(),
        }
    }

    pub fn code_b64u(&self) -> String {
        keys::b64u(&self.code)
    }

    /// The public half of `dk`, for the QR link.
    pub fn dk_b64u(&self) -> String {
        self.dk.public_b64u()
    }

    pub fn expires_ms(&self) -> u64 {
        self.expires_ms
    }

    pub fn expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_ms
    }

    /// Judge a `pair_request` the server forwarded.
    #[allow(clippy::too_many_arguments)]
    pub fn on_request(
        &mut self,
        now_ms: u64,
        desktop_id: &str,
        existing: &[RemoteDevice],
        device_id: &str,
        pubkey: &str,
        ek: &str,
        name: &str,
        proof: &str,
    ) -> Verdict {
        if self.expired(now_ms) {
            return Verdict::Reject("expired");
        }
        if self.requests >= MAX_REQUESTS {
            return Verdict::Reject("budget");
        }
        self.requests += 1;
        if !keys::verify_pair_proof(&self.code, desktop_id, device_id, pubkey, ek, proof) {
            return Verdict::Reject("bad_proof");
        }
        // A valid proof needs the code, and the code was in one QR: a second
        // device with one means it leaked. The same device (id, key and `ek`)
        // asking again is only a retry; a different `ek` under the same id and
        // key is someone else who read the code.
        if self
            .proven
            .iter()
            .any(|(id, pk, e)| id != device_id || pk != pubkey || e != ek)
        {
            return Verdict::Conflict;
        }
        if self.proven.is_empty() {
            self.proven
                .push((device_id.to_string(), pubkey.to_string(), ek.to_string()));
        }
        if !is_uuid(device_id) || !keys::valid_pubkey(pubkey) {
            return Verdict::Reject("bad_request");
        }
        if !keys::valid_ecdh_pubkey(ek) {
            return Verdict::Reject("bad_ek");
        }
        if let Some(d) = existing.iter().find(|d| d.id == device_id) {
            if d.pubkey != pubkey {
                // Never let a new key take over a paired device's id.
                return Verdict::Reject("id_taken");
            }
        } else if existing.len() >= MAX_DEVICES {
            return Verdict::Reject("limit");
        }
        if self.pending.iter().any(|p| p.device_id == device_id) {
            return Verdict::Reject("duplicate");
        }
        let dk_pub = self.dk.public_b64u();
        let Some(link) = LinkKey::derive(
            &self.dk,
            ek,
            &self.code,
            &LinkParams {
                desktop_id,
                device_id,
                pubkey,
                ek,
                dk: &dk_pub,
            },
        ) else {
            return Verdict::Reject("bad_ek");
        };
        let check = Check::random();
        let name = device_name(name);
        self.pending.push(Pending {
            device_id: device_id.to_string(),
            pubkey: pubkey.to_string(),
            name: name.clone(),
            check: check.clone(),
            link: link.clone(),
            tries: 0,
        });
        Verdict::Ask(Asked {
            device_id: device_id.to_string(),
            name,
            link,
            check,
        })
    }

    /// The user's answer for `device_id`. The request stays pending across
    /// wrong tries; a right number, a rejection or the last wrong try removes
    /// it. Anything that is not byte-equal to the number counts as wrong.
    pub fn decide(&mut self, device_id: &str, approve: bool, typed: &str) -> Decided {
        let Some(i) = self.pending.iter().position(|p| p.device_id == device_id) else {
            return Decided::Unknown;
        };
        if !approve {
            self.pending.remove(i);
            return Decided::Rejected;
        }
        if self.pending[i].check.matches(typed) {
            let p = self.pending.remove(i);
            return Decided::Approved {
                device_id: p.device_id,
                pubkey: p.pubkey,
                name: p.name,
                link: p.link,
            };
        }
        let p = &mut self.pending[i];
        p.tries += 1;
        if p.tries >= MAX_TRIES {
            self.pending.remove(i);
            return Decided::Refused;
        }
        Decided::Wrong {
            tries_left: MAX_TRIES - p.tries,
        }
    }

    /// Remove every waiting request and return their ids.
    pub fn drain_pending(&mut self) -> Vec<String> {
        self.pending.drain(..).map(|p| p.device_id).collect()
    }

    pub fn pending_ids(&self) -> Vec<String> {
        self.pending.iter().map(|p| p.device_id.clone()).collect()
    }

    /// The sealed-body plaintext of the number a waiting request is held to
    /// (for the engine's `pair_challenge`; `None` for an unknown device).
    #[cfg(test)]
    pub(super) fn check_for_test(&self, device_id: &str) -> Option<String> {
        self.pending
            .iter()
            .find(|p| p.device_id == device_id)
            .map(|p| p.check.as_str().to_string())
    }
}

/// What a name that cleans to nothing becomes (the page may localise it).
pub const UNNAMED_DEVICE: &str = "Unnamed device";

/// Combining marks, and the scripts whose vowel signs are marks the
/// `Alphabetic` property misses (whole blocks). A digit is never one: the
/// whole-block ranges of the Indic scripts hold their digits too, so those are
/// ruled out here and not left to the caller's order of checks.
fn is_mark_like(c: char) -> bool {
    !c.is_numeric()
        && matches!(c as u32,
            0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5 | 0x05C7 | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670
            | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED
            | 0x0900..=0x0963 | 0x0971..=0x0DFF | 0x0E00..=0x0E4E | 0x0E5C..=0x0EFF
            | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0x3099..=0x309A
            | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F)
}

/// A phone's self-chosen name. A stranger chooses it, and it is shown next to
/// the prompt for the check number, so it must not carry a number or any
/// punctuation that could frame a fake instruction: redacted, then only
/// letters, marks, spaces and `. - _ '` survive (every Unicode digit and all
/// other punctuation and symbols go), whitespace collapses, 40 chars at most,
/// never empty ([`UNNAMED_DEVICE`]).
pub fn device_name(raw: &str) -> String {
    // Redaction sees a generous head (digits it matches go with the filter
    // below anyway); the 40-character bound applies to what is kept.
    let n = super::clip::text(raw.trim(), NAME_MAX * 4);
    let kept: String = n
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some(' ')
            } else if c.is_numeric() {
                None
            } else if matches!(c, '.' | '-' | '_' | '\'') || c.is_alphabetic() || is_mark_like(c) {
                Some(c)
            } else {
                None
            }
        })
        .collect();
    let joined = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let out: String = joined.chars().take(NAME_MAX).collect();
    let out = out.trim().to_string();
    if out.is_empty() {
        UNNAMED_DEVICE.to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::keys::{pair_proof, Keypair};

    const DESK: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
    const DEV: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
    const OTHER: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

    /// A phone: its signing key and its key-agreement key.
    struct Phone {
        pk: String,
        ek: EcdhKey,
    }

    fn phone() -> Phone {
        Phone {
            pk: Keypair::generate().public_b64u(),
            ek: EcdhKey::generate(),
        }
    }

    fn ask_with(
        p: &mut Pairing,
        now: u64,
        existing: &[RemoteDevice],
        dev: &str,
        phone: &Phone,
    ) -> Verdict {
        let ek = phone.ek.public_b64u();
        let proof = pair_proof(&p.code, DESK, dev, &phone.pk, &ek);
        p.on_request(now, DESK, existing, dev, &phone.pk, &ek, "My Phone", &proof)
    }

    fn ask(p: &mut Pairing, now: u64, dev: &str, phone: &Phone) -> Verdict {
        ask_with(p, now, &[], dev, phone)
    }

    fn asked(v: Verdict) -> Asked {
        match v {
            Verdict::Ask(a) => a,
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn good_proof_asks_the_user_and_the_phone_derives_the_same_key() {
        let mut p = Pairing::open(1_000);
        let ph = phone();
        let a = asked(ask(&mut p, 2_000, DEV, &ph));
        assert_eq!(a.device_id, DEV);
        assert_eq!(a.name, "My Phone");
        // The phone, holding `ek`'s private half and the QR's `dk`, gets it too.
        let ek_pub = ph.ek.public_b64u();
        let dk_pub = p.dk_b64u();
        let phone_side = LinkKey::derive(
            &ph.ek,
            &dk_pub,
            &p.code,
            &LinkParams {
                desktop_id: DESK,
                device_id: DEV,
                pubkey: &ph.pk,
                ek: &ek_pub,
                dk: &dk_pub,
            },
        )
        .unwrap();
        assert_eq!(phone_side, a.link);
        assert_eq!(p.pending_ids(), vec![DEV.to_string()]);
    }

    #[test]
    fn the_code_and_the_public_fields_alone_do_not_give_the_key() {
        // What a photo of the QR plus the relay's view holds: the code, `dk`
        // (public), the device's `pubkey`, `ek` and every id.
        let mut p = Pairing::open(0);
        let ph = phone();
        let a = asked(ask(&mut p, 1, DEV, &ph));
        let (ek_pub, dk_pub) = (ph.ek.public_b64u(), p.dk_b64u());
        let params = LinkParams {
            desktop_id: DESK,
            device_id: DEV,
            pubkey: &ph.pk,
            ek: &ek_pub,
            dk: &dk_pub,
        };
        let eve = EcdhKey::generate();
        assert_ne!(
            LinkKey::derive(&eve, &dk_pub, &p.code, &params).unwrap(),
            a.link
        );
        assert_ne!(
            LinkKey::derive(&eve, &ek_pub, &p.code, &params).unwrap(),
            a.link
        );
    }

    #[test]
    fn a_window_has_its_own_dk() {
        let (a, b) = (Pairing::open(0), Pairing::open(0));
        assert_ne!(a.dk_b64u(), b.dk_b64u());
        assert!(keys::valid_ecdh_pubkey(&a.dk_b64u()));
        // The private half never shows in Debug, nor does the check number.
        let dbg = format!("{a:?}");
        assert!(dbg.contains("EcdhKey(<redacted>)"));
    }

    #[test]
    fn bad_proof_is_dropped() {
        let mut p = Pairing::open(0);
        let ph = phone();
        let ek = ph.ek.public_b64u();
        let wrong = pair_proof(&random_bytes(16), DESK, DEV, &ph.pk, &ek);
        assert_eq!(
            p.on_request(1, DESK, &[], DEV, &ph.pk, &ek, "x", &wrong),
            Verdict::Reject("bad_proof")
        );
        assert_eq!(
            p.on_request(1, DESK, &[], DEV, &ph.pk, &ek, "x", ""),
            Verdict::Reject("bad_proof")
        );
        // A proof made for a different key, or a different `ek`, does not
        // carry over.
        let other = Keypair::generate().public_b64u();
        let proof = pair_proof(&p.code, DESK, DEV, &other, &ek);
        assert_eq!(
            p.on_request(1, DESK, &[], DEV, &ph.pk, &ek, "x", &proof),
            Verdict::Reject("bad_proof")
        );
        let other_ek = EcdhKey::generate().public_b64u();
        let proof = pair_proof(&p.code, DESK, DEV, &ph.pk, &ek);
        assert_eq!(
            p.on_request(1, DESK, &[], DEV, &ph.pk, &other_ek, "x", &proof),
            Verdict::Reject("bad_proof")
        );
        assert!(p.pending_ids().is_empty());
    }

    #[test]
    fn an_ek_that_is_not_a_point_is_refused_even_with_a_valid_proof() {
        for bad in ["", "AAAA", "not base64!", &keys::b64u(&[4u8; 65])] {
            let mut p = Pairing::open(0);
            let ph = phone();
            let proof = pair_proof(&p.code, DESK, DEV, &ph.pk, bad);
            assert_eq!(
                p.on_request(1, DESK, &[], DEV, &ph.pk, bad, "x", &proof),
                Verdict::Reject("bad_ek"),
                "{bad}"
            );
            assert!(p.pending_ids().is_empty());
        }
    }

    #[test]
    fn expired_window_refuses() {
        let mut p = Pairing::open(0);
        assert_eq!(p.expires_ms(), 120_000);
        let ph = phone();
        assert!(matches!(ask(&mut p, 119_999, DEV, &ph), Verdict::Ask(_)));
        let mut p = Pairing::open(0);
        assert_eq!(ask(&mut p, 120_000, DEV, &ph), Verdict::Reject("expired"));
        assert!(p.expired(120_000));
        assert!(!p.expired(119_999));
    }

    #[test]
    fn request_budget_is_enforced() {
        let mut p = Pairing::open(0);
        let ph = phone();
        let ek = ph.ek.public_b64u();
        for _ in 0..MAX_REQUESTS {
            let _ = p.on_request(1, DESK, &[], DEV, &ph.pk, &ek, "x", "bad");
        }
        assert_eq!(ask(&mut p, 2, DEV, &ph), Verdict::Reject("budget"));
    }

    #[test]
    fn malformed_ids_and_keys_are_refused_even_with_a_valid_proof() {
        let ph = phone();
        let mut p = Pairing::open(0);
        assert_eq!(
            ask(&mut p, 1, "NOT-A-UUID", &ph),
            Verdict::Reject("bad_request")
        );
        let mut p = Pairing::open(0);
        let bad = Phone {
            pk: "AAAA".into(),
            ek: EcdhKey::generate(),
        };
        assert_eq!(ask(&mut p, 1, DEV, &bad), Verdict::Reject("bad_request"));
    }

    #[test]
    fn an_existing_id_cannot_be_taken_over() {
        let old = phone();
        let existing = vec![RemoteDevice {
            id: DEV.into(),
            pubkey: old.pk.clone(),
            cap: "view".into(),
            ..RemoteDevice::default()
        }];
        let new = phone();
        let mut p = Pairing::open(0);
        assert_eq!(
            ask_with(&mut p, 1, &existing, DEV, &new),
            Verdict::Reject("id_taken")
        );
        // The same key may pair again (it just re-confirms).
        let mut p = Pairing::open(0);
        assert!(matches!(
            ask_with(&mut p, 1, &existing, DEV, &old),
            Verdict::Ask(_)
        ));
    }

    #[test]
    fn device_limit_and_duplicate() {
        let mut p = Pairing::open(0);
        let ph = phone();
        let full: Vec<RemoteDevice> = (0..MAX_DEVICES)
            .map(|i| RemoteDevice {
                id: format!("00000000-0000-4000-8000-{i:012}"),
                ..RemoteDevice::default()
            })
            .collect();
        assert_eq!(
            ask_with(&mut p, 1, &full, DEV, &ph),
            Verdict::Reject("limit")
        );
        // The same device twice is a retry, not a conflict.
        let mut p = Pairing::open(0);
        assert!(matches!(ask(&mut p, 1, DEV, &ph), Verdict::Ask(_)));
        assert_eq!(ask(&mut p, 1, DEV, &ph), Verdict::Reject("duplicate"));
    }

    #[test]
    fn a_second_device_with_a_valid_proof_is_a_conflict() {
        let (a, b) = (phone(), phone());
        // A different device id.
        let mut p = Pairing::open(0);
        assert!(matches!(ask(&mut p, 1, DEV, &a), Verdict::Ask(_)));
        assert_eq!(ask(&mut p, 1, OTHER, &b), Verdict::Conflict);
        // The same id with another signing key.
        let mut p = Pairing::open(0);
        assert!(matches!(ask(&mut p, 1, DEV, &a), Verdict::Ask(_)));
        assert_eq!(ask(&mut p, 1, DEV, &b), Verdict::Conflict);
        // The same id and signing key with another `ek`: someone who read the
        // QR and replays a victim's public fields.
        let mut p = Pairing::open(0);
        assert!(matches!(ask(&mut p, 1, DEV, &a), Verdict::Ask(_)));
        let impostor = Phone {
            pk: a.pk.clone(),
            ek: EcdhKey::generate(),
        };
        assert_eq!(ask(&mut p, 1, DEV, &impostor), Verdict::Conflict);
        // Still a conflict after the first was answered.
        let mut p = Pairing::open(0);
        assert!(matches!(ask(&mut p, 1, DEV, &a), Verdict::Ask(_)));
        assert_eq!(p.decide(DEV, false, ""), Decided::Rejected);
        assert_eq!(ask(&mut p, 2, OTHER, &b), Verdict::Conflict);
    }

    #[test]
    fn a_second_request_with_an_invalid_proof_is_no_conflict() {
        let (a, b) = (phone(), phone());
        let mut p = Pairing::open(0);
        assert!(matches!(ask(&mut p, 1, DEV, &a), Verdict::Ask(_)));
        let ek = b.ek.public_b64u();
        let wrong = pair_proof(&random_bytes(16), DESK, OTHER, &b.pk, &ek);
        assert_eq!(
            p.on_request(2, DESK, &[], OTHER, &b.pk, &ek, "x", &wrong),
            Verdict::Reject("bad_proof")
        );
        // The first is still waiting for the user.
        assert_eq!(p.pending_ids(), vec![DEV.to_string()]);
    }

    #[test]
    fn the_right_number_approves_and_only_then_is_the_key_released() {
        let mut p = Pairing::open(0);
        let a = asked(ask(&mut p, 1, DEV, &phone()));
        let number = p.check_for_test(DEV).unwrap();
        assert_eq!(number.len(), 6);
        match p.decide(DEV, true, &number) {
            Decided::Approved {
                device_id, link, ..
            } => {
                assert_eq!(device_id, DEV);
                assert_eq!(link, a.link);
            }
            d => panic!("{d:?}"),
        }
        // Used up.
        assert_eq!(p.decide(DEV, true, &number), Decided::Unknown);
    }

    #[test]
    fn three_wrong_numbers_refuse_and_the_right_one_after_that_does_not_help() {
        let mut p = Pairing::open(0);
        asked(ask(&mut p, 1, DEV, &phone()));
        let number = p.check_for_test(DEV).unwrap();
        let wrong = if number == "000000" {
            "000001"
        } else {
            "000000"
        };
        assert_eq!(p.decide(DEV, true, wrong), Decided::Wrong { tries_left: 2 });
        assert_eq!(p.decide(DEV, true, "12"), Decided::Wrong { tries_left: 1 });
        // Still waiting: a wrong try does not remove it.
        assert_eq!(p.pending_ids(), vec![DEV.to_string()]);
        assert_eq!(p.decide(DEV, true, ""), Decided::Refused);
        assert!(p.pending_ids().is_empty());
        assert_eq!(p.decide(DEV, true, &number), Decided::Unknown);
    }

    #[test]
    fn a_right_number_on_the_third_try_still_approves() {
        let mut p = Pairing::open(0);
        asked(ask(&mut p, 1, DEV, &phone()));
        let number = p.check_for_test(DEV).unwrap();
        let wrong = if number == "111111" {
            "222222"
        } else {
            "111111"
        };
        assert!(matches!(p.decide(DEV, true, wrong), Decided::Wrong { .. }));
        assert!(matches!(p.decide(DEV, true, wrong), Decided::Wrong { .. }));
        assert!(matches!(
            p.decide(DEV, true, &number),
            Decided::Approved { .. }
        ));
    }

    #[test]
    fn rejecting_removes_the_request_and_unknown_ids_are_unknown() {
        let mut p = Pairing::open(0);
        asked(ask(&mut p, 1, DEV, &phone()));
        assert_eq!(p.decide(OTHER, true, "123456"), Decided::Unknown);
        assert_eq!(p.decide(DEV, false, "ignored"), Decided::Rejected);
        assert_eq!(p.decide(DEV, false, ""), Decided::Unknown);
    }

    #[test]
    fn tries_are_per_request() {
        let mut p = Pairing::open(0);
        let ph = phone();
        asked(ask(&mut p, 1, DEV, &ph));
        let n = p.check_for_test(DEV).unwrap();
        let wrong = if n == "000000" { "000001" } else { "000000" };
        p.decide(DEV, true, wrong);
        p.decide(DEV, true, wrong);
        assert_eq!(p.drain_pending(), vec![DEV.to_string()]);
        assert!(p.pending_ids().is_empty());
    }

    #[test]
    fn mark_like_is_never_a_digit_and_still_covers_vowel_signs() {
        // Devanagari, Thai, Bengali and Lao digits sit inside whole-block ranges.
        for c in [
            '\u{0966}', '\u{096F}', '\u{0E50}', '\u{0E59}', '\u{09E6}', '\u{0ED0}',
        ] {
            assert!(!is_mark_like(c), "{c:?} is a digit");
        }
        for u in 0..=0x10FFFFu32 {
            if let Some(c) = char::from_u32(u) {
                assert!(!(c.is_numeric() && is_mark_like(c)), "U+{u:04X}");
            }
        }
        // Vowel signs and combining marks stay marks.
        for c in ['\u{0301}', '\u{093F}', '\u{094D}', '\u{0E31}', '\u{064B}'] {
            assert!(is_mark_like(c), "{c:?} is a mark");
        }
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(device_name("  iPhone  "), "iPhone");
        assert_eq!(device_name(""), UNNAMED_DEVICE);
        assert_eq!(device_name("\x1b\x07"), UNNAMED_DEVICE);
        // Invisible and direction-changing characters go too.
        assert_eq!(device_name("\u{200B}iPh\u{202E}one\u{FEFF}"), "iPhone");
        assert_eq!(device_name("\u{200B}\u{2028}"), UNNAMED_DEVICE);
        // A name cannot carry a number or frame an instruction.
        assert_eq!(
            device_name("Pixel 8\u{201D} \u{2014} your number: 482913 \u{201C}"),
            "Pixel your number"
        );
        assert_eq!(
            device_name("\u{0664}\u{0665}\u{FF11}\u{2460}"),
            UNNAMED_DEVICE
        );
        assert_eq!(
            device_name("\"\u{201C}\u{300C}!?[]()<>:;,/\\"),
            UNNAMED_DEVICE
        );
        assert_eq!(device_name("Sam's phone_2.0-x"), "Sam's phone_.-x");
        assert_eq!(device_name("a \n\t b"), "a b");
        assert_eq!(device_name("\u{1F600}"), UNNAMED_DEVICE);
        let kept = "한국 폰 日本語 e\u{0301} \u{0939}\u{093F}\u{0928}\u{094D}\u{0926}\u{0940}";
        assert_eq!(device_name(kept), kept);
        assert_eq!(
            device_name(&"my phone ".repeat(60)).chars().count(),
            NAME_MAX
        );
    }

    #[test]
    fn debug_output_hides_the_code() {
        let p = Pairing::open(0);
        let shown = format!("{p:?}");
        assert!(shown.contains("<redacted>"));
        assert!(!shown.contains(&p.code_b64u()));
        assert!(!shown.contains(&format!("{:?}", p.code)));
    }

    #[test]
    fn codes_are_random_and_16_bytes() {
        let a = Pairing::open(0);
        let b = Pairing::open(0);
        assert_eq!(a.code.len(), 16);
        assert_ne!(a.code, b.code);
        assert_eq!(keys::b64u_decode(&a.code_b64u()).unwrap(), a.code);
    }
}
