//! Pure, `std`-only logic for saving pasted clipboard images to disk and
//! time-pruning old ones. Deliberately free of any Tauri dependency so it
//! compiles and its tests run under `cargo test --no-default-features --lib
//! -p ymux` on Linux CI, unlike `commands.rs` (gated behind `desktop`). The
//! `#[tauri::command]` wrapper in `commands.rs` calls `save` and maps
//! `std::io::Error` to `YmuxError::Io`.
//!
//! Files are named `clip-<unix-millis>.png`; pruning parses that embedded
//! timestamp rather than the filesystem mtime, so it is deterministic and
//! testable without touching file times.

use std::path::{Path, PathBuf};

/// Directory pasted images are written to: `<config_dir>/ymux/paste-images`,
/// falling back to a relative directory if the OS config dir can't be
/// determined (mirrors `scrollback::scrollback_dir`).
pub fn paste_images_dir() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join("ymux").join("paste-images"))
        .unwrap_or_else(|| PathBuf::from("./ymux-paste-images"))
}

/// Path to the image file for a given millisecond timestamp under `base`.
fn file_under(base: &Path, millis: u128) -> PathBuf {
    base.join(format!("clip-{millis}.png"))
}

/// Parse the embedded millisecond timestamp out of a `clip-<millis>.png` file
/// name. Returns `None` for any name that doesn't match that exact shape.
fn parse_millis(name: &str) -> Option<u128> {
    name.strip_prefix("clip-")
        .and_then(|s| s.strip_suffix(".png"))
        .and_then(|s| s.parse::<u128>().ok())
}

/// Write `bytes` as `clip-<millis>.png` under `base` (created if needed) via a
/// temp file + rename so a crash mid-write can't leave a truncated image,
/// mirroring `scrollback::save_blob_under`. Returns the final path.
fn save_under(base: &Path, millis: u128, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(base)?;
    let path = file_under(base, millis);
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Delete every `clip-<millis>.png` in `base` whose embedded timestamp is
/// older than `retention_millis` relative to `now_millis`. Non-matching files
/// are left untouched. A per-file removal error is ignored so one locked file
/// can't abort pruning the rest.
fn prune_under(base: &Path, now_millis: u128, retention_millis: u128) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(millis) = parse_millis(name) {
            if now_millis.saturating_sub(millis) > retention_millis {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

/// Prune old pasted images, then save `bytes` as a new `clip-<now>.png` under
/// the real OS paste-images directory, returning its absolute path. `retention`
/// is how long a pasted image is kept before it becomes eligible for pruning.
pub fn save(bytes: &[u8], retention: std::time::Duration) -> std::io::Result<PathBuf> {
    let dir = paste_images_dir();
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = prune_under(&dir, now_millis, retention.as_millis());
    save_under(&dir, now_millis, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ymux-paste-images-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("mkdir");
        base
    }

    #[test]
    fn save_under_writes_png_and_returns_path() {
        let base = tempdir();
        let bytes = b"\x89PNG\r\n\x1a\nfake-png-bytes";
        let path = save_under(&base, 1234, bytes).expect("save_under should succeed");
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("clip-1234.png")
        );
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn parse_millis_reads_valid_and_rejects_other() {
        assert_eq!(parse_millis("clip-1710000000000.png"), Some(1710000000000));
        assert_eq!(parse_millis("clip-0.png"), Some(0));
        assert_eq!(parse_millis("clip-abc.png"), None);
        assert_eq!(parse_millis("notes.txt"), None);
        assert_eq!(parse_millis("clip-123.txt"), None);
    }

    #[test]
    fn prune_removes_old_keeps_recent() {
        let base = tempdir();
        save_under(&base, 1000, b"old").expect("save old");
        save_under(&base, 9000, b"recent").expect("save recent");
        // now = 10000, retention = 2000ms: 10000-1000=9000 > 2000 (drop),
        // 10000-9000=1000 < 2000 (keep).
        prune_under(&base, 10_000, 2_000).expect("prune should succeed");
        assert!(
            !file_under(&base, 1000).exists(),
            "old file should be pruned"
        );
        assert!(
            file_under(&base, 9000).exists(),
            "recent file should remain"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prune_ignores_non_clip_files() {
        let base = tempdir();
        std::fs::write(base.join("keepme.txt"), b"x").unwrap();
        save_under(&base, 1000, b"old").expect("save old");
        prune_under(&base, 10_000, 2_000).expect("prune should succeed");
        assert!(
            base.join("keepme.txt").exists(),
            "unrelated files must be left alone"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
