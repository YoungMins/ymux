//! Durable, atomic replacement of small state files (config, agent sessions,
//! scrollback, editor drafts, Claude Code `settings.json`).
//!
//! A write goes to a uniquely named sibling temp file, is flushed to disk
//! (`sync_all`) and only then renamed over the target, so a crash or power
//! loss leaves either the old content or the new one — never an empty or
//! truncated file. On Unix the parent directory is fsynced afterwards
//! (best-effort) so the rename itself survives a crash. The temp file never
//! outlives a failure.
//!
//! Pure `std` (plus `uuid` for the temp name), not behind `desktop`.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The temp file a write to `path` goes through: a hidden sibling in the same
/// directory (same volume, so the rename is atomic), unique per write so
/// concurrent writers never share one. It ends in `.tmp`, never in the
/// target's own extension, so directory listings that match on the extension
/// (`drafts::list`) skip an in-flight write.
pub fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let tmp = format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple());
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(tmp),
        _ => PathBuf::from(tmp),
    }
}

/// Atomically and durably replace `path` with `bytes`. The parent directory
/// must already exist. The new file gets default (umask) permissions.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, None)
}

/// [`write_atomic`], with the new file carrying `perms` (typically the
/// replaced file's own, so a `0600` file does not come back `0644`). On Unix
/// the mode is applied before any byte is written, so the content is never
/// readable under looser permissions; on Windows only the read-only flag
/// exists, and it is applied after the content is on disk.
pub fn write_atomic_with(
    path: &Path,
    bytes: &[u8],
    perms: Option<&fs::Permissions>,
) -> io::Result<()> {
    let tmp = temp_path(path);
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        #[cfg(unix)]
        if let Some(p) = perms {
            file.set_permissions(p.clone())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(not(unix))]
        if let Some(p) = perms {
            if p.readonly() {
                file.set_permissions(p.clone())?;
            }
        }
        drop(file);
        fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    sync_parent(path);
    Ok(())
}

/// Best-effort fsync of `path`'s directory, making a rename into it durable.
/// A no-op where directories cannot be opened (Windows).
pub fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ymux-durable-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&base).expect("mkdir");
        base
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .expect("read_dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn replaces_content_and_leaves_no_temp_file() {
        let dir = tempdir();
        let path = dir.join("a.json");
        write_atomic(&path, b"one").expect("write");
        write_atomic(&path, b"two").expect("rewrite");
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(entries(&dir), vec!["a.json".to_string()]);
    }

    #[test]
    fn failed_rename_removes_the_temp_file_and_keeps_the_target() {
        let dir = tempdir();
        // A directory in the target's place: the rename fails.
        let path = dir.join("a.json");
        fs::create_dir(&path).expect("mkdir target");
        fs::write(path.join("keep"), b"x").expect("non-empty");
        assert!(write_atomic(&path, b"new").is_err());
        assert_eq!(entries(&dir), vec!["a.json".to_string()]);
        assert!(path.join("keep").is_file());
    }

    #[test]
    fn missing_parent_is_an_error_not_a_panic() {
        let dir = tempdir();
        assert!(write_atomic(&dir.join("nope").join("a.json"), b"x").is_err());
    }

    #[test]
    fn temp_name_is_a_hidden_tmp_sibling() {
        let t = temp_path(Path::new("/x/y/draft.json"));
        assert_eq!(t.parent(), Some(Path::new("/x/y")));
        let name = t.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with(".draft.json.") && name.ends_with(".tmp"),
            "{name}"
        );
        assert_ne!(t, temp_path(Path::new("/x/y/draft.json")));
    }

    #[cfg(unix)]
    #[test]
    fn permissions_are_carried_over() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir();
        let path = dir.join("settings.json");
        fs::write(&path, b"{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let perms = fs::metadata(&path).unwrap().permissions();
        write_atomic_with(&path, b"{\"a\":1}", Some(&perms)).expect("write");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
