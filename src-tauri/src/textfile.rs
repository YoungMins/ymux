//! Reading and writing a text file without quietly rewriting it.
//!
//! Pure: bytes in, bytes out, no filesystem. `fsops.rs` does the IO.
//!
//! ## What this exists to prevent
//!
//! The retired `ycode` TUI was UTF-8 only, split on `str::lines()` — which folds CRLF
//! away and cannot tell a final newline from its absence — and dropped the
//! trailing newline on save. On a
//! Windows-first app that is a file-corrupting bug set, not a rough edge:
//! opening a CRLF file and pressing save rewrites every line of it. The GUI
//! editor must not inherit any of it, so the rules are here, with tests,
//! before any pane exists to call them:
//!
//!  - the dominant line ending is detected on read and restored on write;
//!  - a UTF-8 BOM is stripped, recorded, and put back;
//!  - a trailing newline is preserved, and so is its absence;
//!  - non-UTF-8 content is refused, never lossy-decoded — a lossy read
//!    followed by a save destroys the file, and legacy CP949/EUC-KR support
//!    is out of scope for exactly that reason (spec §2.3);
//!  - a [`ContentStamp`] lets a write refuse when the file changed under it.
//!
//! The editor holds text with `\n` endings throughout — one representation
//! in the buffer, the file's own on disk.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{YmuxError, YmuxResult};

/// Largest file the editor will open for editing, and the largest it will
/// write.
///
/// Over it a read still succeeds but comes back `truncated` and read-only:
/// neither editor handles a 200 MB file well, and refusing loudly beats
/// hanging (spec §1.2).
pub const MAX_EDIT_BYTES: usize = 8 * 1024 * 1024;

/// The UTF-8 byte-order mark. Common on Windows, and meaningful: stripping
/// it on read and not restoring it changes the file.
pub const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// The line ending a file uses.
///
/// `Cr` is not in the spec's list but is in the type: without it a
/// classic-Mac CR-only file would be normalized to LF and written back with
/// every line changed — the same class of silent rewrite this module exists
/// to stop. It costs one variant and one match arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Eol {
    /// `\n`.
    Lf,
    /// `\r\n`.
    Crlf,
    /// A lone `\r`.
    Cr,
    /// More than one kind. Written back as LF, once, after the GUI warns —
    /// the only case where a save deliberately changes endings.
    Mixed,
    /// No line ending anywhere (a single line, with or without content).
    None,
}

impl Eol {
    /// The bytes this ending writes. `Mixed` collapses to LF.
    fn as_str(self) -> &'static str {
        match self {
            Eol::Crlf => "\r\n",
            Eol::Cr => "\r",
            // `None` never appears in the text, so what it maps to is moot.
            Eol::Lf | Eol::Mixed | Eol::None => "\n",
        }
    }
}

/// Identity of a file's contents at a moment in time.
///
/// Both halves are carried, but a conditional write compares the **hash
/// only** (see `fsops::imp::write_text`): a formatter or a `touch` that
/// rewrites identical bytes moves the mtime and is not a conflict, because
/// overwriting those bytes loses nothing. The mtime is kept because the
/// editor's focus-based staleness poll wants a cheap "might have changed?"
/// before it reads the file to hash it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentStamp {
    /// Unix-epoch milliseconds, or 0 if unknown.
    pub modified_ms: u64,
    /// Lowercase hex SHA-256 of the file's bytes *as they are on disk*,
    /// including any BOM and the file's own line endings.
    pub sha256: String,
}

/// A text file as the editor pane receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextFile {
    /// Contents with `\n` endings and no BOM.
    pub text: String,
    pub eol: Eol,
    /// The file began with a UTF-8 BOM.
    pub bom: bool,
    pub stamp: ContentStamp,
    /// The file is over [`MAX_EDIT_BYTES`] and `text` is only its head. The
    /// pane opens read-only, so this file's `stamp` is never used to guard a
    /// write.
    pub truncated: bool,
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Stamp `bytes` as they sit on disk.
pub fn stamp_of(bytes: &[u8], modified_ms: u64) -> ContentStamp {
    ContentStamp {
        modified_ms,
        sha256: sha256_hex(bytes),
    }
}

/// Which line ending dominates `s`.
///
/// Counts CRLF first so its `\r` and `\n` are not also counted as a lone CR
/// and a lone LF. More than one kind present is [`Eol::Mixed`] regardless of
/// which is commonest — the GUI needs to warn, and "mostly CRLF" is still a
/// file a save would rewrite.
pub fn detect_eol(s: &str) -> Eol {
    let crlf = s.matches("\r\n").count();
    // Remove the CRLF pairs before counting the singletons.
    let rest = s.replace("\r\n", "");
    let lf = rest.matches('\n').count();
    let cr = rest.matches('\r').count();

    match (crlf > 0, lf > 0, cr > 0) {
        (false, false, false) => Eol::None,
        (true, false, false) => Eol::Crlf,
        (false, true, false) => Eol::Lf,
        (false, false, true) => Eol::Cr,
        _ => Eol::Mixed,
    }
}

/// Fold every line ending in `s` to `\n`.
///
/// CRLF first, then a lone CR, so no `\r` survives into the buffer. That
/// totality is what makes [`restore_eol`] exact: a round trip through
/// `normalize_to_lf` + `restore_eol` reproduces the input byte for byte for
/// any file with one consistent ending.
pub fn normalize_to_lf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Re-apply `eol` to text that holds `\n` endings.
pub fn restore_eol(s: &str, eol: Eol) -> String {
    match eol {
        Eol::Lf | Eol::Mixed | Eol::None => s.to_string(),
        _ => s.replace('\n', eol.as_str()),
    }
}

/// Turn the bytes of a file into a [`TextFile`].
///
/// `total_len` is the file's real length, which may exceed `bytes.len()`
/// when the caller stopped reading at the cap.
///
/// Returns [`YmuxError::NotUtf8`] rather than lossy-decoding. When the read
/// was capped, an incomplete UTF-8 sequence at the very end is dropped — the
/// cap cut a character in half, which is not the file's fault and must not
/// be reported as invalid (the same rule the retired `ydir` TUI's preview applied).
pub fn decode(bytes: &[u8], modified_ms: u64, total_len: u64, path: &str) -> YmuxResult<TextFile> {
    let stamp = stamp_of(bytes, modified_ms);
    let truncated = total_len > bytes.len() as u64 || bytes.len() > MAX_EDIT_BYTES;

    let bom = bytes.starts_with(&UTF8_BOM);
    let body = if bom { &bytes[UTF8_BOM.len()..] } else { bytes };

    let decoded = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(e) if truncated && e.error_len().is_none() => {
            // `error_len() == None` means "unexpected end of input": the
            // cap landed mid-character. Everything before it is good.
            std::str::from_utf8(&body[..e.valid_up_to()])
                .map_err(|_| YmuxError::NotUtf8(path.to_string()))?
        }
        Err(_) => return Err(YmuxError::NotUtf8(path.to_string())),
    };

    Ok(TextFile {
        eol: detect_eol(decoded),
        text: normalize_to_lf(decoded),
        bom,
        stamp,
        truncated,
    })
}

/// Turn editor text back into the bytes to write.
///
/// `text` is expected to hold `\n` endings; anything else in it is
/// normalized first so a paste carrying CRLF cannot smuggle mixed endings
/// into a file.
pub fn encode(text: &str, eol: Eol, bom: bool, path: &str) -> YmuxResult<Vec<u8>> {
    let body = restore_eol(&normalize_to_lf(text), eol);
    let len = body.len() + if bom { UTF8_BOM.len() } else { 0 };
    if len > MAX_EDIT_BYTES {
        return Err(YmuxError::TooLarge(format!(
            "{path}: {len} bytes exceeds the {MAX_EDIT_BYTES}-byte edit limit"
        )));
    }
    let mut out = Vec::with_capacity(len);
    if bom {
        out.extend_from_slice(&UTF8_BOM);
    }
    out.extend_from_slice(body.as_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_every_ending() {
        assert_eq!(detect_eol("a\nb\nc"), Eol::Lf);
        assert_eq!(detect_eol("a\r\nb\r\nc"), Eol::Crlf);
        assert_eq!(detect_eol("a\rb\rc"), Eol::Cr);
        assert_eq!(detect_eol("a\r\nb\nc"), Eol::Mixed);
        assert_eq!(detect_eol("a\nb\rc"), Eol::Mixed);
        assert_eq!(detect_eol("one line"), Eol::None);
        assert_eq!(detect_eol(""), Eol::None);
    }

    /// The bug ycode had: a CRLF file must come back out as a CRLF file,
    /// byte for byte, so saving an untouched file is a no-op diff.
    #[test]
    fn crlf_round_trips_byte_for_byte() {
        for raw in [
            "a\r\nb\r\nc\r\n",
            "a\r\nb\r\nc",
            "\r\n",
            "안녕\r\n하세요\r\n",
        ] {
            let f = decode(raw.as_bytes(), 1, raw.len() as u64, "t").unwrap();
            assert_eq!(f.eol, Eol::Crlf, "{raw:?}");
            let out = encode(&f.text, f.eol, f.bom, "t").unwrap();
            assert_eq!(out, raw.as_bytes(), "{raw:?}");
        }
    }

    #[test]
    fn lf_and_cr_only_files_round_trip_too() {
        for (raw, want) in [
            ("a\nb\n", Eol::Lf),
            ("a\rb\r", Eol::Cr),
            ("solo", Eol::None),
        ] {
            let f = decode(raw.as_bytes(), 1, raw.len() as u64, "t").unwrap();
            assert_eq!(f.eol, want, "{raw:?}");
            assert_eq!(encode(&f.text, f.eol, f.bom, "t").unwrap(), raw.as_bytes());
        }
    }

    /// The other half of ycode's bug: `str::lines()` cannot tell these two
    /// files apart, and saving either produced the same bytes.
    #[test]
    fn trailing_newline_is_preserved_and_so_is_its_absence() {
        let with = decode(b"a\nb\n", 1, 4, "t").unwrap();
        assert_eq!(with.text, "a\nb\n");
        assert_eq!(
            encode(&with.text, with.eol, with.bom, "t").unwrap(),
            b"a\nb\n"
        );

        let without = decode(b"a\nb", 1, 3, "t").unwrap();
        assert_eq!(without.text, "a\nb");
        assert_eq!(
            encode(&without.text, without.eol, without.bom, "t").unwrap(),
            b"a\nb"
        );
    }

    #[test]
    fn bom_is_stripped_recorded_and_restored() {
        let mut raw = UTF8_BOM.to_vec();
        raw.extend_from_slice("안녕\r\n하세요\r\n".as_bytes());

        let f = decode(&raw, 1, raw.len() as u64, "t").unwrap();
        assert!(f.bom);
        assert!(
            !f.text.starts_with('\u{feff}'),
            "the BOM must not leak into the buffer"
        );
        assert_eq!(f.text, "안녕\n하세요\n");
        assert_eq!(f.eol, Eol::Crlf);
        assert_eq!(encode(&f.text, f.eol, f.bom, "t").unwrap(), raw);

        // A file without one does not grow one.
        let plain = decode(b"x\n", 1, 2, "t").unwrap();
        assert!(!plain.bom);
        assert_eq!(
            encode(&plain.text, plain.eol, plain.bom, "t").unwrap(),
            b"x\n"
        );
    }

    /// Mixed endings are the one case a save deliberately rewrites, and the
    /// spec sanctions it (LF, once, after a GUI warning).
    #[test]
    fn mixed_endings_report_mixed_and_write_lf() {
        let f = decode(b"a\r\nb\nc\r\n", 1, 8, "t").unwrap();
        assert_eq!(f.eol, Eol::Mixed);
        assert_eq!(f.text, "a\nb\nc\n");
        assert_eq!(encode(&f.text, f.eol, f.bom, "t").unwrap(), b"a\nb\nc\n");
    }

    #[test]
    fn invalid_utf8_is_refused_not_lossy_decoded() {
        // 0x80 is a bare continuation byte; a CP949 file looks like this.
        let err = decode(&[b'a', 0x80, b'b'], 1, 3, "cp949.txt").unwrap_err();
        assert_eq!(err.kind(), "not_utf8");
        assert!(err.to_string().contains("cp949.txt"));
    }

    /// A capped read that cut a Hangul syllable in half is not an encoding
    /// error; the fragment is dropped.
    #[test]
    fn a_capped_read_drops_an_incomplete_trailing_sequence() {
        let full = "안녕하세요";
        let bytes = full.as_bytes();
        let cut = &bytes[..bytes.len() - 1];
        assert!(std::str::from_utf8(cut).is_err());

        // Claiming the file is longer than what was read marks it truncated.
        let f = decode(cut, 1, bytes.len() as u64, "t").unwrap();
        assert!(f.truncated);
        assert_eq!(f.text, "안녕하세");

        // The same bytes *not* declared truncated are a real error - this is
        // what stops a genuinely corrupt file being silently shortened.
        let err = decode(cut, 1, cut.len() as u64, "t").unwrap_err();
        assert_eq!(err.kind(), "not_utf8");
    }

    #[test]
    fn over_the_cap_reads_truncated_and_refuses_to_write() {
        let big = "x".repeat(MAX_EDIT_BYTES + 10);
        // Read: head only, truncated, and the pane opens read-only.
        let f = decode(&big.as_bytes()[..1024], 1, big.len() as u64, "big.txt").unwrap();
        assert!(f.truncated);

        // Write: refused outright rather than half-written.
        let err = encode(&big, Eol::Lf, false, "big.txt").unwrap_err();
        assert_eq!(err.kind(), "too_large");
        assert!(err.to_string().contains("big.txt"));

        // Exactly at the cap is allowed; the BOM counts toward it.
        let at_cap = "y".repeat(MAX_EDIT_BYTES);
        assert!(encode(&at_cap, Eol::Lf, false, "t").is_ok());
        assert_eq!(
            encode(&at_cap, Eol::Lf, true, "t").unwrap_err().kind(),
            "too_large"
        );
    }

    #[test]
    fn stamp_follows_the_content_and_the_mtime() {
        let a = stamp_of(b"hello", 1000);
        let same = stamp_of(b"hello", 1000);
        assert_eq!(a, same);

        // Content changed.
        assert_ne!(a.sha256, stamp_of(b"hello!", 1000).sha256);
        // Touched but not changed: the hash holds, so the editor can tell a
        // harmless rewrite from a real conflict.
        assert_eq!(a.sha256, stamp_of(b"hello", 2000).sha256);
        assert_ne!(a, stamp_of(b"hello", 2000));
    }

    #[test]
    fn sha256_matches_the_known_vector_for_the_empty_input() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// The stamp is of the bytes on disk, BOM and CRLF included - otherwise
    /// a stamp taken by the backend would never match one taken by anything
    /// else looking at the same file.
    #[test]
    fn stamp_is_over_the_on_disk_bytes_not_the_buffer() {
        let mut raw = UTF8_BOM.to_vec();
        raw.extend_from_slice(b"a\r\n");
        let f = decode(&raw, 7, raw.len() as u64, "t").unwrap();
        assert_eq!(f.stamp.sha256, sha256_hex(&raw));
        assert_ne!(f.stamp.sha256, sha256_hex(f.text.as_bytes()));
        assert_eq!(f.stamp.modified_ms, 7);
    }

    /// Text arriving from a paste with CRLF in it must not smuggle mixed
    /// endings into an LF file.
    #[test]
    fn encode_normalizes_whatever_the_buffer_hands_it() {
        assert_eq!(encode("a\r\nb", Eol::Lf, false, "t").unwrap(), b"a\nb");
        assert_eq!(encode("a\r\nb", Eol::Crlf, false, "t").unwrap(), b"a\r\nb");
    }
}
