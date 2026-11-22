//! Pictures in a transcript, for the `image` op (PROTOCOL.md section 5.2).
//!
//! A picture is a base64 string inside a transcript line. Listing one in a
//! chat item (a descriptor: id, type, size) looks only at the first few bytes
//! of it; fetching one decodes the whole string, checks it again and cuts it
//! into parts of [`PART_BYTES`]. The type is ALWAYS the one the file signature
//! says (PNG, JPEG, GIF, WebP), never the declared media type, and anything
//! else (SVG, unknown, bad base64, over [`IMAGE_MAX`]) is not a picture here.
//!
//! Pure `std` + `base64` + `serde_json` values: nothing here may panic.

use std::sync::Arc;

use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use serde_json::{json, Value};

use super::json::J;
use crate::remote::keys::b64u;

/// A picture larger than this (decoded) is not listed and cannot be fetched.
pub const IMAGE_MAX: u64 = 8 * 1024 * 1024;
/// Bytes of the file in one reply part (96 KiB): 128 KiB of base64url, which
/// with its envelope stays under the 256 KiB frame cap.
pub const PART_BYTES: usize = 96 * 1024;
/// Most parts a reply may name (`8 MiB / 96 KiB` is 86; the protocol allows
/// `part` up to 255).
pub const PARTS_MAX: u64 = 256;
/// Most pictures one item lists.
pub const PER_ITEM: usize = 8;
/// A picture's id is `<line offset>.<ID_BASE + ordinal>`. Items of a line are
/// numbered below 40 (0..20 with text, 20..40 pictures only), so these never
/// meet.
pub const ID_BASE: u32 = 100;
/// Most pictures one line can list: 40 items of 8 pictures.
pub const LINE_MAX: u32 = 320;

/// Standard alphabet; padding optional, trailing bits tolerated (the decoder
/// is a reader, not a validator).
const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// A picture found in a line, still base64 and still borrowed from the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pic<'t> {
    pub b64: &'t str,
    pub kind: &'static str,
    /// Decoded size, worked out from the length of the base64.
    pub size: u64,
}

/// The type the first bytes of a file say it is, among the four allowed.
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The decoded length of `b64`, from its length and padding alone.
pub fn decoded_len(b64: &str) -> Option<u64> {
    let b = b64.as_bytes();
    let len = b.len() as u64;
    let pad = b.iter().rev().take(2).take_while(|c| **c == b'=').count() as u64;
    if pad > 0 {
        return (len % 4 == 0).then(|| len / 4 * 3 - pad);
    }
    match len % 4 {
        0 => Some(len / 4 * 3),
        2 => Some(len / 4 * 3 + 1),
        3 => Some(len / 4 * 3 + 2),
        _ => None,
    }
}

/// `b64` as a listable picture: a valid-looking start, one of the four
/// signatures, a size from 1 byte to [`IMAGE_MAX`]. Only the first 24
/// characters are decoded.
pub fn from_b64(b64: &str) -> Option<Pic<'_>> {
    let size = decoded_len(b64)?;
    if size == 0 || size > IMAGE_MAX {
        return None;
    }
    let head = b64.as_bytes().get(..b64.len().min(24))?;
    let head = LENIENT.decode(head).ok()?;
    Some(Pic {
        b64,
        kind: sniff(&head)?,
        size,
    })
}

/// `data:image/png;base64,<b64>`: the declared type is ignored, but `base64`
/// must be there and the comma must come early.
pub fn from_data_url(url: &str) -> Option<Pic<'_>> {
    let rest = url.strip_prefix("data:")?;
    let comma = rest.as_bytes().iter().take(128).position(|c| *c == b',')?;
    let header = rest.get(..comma)?;
    if !header
        .split(';')
        .any(|p| p.trim().eq_ignore_ascii_case("base64"))
    {
        return None;
    }
    from_b64(rest.get(comma + 1..)?)
}

fn block_type<'t>(b: &'t J<'_>) -> Option<&'t str> {
    b.get("type").and_then(J::as_str)
}

/// The pictures among Claude Code content blocks:
/// `{"type":"image","source":{"type":"base64","media_type":…,"data":…}}`.
pub fn claude_pics<'t>(blocks: &'t [J<'_>]) -> Vec<Pic<'t>> {
    blocks
        .iter()
        .filter(|b| block_type(b) == Some("image"))
        .filter_map(|b| {
            let src = b.get("source")?;
            if src.get("type")?.as_str()? != "base64" {
                return None;
            }
            from_b64(src.get("data")?.as_str()?)
        })
        .take(PER_ITEM)
        .collect()
}

/// The pictures among Codex content items: `input_image` (or `output_image`)
/// with `image_url` a data URL, or an object whose `url` is one.
pub fn codex_pics<'t>(blocks: &'t [J<'_>]) -> Vec<Pic<'t>> {
    blocks
        .iter()
        .filter(|b| matches!(block_type(b), Some("input_image" | "output_image")))
        .filter_map(|b| {
            let u = b.get("image_url")?;
            from_data_url(u.as_str().or_else(|| u.get("url")?.as_str())?)
        })
        .take(PER_ITEM)
        .collect()
}

/// The whole picture, decoded and checked again: its length is the one the
/// listing said and its signature is the type the listing said.
pub fn decode(p: &Pic<'_>) -> Option<Vec<u8>> {
    let v = LENIENT.decode(p.b64).ok()?;
    (v.len() as u64 == p.size && sniff(&v) == Some(p.kind)).then_some(v)
}

/// Number of parts a picture of `size` bytes comes in.
pub fn parts(size: u64) -> u64 {
    size.div_ceil(PART_BYTES as u64)
}

/// The reply data for part `part` of a decoded picture, or `None` when the
/// picture is empty, too big, or `part` is out of range.
pub fn part_reply(
    conv: &str,
    id: &str,
    kind: &'static str,
    data: &[u8],
    part: u32,
) -> Option<Value> {
    let size = data.len() as u64;
    let n = parts(size);
    if size == 0 || size > IMAGE_MAX || n > PARTS_MAX || u64::from(part) >= n {
        return None;
    }
    let a = (part as usize).checked_mul(PART_BYTES)?;
    let b = a.saturating_add(PART_BYTES).min(data.len());
    let slice = data.get(a..b)?;
    Some(json!({
        "conv": conv, "id": id, "type": kind, "size": size,
        "part": part, "parts": n, "data": b64u(slice),
    }))
}

/// `<offset>.<100 + ordinal>` -> `(offset, ordinal)`; anything else is `None`.
pub fn parse_id(id: &str) -> Option<(u64, u32)> {
    let (off, n) = id.split_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.len() <= 20 && s.bytes().all(|c| c.is_ascii_digit());
    if !digits(off) || !digits(n) {
        return None;
    }
    let ord = n.parse::<u32>().ok()?.checked_sub(ID_BASE)?;
    (ord < LINE_MAX).then_some((off.parse().ok()?, ord))
}

pub fn make_id(offset: u64, ordinal: u32) -> String {
    format!("{offset}.{}", ID_BASE + ordinal)
}

// ------------------------------------------------------------------- cache

/// Most bytes the decoded pictures kept may hold together. There is no limit
/// on their number: what fits is kept, least recently used out first.
pub const CACHE_BYTES: usize = 32 * 1024 * 1024;

/// What the file looked like when a picture was decoded from it: its length and
/// the length of the picture's line. A cached picture is served only while the
/// file still looks like that at the picture's offset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stamp {
    pub file_len: u64,
    pub line_len: u64,
}

struct Entry {
    pane: String,
    conv: String,
    id: String,
    kind: &'static str,
    data: Arc<Vec<u8>>,
    stamp: Stamp,
}

/// The last few decoded pictures, so that fetching a picture in parts reads and
/// decodes its line once.
#[derive(Default)]
pub struct PicCache {
    entries: Vec<Entry>,
}

impl PicCache {
    pub fn get(
        &mut self,
        pane: &str,
        conv: &str,
        id: &str,
    ) -> Option<(&'static str, Arc<Vec<u8>>)> {
        self.get_stamped(pane, conv, id).map(|(k, d, _)| (k, d))
    }

    pub fn get_stamped(
        &mut self,
        pane: &str,
        conv: &str,
        id: &str,
    ) -> Option<(&'static str, Arc<Vec<u8>>, Stamp)> {
        let i = self
            .entries
            .iter()
            .position(|e| e.pane == pane && e.conv == conv && e.id == id)?;
        // Most recently used goes last.
        let e = self.entries.remove(i);
        let out = (e.kind, e.data.clone(), e.stamp);
        self.entries.push(e);
        Some(out)
    }

    pub fn remove(&mut self, pane: &str, conv: &str, id: &str) {
        self.entries
            .retain(|e| !(e.pane == pane && e.conv == conv && e.id == id));
    }

    pub fn put(
        &mut self,
        pane: &str,
        conv: &str,
        id: &str,
        kind: &'static str,
        data: Arc<Vec<u8>>,
    ) {
        self.put_stamped(pane, conv, id, kind, data, Stamp::default());
    }

    pub fn put_stamped(
        &mut self,
        pane: &str,
        conv: &str,
        id: &str,
        kind: &'static str,
        data: Arc<Vec<u8>>,
        stamp: Stamp,
    ) {
        self.remove(pane, conv, id);
        self.entries.push(Entry {
            pane: pane.to_string(),
            conv: conv.to_string(),
            id: id.to_string(),
            kind,
            data,
            stamp,
        });
        // The newest (just put) stays even if it alone were over the cap.
        while self.entries.len() > 1 && self.bytes() > CACHE_BYTES {
            self.entries.remove(0);
        }
    }

    /// The pane is on another conversation now: its pictures are not served.
    pub fn forget_other_conv(&mut self, pane: &str, conv: &str) {
        self.entries.retain(|e| e.pane != pane || e.conv == conv);
    }

    /// The pane has no agent (or no pane): nothing is kept for it.
    pub fn forget_pane(&mut self, pane: &str) {
        self.entries.retain(|e| e.pane != pane);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn bytes(&self) -> usize {
        self.entries.iter().map(|e| e.data.len()).sum()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::keys::b64u_decode;
    use base64::engine::general_purpose::STANDARD;

    pub(crate) const PNG8: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    fn b64(bytes: &[u8]) -> String {
        STANDARD.encode(bytes)
    }

    fn with_sig(sig: &[u8], total: usize) -> Vec<u8> {
        let mut v = sig.to_vec();
        v.resize(total.max(sig.len()), 7);
        v
    }

    #[test]
    fn the_four_signatures_and_nothing_else() {
        assert_eq!(sniff(&with_sig(&PNG8, 20)), Some("image/png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF87a\x01\x00"), Some("image/gif"));
        assert_eq!(sniff(b"GIF89a\x01\x00"), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\x10\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        assert_eq!(
            sniff(b"RIFF\x10\x00\x00\x00WAVEfmt "),
            None,
            "RIFF but not WebP"
        );
        assert_eq!(sniff(b"RIFF\x10\x00\x00\x00WEB"), None, "too short");
        assert_eq!(sniff(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None);
        assert_eq!(sniff(b"GIF88a"), None);
        assert_eq!(sniff(&PNG8[..7]), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn the_decoded_length_comes_from_the_text_alone() {
        for n in 0..70usize {
            let data = vec![1u8; n];
            let padded = b64(&data);
            assert_eq!(decoded_len(&padded), Some(n as u64), "{n} padded");
            let bare = padded.trim_end_matches('=');
            assert_eq!(decoded_len(bare), Some(n as u64), "{n} unpadded");
        }
        assert_eq!(decoded_len("A"), None, "one stray character");
        assert_eq!(decoded_len("AAAAA"), None);
        assert_eq!(decoded_len("AAA=="), None, "padding that does not fit");
        assert_eq!(decoded_len("AA="), None);
        assert_eq!(decoded_len(""), Some(0));
    }

    #[test]
    fn a_listable_picture_needs_a_signature_and_a_sane_size() {
        let png = b64(&with_sig(&PNG8, 100));
        let p = from_b64(&png).unwrap();
        assert_eq!((p.kind, p.size), ("image/png", 100));
        // The declared type is nowhere in the input: a JPEG body stays a JPEG.
        let jpg = b64(&with_sig(&[0xFF, 0xD8, 0xFF, 0xE0], 50));
        assert_eq!(from_b64(&jpg).unwrap().kind, "image/jpeg");
        // Short bodies still work (no 24-character head to cut).
        assert_eq!(from_b64(&b64(&with_sig(&PNG8, 8))).unwrap().size, 8);
        assert!(from_b64("").is_none());
        assert!(from_b64("!!!!").is_none(), "not base64");
        assert!(from_b64(&b64(b"hello world, this is not a picture")).is_none());
        assert!(
            from_b64(&b64(b"<svg/>")).is_none(),
            "SVG is never a picture"
        );
        // A bad start is refused even when the rest is long and fine.
        let mut bad = String::from("$$$$");
        bad.push_str(&png);
        assert!(from_b64(&bad).is_none());
        // Size limits: exactly 8 MiB is in, one byte more is out.
        let exact = b64(&with_sig(&PNG8, IMAGE_MAX as usize));
        assert_eq!(from_b64(&exact).unwrap().size, IMAGE_MAX);
        let over = b64(&with_sig(&PNG8, IMAGE_MAX as usize + 1));
        assert!(from_b64(&over).is_none());
        // Padding in the middle of the head.
        assert!(from_b64("iVBORw0K=GgoAAAAA").is_none());
    }

    #[test]
    fn data_urls_need_base64_and_an_early_comma() {
        let png = b64(&with_sig(&PNG8, 64));
        let good = format!("data:image/png;base64,{png}");
        assert_eq!(from_data_url(&good).unwrap().kind, "image/png");
        // The declared type may be anything, even wrong.
        let lie = format!("data:image/jpeg;base64,{png}");
        assert_eq!(from_data_url(&lie).unwrap().kind, "image/png");
        assert!(
            from_data_url(&format!("data:image/png,{png}")).is_none(),
            "not base64"
        );
        assert!(
            from_data_url(&format!("data:image/png;base64{png}")).is_none(),
            "no comma"
        );
        assert!(from_data_url("data:").is_none());
        assert!(from_data_url("https://example.test/a.png").is_none());
        assert!(from_data_url(&format!("data:{},{png}", "x".repeat(200))).is_none());
        assert!(from_data_url(&format!("data:image/png;BASE64,{png}")).is_some());
        assert!(from_data_url("").is_none());
        // Multi-byte text before the comma cannot cut a character.
        assert!(from_data_url("data:ééééé").is_none());
    }

    #[test]
    fn decode_checks_length_and_signature_again() {
        let data = with_sig(&PNG8, 5000);
        let text = b64(&data);
        let p = from_b64(&text).unwrap();
        assert_eq!(decode(&p).unwrap(), data);
        // A listing that lied about the size, or the type, decodes to nothing.
        assert!(decode(&Pic { size: 4999, ..p }).is_none());
        assert!(decode(&Pic {
            kind: "image/gif",
            ..p
        })
        .is_none());
        // Garbage after a valid head: listed (only the head is looked at),
        // refused by the full decode.
        let mut broken = text.clone();
        broken.replace_range(100..104, "!!!!");
        let p = from_b64(&broken).unwrap();
        assert!(decode(&p).is_none());
        // Whitespace inside is not base64 here.
        let mut spaced = text.clone();
        spaced.replace_range(200..201, "\n");
        let p = from_b64(&spaced).unwrap();
        assert!(decode(&p).is_none());
    }

    #[test]
    fn parts_are_96_kib_and_the_last_is_the_remainder() {
        assert_eq!(PART_BYTES, 98_304);
        assert_eq!(parts(0), 0);
        assert_eq!(parts(1), 1);
        assert_eq!(parts(98_304), 1);
        assert_eq!(parts(98_305), 2);
        assert_eq!(parts(IMAGE_MAX), 86);
        let data: Vec<u8> = (0..250_000u32).map(|i| (i % 251) as u8).collect();
        let mut joined = Vec::new();
        for k in 0..3 {
            let r = part_reply("cv", "10.100", "image/png", &data, k).unwrap();
            assert_eq!(
                (r["size"].as_u64(), r["part"].as_u64(), r["parts"].as_u64()),
                (Some(250_000), Some(u64::from(k)), Some(3))
            );
            let bytes = b64u_decode(r["data"].as_str().unwrap()).unwrap();
            assert_eq!(
                bytes.len(),
                if k < 2 {
                    PART_BYTES
                } else {
                    250_000 - 2 * PART_BYTES
                }
            );
            joined.extend(bytes);
        }
        assert_eq!(joined, data);
        assert!(part_reply("cv", "10.100", "image/png", &data, 3).is_none());
        assert!(part_reply("cv", "10.100", "image/png", &data, u32::MAX).is_none());
        assert!(
            part_reply("cv", "10.100", "image/png", &[], 0).is_none(),
            "zero bytes"
        );
        let huge = vec![0u8; IMAGE_MAX as usize + 1];
        assert!(part_reply("cv", "10.100", "image/png", &huge, 0).is_none());
    }

    #[test]
    fn ids_live_in_their_own_number_space() {
        assert_eq!(make_id(1100, 0), "1100.100");
        assert_eq!(parse_id("1100.100"), Some((1100, 0)));
        assert_eq!(parse_id("0.419"), Some((0, 319)));
        for bad in [
            "",
            "1100",
            "1100.",
            ".100",
            "1100.99",
            "1100.420",
            "1100.0",
            "1100.1.2",
            "a.100",
            "1100.-100",
            "1100.100 ",
            "99999999999999999999999.100",
            "1100.4294967396",
        ] {
            assert_eq!(parse_id(bad), None, "{bad:?}");
        }
        // Chat item ids are `<offset>.<0..40>`: never a picture id.
        for i in 0..40 {
            assert_eq!(parse_id(&format!("5.{i}")), None);
        }
    }

    #[test]
    fn the_cache_is_an_lru_bounded_by_bytes_only() {
        let mut c = PicCache::default();
        let pic = |n: usize| Arc::new(vec![0u8; n]);
        // Many small ones all stay (no count limit).
        for i in 0..10 {
            c.put("p", "c", &format!("{i}.100"), "image/png", pic(10));
        }
        assert_eq!((c.len(), c.bytes()), (10, 100));
        // The same key again replaces, never doubles.
        c.put("p", "c", "3.100", "image/png", pic(20));
        assert_eq!((c.len(), c.bytes()), (10, 110));
        c.clear();
        // Three interleaved 8 MiB pictures fit together (24 MiB): fetching
        // their parts round-robin never misses after the first read.
        let big = IMAGE_MAX as usize;
        for id in ["1.100", "2.100", "3.100"] {
            c.put("p", "c", id, "image/png", pic(big));
        }
        for _ in 0..3 {
            for id in ["1.100", "2.100", "3.100"] {
                assert!(c.get("p", "c", id).is_some(), "{id}");
            }
        }
        // A fifth 8 MiB one pushes the total over 32 MiB: the least recently
        // used one goes, not the newest.
        c.put("p", "c", "4.100", "image/png", pic(big));
        assert_eq!(c.len(), 4);
        c.get("p", "c", "1.100");
        c.put("p", "c", "5.100", "image/png", pic(big));
        assert_eq!(c.len(), 4);
        assert!(c.bytes() <= CACHE_BYTES);
        assert!(c.get("p", "c", "2.100").is_none(), "least recently used");
        assert!(c.get("p", "c", "1.100").is_some() && c.get("p", "c", "5.100").is_some());
        // Another conversation of the pane drops its pictures only.
        c.clear();
        c.put("p", "old", "1.100", "image/png", pic(5));
        c.put("q", "old", "1.100", "image/png", pic(5));
        c.forget_other_conv("p", "new");
        assert!(c.get("p", "old", "1.100").is_none());
        assert!(c.get("q", "old", "1.100").is_some());
        c.put("p", "old", "1.100", "image/png", pic(5));
        c.forget_pane("q");
        assert!(c.get("q", "old", "1.100").is_none());
        assert!(c.get("p", "old", "1.100").is_some());
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn blocks_of_both_agents_give_pictures_and_nothing_else() {
        let png = b64(&with_sig(&PNG8, 40));
        let jpg = b64(&with_sig(&[0xFF, 0xD8, 0xFF], 40));
        let text = format!(
            r#"[{{"type":"text","text":"hi"}},
               {{"type":"image","source":{{"type":"base64","media_type":"image/jpeg","data":"{png}"}}}},
               {{"type":"image","source":{{"type":"url","url":"https://example.test/a.png"}}}},
               {{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{jpg}"}}}},
               {{"type":"image","source":{{"type":"base64","data":"AAAA"}}}},
               {{"type":"image"}}]"#
        );
        let v = crate::remote::chat::json::parse(text.as_bytes()).unwrap();
        let kinds: Vec<&str> = claude_pics(v.as_array().unwrap())
            .iter()
            .map(|p| p.kind)
            .collect();
        assert_eq!(
            kinds,
            ["image/png", "image/jpeg"],
            "signature, not the declared type"
        );
        let cx = format!(
            r#"[{{"type":"input_text","text":"x"}},
               {{"type":"input_image","image_url":"data:image/png;base64,{png}"}},
               {{"type":"input_image","image_url":{{"url":"data:image/jpeg;base64,{jpg}"}}}},
               {{"type":"input_image","image_url":"https://example.test/a.png"}},
               {{"type":"input_image","image_url":7}},
               {{"type":"input_image"}}]"#
        );
        let v = crate::remote::chat::json::parse(cx.as_bytes()).unwrap();
        assert_eq!(codex_pics(v.as_array().unwrap()).len(), 2);
        // At most eight per item, however many there are.
        let many = format!(
            "[{}]",
            (0..300)
                .map(|_| format!(
                    r#"{{"type":"image","source":{{"type":"base64","data":"{png}"}}}}"#
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        let v = crate::remote::chat::json::parse(many.as_bytes()).unwrap();
        assert_eq!(claude_pics(v.as_array().unwrap()).len(), PER_ITEM);
    }
}
