//! Local drafts of unsaved editor content (spec §3.5, "belt and braces").
//!
//! The editor pane writes its dirty buffer here on a 2 s debounce, keyed by
//! pane id, so a crash, a killed process or a close path the unsaved-changes
//! guard cannot see (macOS Cmd+Q) does not take the edits with it. It is a
//! safety net, **not** autosave: nothing here ever touches the real file.
//!
//! Same lifecycle and the same shape as [`crate::scrollback`], in a sibling
//! directory: `<config_dir>/ymux/drafts/<pane-id>.json`. The blob is opaque
//! to Rust — the frontend's `src/editor/draft.ts` owns the format.
//!
//! Pure `std`, not behind `desktop`, so the tests run on Linux CI; the
//! `#[tauri::command]` wrappers live in `commands.rs`.

use std::io;
use std::path::{Path, PathBuf};

/// Refuse drafts over this size rather than truncate them: a cut-off draft
/// restored over a file is worse than no draft. Well above what an editable
/// file can produce (`textfile::MAX_EDIT_BYTES` is 8 MiB of text; JSON
/// escaping can inflate it, rarely by much).
pub const MAX_DRAFT_BYTES: usize = 32 * 1024 * 1024;

/// `<config_dir>/ymux/drafts`, falling back to a relative directory the way
/// `scrollback::scrollback_dir` does.
pub fn drafts_dir() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join("ymux").join("drafts"))
        .unwrap_or_else(|| PathBuf::from("./ymux-drafts"))
}

/// The draft file for `pane_id` under `base`. Anything that is not a hex
/// digit or `-` is stripped (the scrollback rule), so an id cannot name a
/// path outside `base`. An id that sanitises to nothing is refused.
fn draft_file_under(base: &Path, pane_id: &str) -> io::Result<PathBuf> {
    let safe: String = pane_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit() || *c == '-')
        .collect();
    if safe.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty pane id"));
    }
    Ok(base.join(format!("{safe}.json")))
}

fn save_under(base: &Path, pane_id: &str, blob: &str) -> io::Result<()> {
    if blob.len() > MAX_DRAFT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("draft of {} bytes is over the cap", blob.len()),
        ));
    }
    let path = draft_file_under(base, pane_id)?;
    std::fs::create_dir_all(base)?;
    // Temp file + rename: a crash mid-write leaves the previous draft, not a
    // truncated one.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, blob.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

fn load_under(base: &Path, pane_id: &str) -> io::Result<String> {
    match std::fs::read_to_string(draft_file_under(base, pane_id)?) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

fn delete_under(base: &Path, pane_id: &str) -> io::Result<()> {
    match std::fs::remove_file(draft_file_under(base, pane_id)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn list_under(base: &Path) -> io::Result<Vec<String>> {
    let iter = match std::fs::read_dir(base) {
        Ok(it) => it,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut ids = Vec::new();
    for entry in iter.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // `<id>.json` only: a `<id>.json.tmp` is a write in progress (or an
        // interrupted one), not a draft.
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            ids.push(id.to_string());
        }
    }
    ids.sort();
    Ok(ids)
}

/// The pane ids that have a draft on disk (the startup sweep's input).
pub fn list() -> io::Result<Vec<String>> {
    list_under(&drafts_dir())
}

/// Write `blob` as `pane_id`'s draft, replacing any previous one.
pub fn save(pane_id: &str, blob: &str) -> io::Result<()> {
    save_under(&drafts_dir(), pane_id, blob)
}

/// `pane_id`'s draft, or `""` if it has none.
pub fn load(pane_id: &str) -> io::Result<String> {
    load_under(&drafts_dir(), pane_id)
}

/// Forget `pane_id`'s draft. A missing draft is not an error.
pub fn delete(pane_id: &str) -> io::Result<()> {
    delete_under(&drafts_dir(), pane_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ymux-drafts-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("mkdir");
        base
    }

    const ID: &str = "0b9c2c3e-1f2a-4b5c-8d9e-0a1b2c3d4e5f";

    #[test]
    fn save_load_delete_round_trip() {
        let base = tempdir();
        let blob = r#"{"v":1,"text":"fn main() {}\n// 한글"}"#;
        save_under(&base, ID, blob).unwrap();
        assert_eq!(load_under(&base, ID).unwrap(), blob);
        // A second save replaces, and leaves no temp file behind.
        save_under(&base, ID, "{}").unwrap();
        assert_eq!(load_under(&base, ID).unwrap(), "{}");
        let names: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![format!("{ID}.json")]);
        delete_under(&base, ID).unwrap();
        assert_eq!(load_under(&base, ID).unwrap(), "");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn list_reports_draft_ids_and_ignores_temp_and_foreign_files() {
        let base = tempdir();
        assert!(list_under(&base.join("absent")).unwrap().is_empty());
        let other = "11111111-2222-3333-4444-555555555555";
        save_under(&base, ID, "{}").unwrap();
        save_under(&base, other, "{}").unwrap();
        std::fs::write(base.join(format!("{ID}.json.tmp")), "x").unwrap();
        std::fs::write(base.join("notes.txt"), "x").unwrap();
        std::fs::write(base.join("not an id.json"), "x").unwrap();
        assert_eq!(
            list_under(&base).unwrap(),
            vec![ID.to_string(), other.to_string()]
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_draft_loads_empty_and_deletes_quietly() {
        let base = tempdir();
        assert_eq!(load_under(&base, ID).unwrap(), "");
        delete_under(&base, ID).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_hostile_id_cannot_leave_the_directory() {
        let base = tempdir();
        let p = draft_file_under(&base, "../../etc/passwd").unwrap();
        assert_eq!(p.parent().unwrap(), base.as_path());
        assert!(!p.to_string_lossy().contains(".."));
        assert!(draft_file_under(&base, "../..").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_oversized_draft_is_refused_not_truncated() {
        let base = tempdir();
        save_under(&base, ID, "keep").unwrap();
        let big = "x".repeat(MAX_DRAFT_BYTES + 1);
        let err = save_under(&base, ID, &big).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        // The previous draft survives the refusal.
        assert_eq!(load_under(&base, ID).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(&base);
    }
}
