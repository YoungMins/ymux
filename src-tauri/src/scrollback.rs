//! Pure, `std`-only logic for persisting per-pane terminal scrollback to
//! disk. Deliberately free of any Tauri dependency so it compiles and its
//! tests run under `cargo test --no-default-features --lib -p ymux` on
//! Linux CI, unlike `commands.rs` (which is gated behind the `desktop`
//! feature). The `#[tauri::command]` wrappers in `commands.rs` just call
//! into these functions and map `std::io::Error` to `YmuxError::Io`.

use std::path::PathBuf;

/// Directory scrollback blobs are written to: `<config_dir>/ymux/scrollback`,
/// falling back to a relative directory if the OS config dir can't be
/// determined (mirrors the fallback used by `config::store`).
pub fn scrollback_dir() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join("ymux").join("scrollback"))
        .unwrap_or_else(|| PathBuf::from("./ymux-scrollback"))
}

/// Path to the scrollback file for a given pane id. `pane_id` is expected to
/// be a UUID string; anything that isn't a hex digit or `-` is stripped so a
/// malicious or malformed id (e.g. containing `..` or path separators)
/// cannot escape `scrollback_dir()`.
pub fn scrollback_file(pane_id: &str) -> PathBuf {
    let safe: String = pane_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit() || *c == '-')
        .collect();
    scrollback_dir().join(format!("{safe}.txt"))
}

/// Cap persisted scrollback at ~256 KB, keeping the tail (most recent
/// output) when the blob exceeds that size.
const SCROLLBACK_CAP_BYTES: usize = 256 * 1024;

/// Save `blob` (the serialized scrollback contents) for `pane_id`, creating
/// the scrollback directory if needed and truncating to the last
/// `SCROLLBACK_CAP_BYTES` bytes if the blob is larger.
pub fn save_blob(pane_id: &str, blob: &str) -> std::io::Result<()> {
    let dir = scrollback_dir();
    std::fs::create_dir_all(&dir)?;
    let bytes = blob.as_bytes();
    let slice = if bytes.len() > SCROLLBACK_CAP_BYTES {
        &bytes[bytes.len() - SCROLLBACK_CAP_BYTES..]
    } else {
        bytes
    };
    std::fs::write(scrollback_file(pane_id), slice)
}

/// Load the persisted scrollback for `pane_id`. Returns an empty string
/// (not an error) if no scrollback has been saved for this pane yet.
pub fn load_blob(pane_id: &str) -> std::io::Result<String> {
    match std::fs::read_to_string(scrollback_file(pane_id)) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// Delete the persisted scrollback for `pane_id`, if any. A missing file is
/// not an error.
pub fn delete_blob(pane_id: &str) -> std::io::Result<()> {
    let path = scrollback_file(pane_id);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Real save -> load -> delete round-trip against a temp directory. We
    /// can't easily redirect `scrollback_dir()` (it isn't parameterized), so
    /// this test uses a real pane id under the real scrollback dir and
    /// cleans up after itself. That mirrors how `config::store` tests treat
    /// the OS config dir as available in CI.
    #[test]
    fn save_load_delete_round_trip() {
        let pane_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let blob = "hello scrollback\nsecond line\n";

        save_blob(pane_id, blob).expect("save_blob should succeed");
        let loaded = load_blob(pane_id).expect("load_blob should succeed");
        assert_eq!(loaded, blob);

        delete_blob(pane_id).expect("delete_blob should succeed");
        let after_delete = load_blob(pane_id).expect("load after delete should succeed");
        assert_eq!(after_delete, "", "deleted scrollback should load as empty");
    }

    #[test]
    fn load_blob_missing_pane_returns_empty() {
        let loaded =
            load_blob("00000000-0000-0000-0000-000000000000").expect("load should not error");
        assert_eq!(loaded, "");
    }

    #[test]
    fn save_blob_caps_to_tail() {
        let pane_id = "11111111-2222-3333-4444-555555555555";
        // Build a blob larger than the cap: a distinguishable head that
        // should be dropped entirely, plus a tail exactly as large as the
        // cap that should survive in full.
        let head = "H".repeat(1024);
        let tail = "T".repeat(SCROLLBACK_CAP_BYTES);
        let blob = format!("{head}{tail}");

        save_blob(pane_id, &blob).expect("save_blob should succeed");
        let loaded = load_blob(pane_id).expect("load_blob should succeed");
        assert_eq!(loaded.len(), SCROLLBACK_CAP_BYTES);
        assert_eq!(loaded, tail);
        assert!(
            !loaded.contains('H'),
            "head should have been truncated away"
        );

        delete_blob(pane_id).expect("cleanup delete should succeed");
    }
}
