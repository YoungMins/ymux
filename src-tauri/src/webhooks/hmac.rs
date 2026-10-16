//! HMAC-SHA256 (RFC 2104) over the `sha2` crate.

use sha2::{Digest, Sha256};

const BLOCK: usize = 64;

/// Raw HMAC-SHA256 tag.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let h = Sha256::digest(key);
        k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
    }
    s
}

/// `sha256=<hex>` over `ts + "." + body`.
pub fn sign(secret: &str, ts: &str, body: &[u8]) -> String {
    let mut msg = Vec::with_capacity(ts.len() + 1 + body.len());
    msg.extend_from_slice(ts.as_bytes());
    msg.push(b'.');
    msg.extend_from_slice(body);
    format!("sha256={}", hex(&hmac_sha256(secret.as_bytes(), &msg)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(key: &[u8], data: &[u8]) -> String {
        hex(&hmac_sha256(key, data))
    }

    #[test]
    fn rfc4231_case1() {
        assert_eq!(
            h(&[0x0b; 20], b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn rfc4231_case2() {
        assert_eq!(
            h(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn rfc4231_case3() {
        assert_eq!(
            h(&[0xaa; 20], &[0xdd; 50]),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
    }

    #[test]
    fn rfc4231_case4() {
        let key: Vec<u8> = (1u8..=25).collect();
        assert_eq!(
            h(&key, &[0xcd; 50]),
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"
        );
    }

    #[test]
    fn rfc4231_case6_long_key() {
        assert_eq!(
            h(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            ),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn sign_covers_timestamp_and_body() {
        let s = sign("k", "1700000000", b"{}");
        assert!(s.starts_with("sha256="));
        assert_eq!(s.len(), 7 + 64);
        let expect = format!("sha256={}", h(b"k", b"1700000000.{}"));
        assert_eq!(s, expect);
        assert_ne!(s, sign("k", "1700000001", b"{}"));
        assert_ne!(s, sign("k", "1700000000", b"{ }"));
    }

    #[test]
    fn empty_key_and_message_do_not_panic() {
        assert_eq!(h(b"", b"").len(), 64);
    }
}
