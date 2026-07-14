//! Pure, `std`-only logic for persisting per-pane terminal scrollback to
//! disk. Deliberately free of any Tauri dependency so it compiles and its
//! tests run under `cargo test --no-default-features --lib -p ymux` on
//! Linux CI, unlike `commands.rs` (which is gated behind the `desktop`
//! feature). The `#[tauri::command]` wrappers in `commands.rs` just call
//! into these functions and map `std::io::Error` to `YmuxError::Io`.

use std::path::{Path, PathBuf};

/// Directory scrollback blobs are written to: `<config_dir>/ymux/scrollback`,
/// falling back to a relative directory if the OS config dir can't be
/// determined (mirrors the fallback used by `config::store`).
pub fn scrollback_dir() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join("ymux").join("scrollback"))
        .unwrap_or_else(|| PathBuf::from("./ymux-scrollback"))
}

/// Path to the scrollback file for a given pane id under an arbitrary base
/// directory. `pane_id` is expected to be a UUID string; anything that isn't
/// a hex digit or `-` is stripped so a malicious or malformed id (e.g.
/// containing `..` or path separators) cannot escape `base`.
fn scrollback_file_under(base: &Path, pane_id: &str) -> PathBuf {
    let safe: String = pane_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit() || *c == '-')
        .collect();
    base.join(format!("{safe}.txt"))
}

/// Path to the scrollback file for a given pane id under the real,
/// OS-resolved scrollback directory. See `scrollback_file_under` for the
/// sanitization rules.
pub fn scrollback_file(pane_id: &str) -> PathBuf {
    scrollback_file_under(&scrollback_dir(), pane_id)
}

/// Cap persisted scrollback at ~256 KB, keeping the tail (most recent
/// output) when the blob exceeds that size.
const SCROLLBACK_CAP_BYTES: usize = 256 * 1024;

/// Save `blob` (the serialized scrollback contents) for `pane_id` under an
/// arbitrary base directory, creating it if needed and truncating to the
/// last `SCROLLBACK_CAP_BYTES` bytes (rounded forward to the next UTF-8 char
/// boundary, so the retained tail is always valid UTF-8) if the blob is
/// larger. Writes via a temp file + rename so a crash mid-write can't leave
/// a truncated/corrupt scrollback file, mirroring `config::store::write_atomic`.
fn save_blob_under(base: &Path, pane_id: &str, blob: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(base)?;
    let out = if blob.len() > SCROLLBACK_CAP_BYTES {
        let mut start = blob.len() - SCROLLBACK_CAP_BYTES;
        while !blob.is_char_boundary(start) {
            start += 1;
        }
        &blob[start..]
    } else {
        blob
    };
    let path = scrollback_file_under(base, pane_id);
    let tmp = path.with_extension("txt.tmp");
    std::fs::write(&tmp, out.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// Save `blob` for `pane_id` under the real, OS-resolved scrollback
/// directory. See `save_blob_under` for truncation/atomicity behavior.
pub fn save_blob(pane_id: &str, blob: &str) -> std::io::Result<()> {
    save_blob_under(&scrollback_dir(), pane_id, blob)
}

/// Load the persisted scrollback for `pane_id` under an arbitrary base
/// directory. Returns an empty string (not an error) if no scrollback has
/// been saved for this pane yet.
fn load_blob_under(base: &Path, pane_id: &str) -> std::io::Result<String> {
    match std::fs::read_to_string(scrollback_file_under(base, pane_id)) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// Load the persisted scrollback for `pane_id` under the real, OS-resolved
/// scrollback directory. See `load_blob_under` for the "empty if missing"
/// contract.
pub fn load_blob(pane_id: &str) -> std::io::Result<String> {
    load_blob_under(&scrollback_dir(), pane_id)
}

/// Delete the persisted scrollback for `pane_id` under an arbitrary base
/// directory, if any. A missing file is not an error.
fn delete_blob_under(base: &Path, pane_id: &str) -> std::io::Result<()> {
    let path = scrollback_file_under(base, pane_id);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Delete the persisted scrollback for `pane_id` under the real, OS-resolved
/// scrollback directory, if any. A missing file is not an error.
pub fn delete_blob(pane_id: &str) -> std::io::Result<()> {
    delete_blob_under(&scrollback_dir(), pane_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, isolated temp directory for a single test, so fs-touching
    /// tests never read/write the real OS scrollback directory. Mirrors
    /// `config::store::tests::tempdir()`.
    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ymux-scrollback-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("mkdir");
        base
    }

    #[test]
    fn scrollback_file_sanitizes_pane_id() {
        let p = scrollback_file("../../evil");
        assert!(!p.to_string_lossy().contains(".."));
    }

    #[test]
    fn scrollback_file_keeps_valid_uuid_chars() {
        let id = "0d1e2f3a-4b5c-6d7e-8f90-123456789abc";
        let p = scrollback_file(id);
        assert_eq!(
            p.file_name().and_then(|n| n.to_str()),
            Some(format!("{id}.txt").as_str())
        );
    }

    /// Real save -> load -> delete round-trip, hermetically isolated to a
    /// temp directory via the `_under` core so this test never touches the
    /// real OS config/scrollback directory.
    #[test]
    fn save_load_delete_round_trip() {
        let base = tempdir();
        let pane_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let blob = "hello scrollback\nsecond line\n";

        save_blob_under(&base, pane_id, blob).expect("save_blob should succeed");
        let loaded = load_blob_under(&base, pane_id).expect("load_blob should succeed");
        assert_eq!(loaded, blob);

        delete_blob_under(&base, pane_id).expect("delete_blob should succeed");
        let after_delete =
            load_blob_under(&base, pane_id).expect("load after delete should succeed");
        assert_eq!(after_delete, "", "deleted scrollback should load as empty");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn load_blob_missing_pane_returns_empty() {
        let base = tempdir();
        let loaded = load_blob_under(&base, "00000000-0000-0000-0000-000000000000")
            .expect("load should not error");
        assert_eq!(loaded, "");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn save_blob_caps_to_tail() {
        let base = tempdir();
        let pane_id = "11111111-2222-3333-4444-555555555555";
        // Build a blob larger than the cap: a distinguishable head that
        // should be dropped entirely, plus a tail exactly as large as the
        // cap that should survive in full.
        let head = "H".repeat(1024);
        let tail = "T".repeat(SCROLLBACK_CAP_BYTES);
        let blob = format!("{head}{tail}");

        save_blob_under(&base, pane_id, &blob).expect("save_blob should succeed");
        let loaded = load_blob_under(&base, pane_id).expect("load_blob should succeed");
        assert_eq!(loaded.len(), SCROLLBACK_CAP_BYTES);
        assert_eq!(loaded, tail);
        assert!(
            !loaded.contains('H'),
            "head should have been truncated away"
        );

        delete_blob_under(&base, pane_id).expect("cleanup delete should succeed");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Regression test for the UTF-8-safe tail cap (Fix 1). Builds a blob out
    /// of many 3-byte UTF-8 characters (box-drawing `─`, U+2500). Since the
    /// blob length is always a multiple of 3 but `SCROLLBACK_CAP_BYTES`
    /// (262144) is not (262144 mod 3 == 1), the raw cut offset
    /// `blob.len() - SCROLLBACK_CAP_BYTES` is *never* a multiple of 3 for any
    /// number of repetitions -- i.e. it deterministically lands mid-character,
    /// not merely by chance. Against the old raw-slice logic, slicing a
    /// `&str` at a non-char-boundary index panics in Rust, so this test would
    /// fail (RED) every run against that implementation. Asserts: save+load
    /// both succeed, the loaded blob is valid UTF-8 (guaranteed by
    /// `&str`/`String` typing), its byte length is within the cap, and it is
    /// a clean suffix composed only of `─` characters (no partial/replacement
    /// characters).
    #[test]
    fn save_blob_caps_to_tail_utf8_safe() {
        let base = tempdir();
        let pane_id = "22222222-3333-4444-5555-666666666666";
        // U+2500 BOX DRAWINGS LIGHT HORIZONTAL, encodes to 3 bytes in UTF-8.
        let ch = '─';
        let ch_len = ch.len_utf8();
        assert_eq!(ch_len, 3);
        // Enough repetitions to exceed the cap; see doc comment above for why
        // the resulting cut offset is guaranteed to be mid-character.
        let total_chars = (SCROLLBACK_CAP_BYTES / ch_len) + 100;
        let blob: String = std::iter::repeat(ch).take(total_chars).collect();
        assert!(blob.len() > SCROLLBACK_CAP_BYTES);

        save_blob_under(&base, pane_id, &blob).expect("save_blob should succeed (RED if panics)");
        let loaded =
            load_blob_under(&base, pane_id).expect("load_blob should succeed and be valid UTF-8");

        assert!(
            loaded.len() <= SCROLLBACK_CAP_BYTES,
            "loaded tail must not exceed the cap"
        );
        assert!(!loaded.is_empty(), "loaded tail must not be empty");
        assert!(
            loaded.chars().all(|c| c == ch),
            "loaded tail must consist solely of intact '─' characters, no replacement/partial chars"
        );
        // The loaded content must be an actual suffix of the original blob.
        assert!(blob.ends_with(&loaded));

        delete_blob_under(&base, pane_id).expect("cleanup delete should succeed");
        let _ = std::fs::remove_dir_all(&base);
    }
}
