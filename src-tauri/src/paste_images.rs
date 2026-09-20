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

/// Directory pasted images are written to: `<cache_dir>/ymux/paste-images`,
/// falling back to a relative directory if the OS cache dir can't be
/// determined. Unlike `scrollback::scrollback_dir` (which intentionally
/// lives under `config_dir` because scrollback is user data worth keeping),
/// pasted images are transient, self-pruning, and can be multi-MB — putting
/// them under `config_dir` (Windows: Roaming AppData) would sync them to a
/// corporate profile server on every logoff. `cache_dir` (Windows: Local
/// AppData) is the correct home for this kind of disposable temp data.
pub fn paste_images_dir() -> PathBuf {
    dirs::cache_dir()
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
/// older than `retention_millis` relative to `now_millis`. Also reclaims
/// stale `clip-<millis>.png.tmp` files using the same age rule — `save_under`
/// writes that name before its rename to `.png`, so a crash between the two
/// would otherwise leave an orphan that `parse_millis` (which only matches
/// `.png`) skips forever. Non-matching files (anything that isn't a
/// `clip-<millis>.png[.tmp]`) are left untouched. A per-file removal error is
/// ignored so one locked file can't abort pruning the rest.
fn prune_under(base: &Path, now_millis: u128, retention_millis: u128) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let millis =
            parse_millis(name).or_else(|| name.strip_suffix(".tmp").and_then(parse_millis));
        if let Some(millis) = millis {
            if now_millis.saturating_sub(millis) > retention_millis {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

/// An `InvalidInput` error with `msg`, for the shape checks below.
fn invalid(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, msg)
}

/// Encode raw 8-bit RGBA pixels as a PNG in memory.
///
/// This is the half of the clipboard-image path that has no OS dependency, so
/// it lives here (with the rest of the `std`-only paste-image logic) rather
/// than next to the `arboard` call in `clipboard_image.rs` — that module is
/// `desktop`-gated and its tests never run on Linux CI, while these do.
///
/// Rejects a degenerate image rather than producing a technically-valid but
/// useless file: a 0×0 encode yields a small, well-formed PNG that would sail
/// straight past the emptiness check in [`save`].
pub fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> std::io::Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(invalid("clipboard image has zero width or height"));
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| invalid("clipboard image dimensions overflow"))?;
    if rgba.len() != expected {
        return Err(invalid(&format!(
            "clipboard image is {} bytes, expected {expected} for {width}x{height} RGBA",
            rgba.len()
        )));
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| std::io::Error::other(format!("png header: {e}")))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| std::io::Error::other(format!("png data: {e}")))?;
        writer
            .finish()
            .map_err(|e| std::io::Error::other(format!("png finish: {e}")))?;
    }
    Ok(out)
}

/// Prune old pasted images, then save `bytes` as a new `clip-<now>.png` under
/// the real OS paste-images directory, returning its absolute path. `retention`
/// is how long a pasted image is kept before it becomes eligible for pruning.
///
/// Empty input is an error, never a 0-byte file: the original bug here was the
/// webview's `navigator.clipboard.read()` handing over an empty `Uint8Array`,
/// which this function faithfully wrote to disk as a 0-byte `.png` whose path
/// was then typed into the shell. A caller with nothing to save must fail
/// loudly instead.
pub fn save(bytes: &[u8], retention: std::time::Duration) -> std::io::Result<PathBuf> {
    if bytes.is_empty() {
        return Err(invalid("refusing to save an empty paste image"));
    }
    let dir = paste_images_dir();
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    if let Err(e) = prune_under(&dir, now_millis, retention.as_millis()) {
        // A prune failure must never fail the user's paste — but it should
        // still be diagnosable, so log it instead of silently swallowing it.
        tracing::warn!(error = %e, "failed to prune old paste images");
    }
    save_under(&dir, now_millis, bytes)
}

/// Prune old pasted images under the real OS paste-images directory without
/// saving a new one. Intended to be called once at app startup so images
/// left over from a previous session don't outlive `retention` just because
/// no new paste ever triggered the prune in `save`. Mirrors the prune half
/// of `save`.
pub fn prune(retention: std::time::Duration) -> std::io::Result<()> {
    let dir = paste_images_dir();
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    prune_under(&dir, now_millis, retention.as_millis())
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

    #[test]
    fn prune_reclaims_stale_tmp_but_not_unrelated_files() {
        let base = tempdir();
        std::fs::write(base.join("keepme.txt"), b"x").unwrap();
        // Simulate a crash between save_under's write and rename: a stale
        // orphaned tmp file with an old embedded timestamp.
        std::fs::write(base.join("clip-1000.png.tmp"), b"orphan").unwrap();
        prune_under(&base, 10_000, 2_000).expect("prune should succeed");
        assert!(
            !base.join("clip-1000.png.tmp").exists(),
            "stale orphaned .tmp file should be reclaimed"
        );
        assert!(
            base.join("keepme.txt").exists(),
            "unrelated files must still be left alone"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn save_rejects_empty_bytes() {
        // The 0-byte-PNG bug: an empty payload must be an error, and must not
        // create a file. `save` checks before it touches the filesystem, so
        // this is safe to call against the real paste-images dir.
        let err = save(&[], std::time::Duration::from_secs(60))
            .expect_err("empty input must be rejected");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn encode_png_rgba_rejects_degenerate_images() {
        assert_eq!(
            encode_png_rgba(0, 0, &[]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput,
            "0x0 would encode to a valid-but-useless PNG"
        );
        assert_eq!(
            encode_png_rgba(2, 0, &[]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            encode_png_rgba(2, 2, &[0u8; 8]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput,
            "2x2 RGBA needs 16 bytes"
        );
    }

    #[test]
    fn encode_png_rgba_round_trips_through_save() {
        // A tiny generated image, not the real clipboard: clipboard access
        // needs a desktop session, so the OS half is covered by the #[ignore]d
        // test in `clipboard_image.rs` instead.
        let base = tempdir();
        #[rustfmt::skip]
        let rgba: [u8; 16] = [
            255, 0, 0, 255,    0, 255, 0, 255,
            0, 0, 255, 128,    9, 9, 9, 0,
        ];
        let png_bytes = encode_png_rgba(2, 2, &rgba).expect("encode");
        assert!(!png_bytes.is_empty());
        assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");

        let path = save_under(&base, 4242, &png_bytes).expect("save");
        let on_disk = std::fs::read(&path).expect("read back");
        assert_eq!(on_disk, png_bytes);

        let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
        let mut reader = decoder.read_info().expect("decode header");
        let info = reader.info().clone();
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let frame = reader.next_frame(&mut buf).expect("decode pixels");
        assert_eq!(
            &buf[..frame.buffer_size()],
            &rgba[..],
            "pixels must survive the round trip, alpha included"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prune_keeps_file_exactly_at_retention_boundary() {
        let base = tempdir();
        // now - millis == retention_millis exactly: the strict `>` check
        // means this is NOT yet past retention, so it must be kept.
        save_under(&base, 8_000, b"boundary").expect("save boundary");
        prune_under(&base, 10_000, 2_000).expect("prune should succeed");
        assert!(
            file_under(&base, 8_000).exists(),
            "file exactly at the retention boundary should be kept"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
