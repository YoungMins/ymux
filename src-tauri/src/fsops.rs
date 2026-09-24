//! The filesystem command surface: `#[tauri::command]`s that read, write,
//! create, rename, copy, move and delete files.
//!
//! `desktop`-gated, because every command takes a `Webview` and an
//! `ipc::Request` and returns through the Tauri IPC. The *decisions* —
//! what to list, how to sort, whether a write is safe, whether a path is
//! the same path — live in [`crate::fsx`] and [`crate::textfile`], which
//! are not gated and are unit-tested on Linux (CLAUDE.md rule 1).
//!
//! ## Security, since this is the largest attack-surface change the app has
//! made
//!
//! **Every command here can touch an arbitrary absolute path.** There is no
//! allow-list and deliberately so (spec §1.5 rule 5): a multiplexer whose
//! panes already run arbitrary shells cannot meaningfully sandbox its own
//! file pane, so the boundary that matters is *who can call*, not *what
//! path*. That boundary is [`crate::fspath::guard_local`], called on the
//! first line of every command, and it is the **only** gate — ymux's own
//! commands sit outside Tauri's capability system entirely (see that
//! function's doc for the source-level evidence).
//!
//! Consequences worth stating plainly:
//!
//!  - `fsx::is_within` is **lexical**. It is not used as a boundary check
//!    anywhere here, because there is no root to escape. A symlink inside a
//!    directory that points outside it is not a privilege escalation on
//!    this surface — the caller could have named the outside path directly.
//!    What symlinks *are* is a data-loss hazard, so: a delete removes the
//!    link and never the target, and a copy refuses a link rather than
//!    silently following it.
//!  - **Nothing here can be made to run a program.** No command takes an
//!    argv, a program name or a shell string; no `Command` is built.
//!    [`fs_open_default`] is the only path that reaches the OS handler at
//!    all, and it goes through [`crate::fspath::validate_open`] and
//!    [`crate::fspath::should_reveal`], which reveal an executable, a
//!    script, a shortcut or a macOS bundle in the file manager instead of
//!    launching it. That is a denylist and cannot be complete, which is
//!    why it is the single chokepoint.
//!
//! Every command is `#[tauri::command(async)]` so it runs off the main
//! thread: a dead network drive blocks `metadata` for tens of seconds and
//! must not freeze the window.

use std::fs;
use std::io;
use std::path::Path;

use serde::Deserialize;

use crate::error::{YmuxError, YmuxResult};
use crate::fspath::guard_local;
use crate::fsx::{self, DirEntryInfo};
use crate::textfile::{self, ContentStamp, Eol, TextFile};

/// Arguments to [`fs_write_text`].
///
/// Named `WriteTextArgs`, not the spec's `WriteArgs`: `commands.rs` already
/// has a `WriteArgs` for PTY writes and two structs of that name in one
/// crate is a trap.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteTextArgs {
    pub path: String,
    pub text: String,
    pub eol: Eol,
    pub bom: bool,
    /// The stamp the caller last read. `Some` makes the write conditional:
    /// it fails with [`YmuxError::Conflict`] if the file on disk no longer
    /// matches, which is what stops the editor clobbering an agent's edit
    /// (spec §3.6). `None` is an unconditional write — a new file, or an
    /// explicit "overwrite anyway" after a conflict.
    #[serde(default)]
    pub expect: Option<ContentStamp>,
}

/// Upper bound on [`fs_read_head`], whatever the caller asks for. Matches the
/// preview's `MAX_PREVIEW_BYTES` (`src/files/preview.ts`): a preview of a
/// 2 GB log must cost the same as a preview of a README.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Entries [`fs_peek_dir`] walks before giving up and reporting `more`.
pub const MAX_PEEK_SCAN: usize = 512;

/// One row of [`fs_peek_dir`]: just enough to draw a directory preview.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PeekEntry {
    pub name: String,
    pub is_dir: bool,
}

/// A bounded look inside a directory. `more` means the walk stopped early.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DirPeek {
    pub entries: Vec<PeekEntry>,
    pub more: bool,
}

// ---------------------------------------------------------------------------
// Helpers. Not `pub`: they take already-guarded input.
// ---------------------------------------------------------------------------

fn mtime_ms(md: &fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Is this entry hidden by the OS, as opposed to by its name?
///
/// Windows has a hidden *attribute* that `tools/ydir` ignores; honouring it
/// is one of the few places the pane is deliberately better than the TUI it
/// replaces.
#[cfg(windows)]
fn os_hidden(md: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn os_hidden(_md: &fs::Metadata) -> bool {
    false
}

/// Build a row for `path`, resolving a symlink's target for `is_dir`/`size`
/// while still reporting that it *is* a link.
fn entry_info(path: &Path) -> YmuxResult<DirEntryInfo> {
    let p = path.to_string_lossy().into_owned();
    let link_md = fs::symlink_metadata(path).map_err(|e| YmuxError::from_io(&e, &p))?;
    let is_symlink = link_md.file_type().is_symlink();
    // A broken link still deserves a row, so the target's metadata is
    // best-effort and falls back to the link's own.
    let md = if is_symlink {
        fs::metadata(path).unwrap_or_else(|_| link_md.clone())
    } else {
        link_md
    };
    Ok(DirEntryInfo {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            // A root (`C:\`, `/`) has no file name; show the path itself.
            .unwrap_or_else(|| p.clone()),
        path: p,
        is_dir: md.is_dir(),
        is_symlink,
        size: if md.is_dir() { 0 } else { md.len() },
        modified_ms: mtime_ms(&md),
    })
}

/// Refuse a destination that already exists, unless it is the same file the
/// source is — which is how a case-only rename (`Foo.txt` → `foo.txt`) looks
/// on a case-insensitive volume.
fn check_destination_free(from: &str, to: &str, overwrite: bool) -> YmuxResult<()> {
    if overwrite || !Path::new(to).exists() {
        return Ok(());
    }
    if fsx::same_file(from, to) {
        return Ok(());
    }
    Err(YmuxError::AlreadyExists(to.to_string()))
}

/// Copy `from` to `to` recursively, treating a symlink as something to
/// refuse rather than follow.
fn copy_tree(from: &Path, to: &Path, overwrite: bool) -> YmuxResult<()> {
    let from_s = from.to_string_lossy().into_owned();
    let md = fs::symlink_metadata(from).map_err(|e| YmuxError::from_io(&e, &from_s))?;

    if md.file_type().is_symlink() {
        // Following it would copy the target's contents under the link's
        // name, which is a silent change of meaning, and re-creating it as a
        // link needs a privilege Windows does not grant by default.
        return Err(YmuxError::Other(format!(
            "{from_s}: refusing to copy a symbolic link (copy its target instead)"
        )));
    }

    if md.is_dir() {
        fs::create_dir_all(to).map_err(|e| YmuxError::from_io(&e, &to.to_string_lossy()))?;
        for entry in fs::read_dir(from).map_err(|e| YmuxError::from_io(&e, &from_s))? {
            let entry = entry.map_err(|e| YmuxError::from_io(&e, &from_s))?;
            copy_tree(&entry.path(), &to.join(entry.file_name()), overwrite)?;
        }
        Ok(())
    } else {
        if !overwrite && to.exists() {
            return Err(YmuxError::AlreadyExists(to.to_string_lossy().into_owned()));
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| YmuxError::from_io(&e, &parent.to_string_lossy()))?;
        }
        fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| YmuxError::from_io(&e, &to.to_string_lossy()))
    }
}

/// Remove one path. A symlink loses the link, never the target.
fn delete_one(path: &Path) -> YmuxResult<()> {
    let p = path.to_string_lossy().into_owned();
    let md = fs::symlink_metadata(path).map_err(|e| YmuxError::from_io(&e, &p))?;
    if md.file_type().is_symlink() {
        // On Windows a *directory* symlink or junction must go through
        // `remove_dir`; on Unix `remove_file` always works. Trying both,
        // rather than a `cfg`, keeps the target safe on either.
        return fs::remove_file(path)
            .or_else(|_| fs::remove_dir(path))
            .map_err(|e| YmuxError::from_io(&e, &p));
    }
    if md.is_dir() {
        fs::remove_dir_all(path).map_err(|e| YmuxError::from_io(&e, &p))
    } else {
        fs::remove_file(path).map_err(|e| YmuxError::from_io(&e, &p))
    }
}

/// The implementation half of each command, factored out so the desktop
/// integration tests can drive it without a webview.
pub(crate) mod imp {
    use super::*;

    pub fn list_dir(path: &str, show_hidden: bool) -> YmuxResult<Vec<DirEntryInfo>> {
        let dir = Path::new(path);
        let mut out = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| YmuxError::from_io(&e, path))? {
            // One unreadable row must not fail the listing.
            let Ok(entry) = entry else { continue };
            let Ok(mut info) = entry_info(&entry.path()) else {
                continue;
            };
            if !show_hidden {
                if fsx::is_hidden_name(&info.name) {
                    continue;
                }
                if fs::symlink_metadata(entry.path())
                    .map(|md| os_hidden(&md))
                    .unwrap_or(false)
                {
                    continue;
                }
            }
            info.name = entry.file_name().to_string_lossy().into_owned();
            out.push(info);
        }
        fsx::sort_entries(&mut out);
        Ok(out)
    }

    pub fn roots() -> Vec<String> {
        #[cfg(windows)]
        {
            // Probing each letter rather than `GetLogicalDrives` keeps the
            // `windows` feature list as it is. It runs off the main thread,
            // so a slow removable drive costs nothing visible.
            (b'A'..=b'Z')
                .map(|c| format!("{}:\\", c as char))
                .filter(|d| Path::new(d).is_dir())
                .collect()
        }
        #[cfg(not(windows))]
        {
            vec!["/".to_string()]
        }
    }

    pub fn home_dir() -> YmuxResult<String> {
        dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or_else(|| YmuxError::NotFound("home directory".into()))
    }

    pub fn stat(path: &str) -> YmuxResult<DirEntryInfo> {
        entry_info(Path::new(path))
    }

    pub fn create_dir(path: &str) -> YmuxResult<()> {
        // `create_dir`, not `create_dir_all`: a missing parent is a mistake
        // worth reporting, and an existing target must be `AlreadyExists`.
        fs::create_dir(path).map_err(|e| YmuxError::from_io(&e, path))
    }

    pub fn create_file(path: &str) -> YmuxResult<()> {
        // `create_new` is atomic, so this cannot race a concurrent create
        // into silently truncating someone's file.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(|_| ())
            .map_err(|e| YmuxError::from_io(&e, path))
    }

    pub fn rename(from: &str, to: &str) -> YmuxResult<()> {
        // `fs::rename` silently overwrites the destination on both Windows
        // and Unix, which for a rename in a file manager is data loss.
        check_destination_free(from, to, false)?;
        fs::rename(from, to).map_err(|e| YmuxError::from_io(&e, to))
    }

    pub fn copy(from: &str, to: &str, overwrite: bool) -> YmuxResult<()> {
        if fsx::same_file(from, to) {
            return Err(YmuxError::Other(format!(
                "{from}: source and destination are the same path"
            )));
        }
        // Copying a directory into its own subtree recurses forever.
        if Path::new(from).is_dir() && fsx::is_within(from, to) {
            return Err(YmuxError::Other(format!(
                "{to}: cannot copy a directory into itself"
            )));
        }
        check_destination_free(from, to, overwrite)?;
        copy_tree(Path::new(from), Path::new(to), overwrite)
    }

    pub fn move_path(from: &str, to: &str, overwrite: bool) -> YmuxResult<()> {
        if fsx::same_file(from, to) {
            // A case-only rename on a case-insensitive volume is a real
            // move; anything else is a no-op.
            return fs::rename(from, to).map_err(|e| YmuxError::from_io(&e, to));
        }
        if Path::new(from).is_dir() && fsx::is_within(from, to) {
            return Err(YmuxError::Other(format!(
                "{to}: cannot move a directory into itself"
            )));
        }
        check_destination_free(from, to, overwrite)?;
        match fs::rename(from, to) {
            Ok(()) => Ok(()),
            // A rename cannot cross a volume; fall back to copy + delete,
            // which is what every file manager does.
            Err(e) if is_cross_device(&e) => {
                copy_tree(Path::new(from), Path::new(to), overwrite)?;
                delete_one(Path::new(from))
            }
            Err(e) => Err(YmuxError::from_io(&e, to)),
        }
    }

    pub fn delete(paths: &[String], to_trash: bool) -> YmuxResult<()> {
        if to_trash {
            // The `trash` crate reports one error for the batch; map it
            // whole rather than guessing which path failed.
            return trash::delete_all(paths)
                .map_err(|e| YmuxError::Other(format!("move to trash failed: {e}")));
        }
        for p in paths {
            delete_one(Path::new(p))?;
        }
        Ok(())
    }

    pub fn read_text(path: &str) -> YmuxResult<TextFile> {
        use std::io::Read;

        let md = fs::metadata(path).map_err(|e| YmuxError::from_io(&e, path))?;
        if md.is_dir() {
            return Err(YmuxError::Other(format!("{path} is a directory")));
        }
        let mut f = fs::File::open(path).map_err(|e| YmuxError::from_io(&e, path))?;
        // Read at most the cap plus a few bytes, so a file that is exactly
        // at the cap is not reported truncated and a cut multi-byte
        // character still has its start in the buffer.
        let mut buf = Vec::new();
        f.by_ref()
            .take(textfile::MAX_EDIT_BYTES as u64 + 4)
            .read_to_end(&mut buf)
            .map_err(|e| YmuxError::from_io(&e, path))?;

        // A NUL in the head means the editor must not load this as text.
        // `NotUtf8` is the honest answer for the UI's purposes: the pane's
        // banner for it is a read-only byte notice, which is exactly right
        // for a binary file too.
        if fsx::is_probably_binary(&buf) {
            return Err(YmuxError::NotUtf8(format!("{path} (binary)")));
        }

        let total = md.len();
        let keep = buf.len().min(textfile::MAX_EDIT_BYTES);
        textfile::decode(&buf[..keep], mtime_ms(&md), total, path)
    }

    pub fn write_text(args: &WriteTextArgs) -> YmuxResult<ContentStamp> {
        let path = args.path.as_str();
        let bytes = textfile::encode(&args.text, args.eol, args.bom, path)?;

        if let Some(expect) = &args.expect {
            // Compared on the **hash only**, not on the mtime. A formatter
            // or a `touch` that rewrites identical bytes is not a conflict:
            // overwriting them loses nothing, and reporting one would make
            // the editor cry wolf on every agent run that reformatted and
            // changed nothing. A missing file *is* a conflict — it was
            // deleted under the buffer.
            match current_stamp(path)? {
                Some(on_disk) if on_disk.sha256 == expect.sha256 => {}
                _ => return Err(YmuxError::Conflict(path.to_string())),
            }
        }

        fs::write(path, &bytes).map_err(|e| YmuxError::from_io(&e, path))?;
        let md = fs::metadata(path).map_err(|e| YmuxError::from_io(&e, path))?;
        Ok(textfile::stamp_of(&bytes, mtime_ms(&md)))
    }

    /// The first `max` bytes of a file (at most [`MAX_HEAD_BYTES`]), for the
    /// files pane's preview and its binary sniff before "open". Unlike
    /// [`read_text`] it neither decodes nor refuses binary content: the
    /// preview's decisions live in `src/files/preview.ts`.
    pub fn read_head(path: &str, max: usize) -> YmuxResult<Vec<u8>> {
        use std::io::Read;

        let md = fs::metadata(path).map_err(|e| YmuxError::from_io(&e, path))?;
        if md.is_dir() {
            return Err(YmuxError::Other(format!("{path} is a directory")));
        }
        let f = fs::File::open(path).map_err(|e| YmuxError::from_io(&e, path))?;
        let mut buf = Vec::new();
        f.take(max.min(MAX_HEAD_BYTES) as u64)
            .read_to_end(&mut buf)
            .map_err(|e| YmuxError::from_io(&e, path))?;
        Ok(buf)
    }

    /// The head of a directory for the preview: names and kinds only, no
    /// per-entry `stat`, walking at most [`MAX_PEEK_SCAN`] entries so a
    /// cursor resting on `node_modules` costs the same as on any folder.
    /// Counts entries *walked*, not kept, as `tools/ydir`'s preview did.
    pub fn peek_dir(path: &str, show_hidden: bool) -> YmuxResult<DirPeek> {
        let mut entries = Vec::new();
        let mut more = false;
        let iter = fs::read_dir(path).map_err(|e| YmuxError::from_io(&e, path))?;
        for (scanned, entry) in iter.enumerate() {
            if scanned >= MAX_PEEK_SCAN {
                more = true;
                break;
            }
            let Ok(entry) = entry else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if !show_hidden {
                if fsx::is_hidden_name(&name) {
                    continue;
                }
                // On Windows this metadata comes from the directory read
                // itself, so it costs no extra syscall.
                if entry.metadata().map(|md| os_hidden(&md)).unwrap_or(false) {
                    continue;
                }
            }
            let is_dir = match entry.file_type() {
                Ok(t) if t.is_symlink() => entry.path().is_dir(),
                Ok(t) => t.is_dir(),
                Err(_) => false,
            };
            entries.push(PeekEntry { name, is_dir });
        }
        Ok(DirPeek { entries, more })
    }

    /// The stamp of the file as it is right now, or `None` if it is gone.
    fn current_stamp(path: &str) -> YmuxResult<Option<ContentStamp>> {
        let md = match fs::metadata(path) {
            Ok(md) => md,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(YmuxError::from_io(&e, path)),
        };
        let bytes = fs::read(path).map_err(|e| YmuxError::from_io(&e, path))?;
        Ok(Some(textfile::stamp_of(&bytes, mtime_ms(&md))))
    }

    pub fn reveal(path: &str) -> YmuxResult<()> {
        crate::fspath::validate_open(path).map_err(YmuxError::Other)?;
        opener::reveal(Path::new(path)).map_err(|e| YmuxError::Other(format!("reveal {path}: {e}")))
    }

    /// Hand `path` to the OS default handler — *or* reveal it, if opening
    /// it would run it.
    pub fn open_default(path: &str) -> YmuxResult<()> {
        crate::fspath::validate_open(path).map_err(YmuxError::Other)?;
        let p = Path::new(path);
        let md = fs::metadata(p).map_err(|e| YmuxError::from_io(&e, path))?;
        // The whole point: an `.exe`, `.bat`, `.lnk` or `.app` is shown in
        // the file manager, never executed. See `fspath::should_reveal`.
        let result = if crate::fspath::should_reveal(p, md.is_dir()) {
            opener::reveal(p)
        } else {
            opener::open(p)
        };
        result.map_err(|e| YmuxError::Other(format!("open {path}: {e}")))
    }

    fn is_cross_device(e: &io::Error) -> bool {
        #[cfg(unix)]
        {
            // EXDEV
            if e.raw_os_error() == Some(18) {
                return true;
            }
        }
        #[cfg(windows)]
        {
            // ERROR_NOT_SAME_DEVICE
            if e.raw_os_error() == Some(17) {
                return true;
            }
        }
        // `io::ErrorKind::CrossesDevices` would read better but postdates
        // this crate's `rust-version = "1.77"`; the two errnos above cover
        // both shipping platforms.
        false
    }
}

// ---------------------------------------------------------------------------
// The commands. Each one is `guard_local` and then a call into `imp`.
// ---------------------------------------------------------------------------

macro_rules! guarded {
    ($webview:expr, $request:expr, $name:literal) => {
        guard_local(&$webview, &$request, $name)?
    };
}

#[tauri::command(async)]
pub fn fs_list_dir(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    path: String,
    show_hidden: bool,
) -> YmuxResult<Vec<DirEntryInfo>> {
    guarded!(webview, request, "fs_list_dir");
    imp::list_dir(&path, show_hidden)
}

#[tauri::command(async)]
pub fn fs_roots(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
) -> YmuxResult<Vec<String>> {
    guarded!(webview, request, "fs_roots");
    Ok(imp::roots())
}

#[tauri::command(async)]
pub fn fs_home_dir(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
) -> YmuxResult<String> {
    guarded!(webview, request, "fs_home_dir");
    imp::home_dir()
}

#[tauri::command(async)]
pub fn fs_stat(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    path: String,
) -> YmuxResult<DirEntryInfo> {
    guarded!(webview, request, "fs_stat");
    imp::stat(&path)
}

#[tauri::command(async)]
pub fn fs_create_dir(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    path: String,
) -> YmuxResult<()> {
    guarded!(webview, request, "fs_create_dir");
    imp::create_dir(&path)
}

#[tauri::command(async)]
pub fn fs_create_file(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    path: String,
) -> YmuxResult<()> {
    guarded!(webview, request, "fs_create_file");
    imp::create_file(&path)
}

#[tauri::command(async)]
pub fn fs_rename(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    from: String,
    to: String,
) -> YmuxResult<()> {
    guarded!(webview, request, "fs_rename");
    imp::rename(&from, &to)
}

#[tauri::command(async)]
pub fn fs_copy(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    from: String,
    to: String,
    overwrite: bool,
) -> YmuxResult<()> {
    guarded!(webview, request, "fs_copy");
    imp::copy(&from, &to, overwrite)
}

#[tauri::command(async)]
pub fn fs_move(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    from: String,
    to: String,
    overwrite: bool,
) -> YmuxResult<()> {
    guarded!(webview, request, "fs_move");
    imp::move_path(&from, &to, overwrite)
}

#[tauri::command(async)]
pub fn fs_delete(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    paths: Vec<String>,
    to_trash: bool,
) -> YmuxResult<()> {
    guarded!(webview, request, "fs_delete");
    imp::delete(&paths, to_trash)
}

#[tauri::command(async)]
pub fn fs_read_text(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    path: String,
) -> YmuxResult<TextFile> {
    guarded!(webview, request, "fs_read_text");
    imp::read_text(&path)
}

/// Raw bytes, not JSON: `tauri::ipc::Response` reaches the frontend as an
/// `ArrayBuffer`, where a `Vec<u8>` would be a 64 Ki-element number array.
#[tauri::command(async)]
pub fn fs_read_head(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    path: String,
    max_bytes: usize,
) -> YmuxResult<tauri::ipc::Response> {
    guarded!(webview, request, "fs_read_head");
    imp::read_head(&path, max_bytes).map(tauri::ipc::Response::new)
}

#[tauri::command(async)]
pub fn fs_peek_dir(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    path: String,
    show_hidden: bool,
) -> YmuxResult<DirPeek> {
    guarded!(webview, request, "fs_peek_dir");
    imp::peek_dir(&path, show_hidden)
}

#[tauri::command(async)]
pub fn fs_write_text(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    args: WriteTextArgs,
) -> YmuxResult<ContentStamp> {
    guarded!(webview, request, "fs_write_text");
    imp::write_text(&args)
}

#[tauri::command(async)]
pub fn fs_reveal(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    path: String,
) -> YmuxResult<()> {
    guarded!(webview, request, "fs_reveal");
    imp::reveal(&path)
}

#[tauri::command(async)]
pub fn fs_open_default(
    webview: tauri::Webview,
    request: tauri::ipc::Request<'_>,
    path: String,
) -> YmuxResult<()> {
    guarded!(webview, request, "fs_open_default");
    imp::open_default(&path)
}

/// `tempfile`-backed round-trips for the real filesystem semantics. These
/// only build in the desktop configuration, so they run on Windows and
/// macOS — which is the point: the semantics they pin (silent overwrite on
/// rename, a directory symlink needing `remove_dir`) are per-platform.
#[cfg(test)]
mod tests {
    use super::imp;
    use super::*;
    use std::path::PathBuf;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn s(p: PathBuf) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn create_list_and_stat_round_trip() {
        let d = tmp();
        let sub = d.path().join("sub");
        imp::create_dir(&s(sub.clone())).unwrap();
        imp::create_file(&s(d.path().join("a.txt"))).unwrap();
        imp::create_file(&s(d.path().join(".hidden"))).unwrap();

        let visible = imp::list_dir(&s(d.path().to_path_buf()), false).unwrap();
        let names: Vec<_> = visible.iter().map(|e| e.name.as_str()).collect();
        // Directory first, dotfile filtered out.
        assert_eq!(names, vec!["sub", "a.txt"]);

        let all = imp::list_dir(&s(d.path().to_path_buf()), true).unwrap();
        assert_eq!(all.len(), 3);

        let st = imp::stat(&s(sub)).unwrap();
        assert!(st.is_dir);
        assert!(!st.is_symlink);
        assert_eq!(st.name, "sub");
    }

    #[test]
    fn creating_something_that_exists_is_already_exists() {
        let d = tmp();
        let f = s(d.path().join("a.txt"));
        imp::create_file(&f).unwrap();
        assert_eq!(imp::create_file(&f).unwrap_err().kind(), "already_exists");

        let g = s(d.path().join("sub"));
        imp::create_dir(&g).unwrap();
        assert_eq!(imp::create_dir(&g).unwrap_err().kind(), "already_exists");
    }

    #[test]
    fn listing_a_missing_directory_is_not_found() {
        let d = tmp();
        let missing = s(d.path().join("nope"));
        assert_eq!(
            imp::list_dir(&missing, false).unwrap_err().kind(),
            "not_found"
        );
        assert_eq!(imp::stat(&missing).unwrap_err().kind(), "not_found");
    }

    /// `fs::rename` silently overwrites on both platforms, so the refusal
    /// has to be ours.
    #[test]
    fn rename_refuses_to_clobber_an_existing_file() {
        let d = tmp();
        let a = s(d.path().join("a.txt"));
        let b = s(d.path().join("b.txt"));
        std::fs::write(&a, b"A").unwrap();
        std::fs::write(&b, b"B").unwrap();

        assert_eq!(imp::rename(&a, &b).unwrap_err().kind(), "already_exists");
        // Neither file was touched.
        assert_eq!(std::fs::read(&a).unwrap(), b"A");
        assert_eq!(std::fs::read(&b).unwrap(), b"B");

        let c = s(d.path().join("c.txt"));
        imp::rename(&a, &c).unwrap();
        assert_eq!(std::fs::read(&c).unwrap(), b"A");
        assert!(!Path::new(&a).exists());
    }

    /// A case-only rename looks like "the destination exists" on a
    /// case-insensitive volume, and must still go through (rule 15's
    /// `same_file` is what tells the two apart).
    #[test]
    fn a_case_only_rename_is_allowed() {
        let d = tmp();
        let lower = s(d.path().join("readme.md"));
        std::fs::write(&lower, b"x").unwrap();
        let upper = s(d.path().join("README.md"));
        imp::rename(&lower, &upper).unwrap();

        // `exists()` is useless here — on a case-insensitive volume both
        // spellings resolve either way. Ask the directory what the name
        // actually is.
        let listed = imp::list_dir(&s(d.path().to_path_buf()), false).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "README.md");
    }

    /// A Windows **junction** is the case that matters in practice — it
    /// needs no privilege to create, unlike a symlink, so it is what users
    /// and tools actually leave lying around. `mklink /J` is a `cmd`
    /// builtin, so this is the one place the test suite shells out.
    #[cfg(windows)]
    #[test]
    fn deleting_a_junction_spares_its_target() {
        let d = tmp();
        let target = d.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("precious.txt"), b"keep me").unwrap();
        let link = d.path().join("junction");

        let out = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .expect("run mklink");
        assert!(
            out.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let info = imp::stat(&s(link.clone())).unwrap();
        assert!(info.is_symlink, "a junction is a reparse point");
        assert!(info.is_dir, "...whose target is a directory");

        imp::delete(std::slice::from_ref(&s(link.clone())), false).unwrap();
        assert!(!link.exists(), "the junction is gone");
        assert!(
            target.join("precious.txt").exists(),
            "the target must survive"
        );
    }

    #[test]
    fn copy_and_move_a_tree() {
        let d = tmp();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("deep")).unwrap();
        std::fs::write(src.join("deep/f.txt"), "안녕\r\n").unwrap();

        let dst = s(d.path().join("copy"));
        imp::copy(&s(src.clone()), &dst, false).unwrap();
        assert_eq!(
            std::fs::read(Path::new(&dst).join("deep/f.txt")).unwrap(),
            "안녕\r\n".as_bytes()
        );
        // The source survives a copy.
        assert!(src.join("deep/f.txt").exists());

        // Copying onto it again needs `overwrite`.
        assert_eq!(
            imp::copy(&s(src.clone()), &dst, false).unwrap_err().kind(),
            "already_exists"
        );
        imp::copy(&s(src.clone()), &dst, true).unwrap();

        let moved = s(d.path().join("moved"));
        imp::move_path(&s(src.clone()), &moved, false).unwrap();
        assert!(!src.exists());
        assert!(Path::new(&moved).join("deep/f.txt").exists());
    }

    #[test]
    fn copy_into_own_subtree_is_refused() {
        let d = tmp();
        let src = d.path().join("proj");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.txt"), b"x").unwrap();

        let inside = s(src.join("backup"));
        assert!(imp::copy(&s(src.clone()), &inside, false).is_err());
        assert!(imp::move_path(&s(src.clone()), &inside, false).is_err());
        // And the sibling-prefix case is *not* refused: `projX` is not
        // inside `proj`.
        let sibling = s(d.path().join("projX"));
        imp::copy(&s(src), &sibling, false).unwrap();
    }

    #[test]
    fn delete_removes_files_and_trees() {
        let d = tmp();
        let f = s(d.path().join("a.txt"));
        std::fs::write(&f, b"x").unwrap();
        let tree = d.path().join("t");
        std::fs::create_dir_all(tree.join("x")).unwrap();
        std::fs::write(tree.join("x/y.txt"), b"y").unwrap();

        imp::delete(&[f.clone(), s(tree.clone())], false).unwrap();
        assert!(!Path::new(&f).exists());
        assert!(!tree.exists());

        // A missing path is reported, not ignored.
        assert_eq!(imp::delete(&[f], false).unwrap_err().kind(), "not_found");
    }

    /// The hazard: deleting a directory symlink must remove the *link*.
    /// Creating one needs a privilege on Windows, so the test skips rather
    /// than fails when it cannot set the link up.
    #[test]
    fn deleting_a_directory_symlink_spares_its_target() {
        let d = tmp();
        let target = d.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("precious.txt"), b"keep me").unwrap();
        let link = d.path().join("link");

        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&target, &link).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link).is_ok();

        if !made {
            eprintln!("skipped: cannot create a directory symlink here");
            return;
        }

        let info = imp::stat(&s(link.clone())).unwrap();
        assert!(info.is_symlink, "the row must say it is a link");
        assert!(info.is_dir, "...while is_dir describes the target");

        imp::delete(&[s(link.clone())], false).unwrap();
        assert!(!link.exists(), "the link is gone");
        assert!(
            target.join("precious.txt").exists(),
            "the target must survive"
        );
    }

    /// Following a link on copy would put the target's contents under the
    /// link's name, which is a silent change of meaning.
    #[test]
    fn copying_a_symlink_is_refused_rather_than_followed() {
        let d = tmp();
        let target = d.path().join("t.txt");
        std::fs::write(&target, b"secret").unwrap();
        let link = d.path().join("link.txt");

        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link).is_ok();
        if !made {
            eprintln!("skipped: cannot create a file symlink here");
            return;
        }

        assert!(imp::copy(&s(link), &s(d.path().join("out.txt")), false).is_err());
    }

    #[test]
    fn read_and_write_preserve_crlf_bom_and_the_trailing_newline() {
        let d = tmp();
        let p = s(d.path().join("w.txt"));
        let mut raw = textfile::UTF8_BOM.to_vec();
        raw.extend_from_slice("안녕\r\n하세요\r\n".as_bytes());
        std::fs::write(&p, &raw).unwrap();

        let f = imp::read_text(&p).unwrap();
        assert!(f.bom);
        assert_eq!(f.eol, Eol::Crlf);
        assert_eq!(f.text, "안녕\n하세요\n");
        assert!(!f.truncated);

        // Saving it untouched must be a no-op diff.
        imp::write_text(&WriteTextArgs {
            path: p.clone(),
            text: f.text.clone(),
            eol: f.eol,
            bom: f.bom,
            expect: Some(f.stamp.clone()),
        })
        .unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), raw);
    }

    /// The agent case (spec §3.6): the file changed under the buffer, so the
    /// write must refuse rather than win.
    #[test]
    fn a_stamped_write_refuses_on_a_stale_stamp_and_succeeds_on_a_fresh_one() {
        let d = tmp();
        let p = s(d.path().join("agent.txt"));
        std::fs::write(&p, b"original\n").unwrap();
        let read = imp::read_text(&p).unwrap();

        // Claude rewrites it underneath us.
        std::fs::write(&p, b"rewritten by an agent\n").unwrap();

        let stale = imp::write_text(&WriteTextArgs {
            path: p.clone(),
            text: "my edit\n".into(),
            eol: Eol::Lf,
            bom: false,
            expect: Some(read.stamp.clone()),
        })
        .unwrap_err();
        assert_eq!(stale.kind(), "conflict");
        // The agent's content is untouched.
        assert_eq!(std::fs::read(&p).unwrap(), b"rewritten by an agent\n");

        // Re-read, then the same write lands.
        let fresh = imp::read_text(&p).unwrap();
        let stamp = imp::write_text(&WriteTextArgs {
            path: p.clone(),
            text: "my edit\n".into(),
            eol: Eol::Lf,
            bom: false,
            expect: Some(fresh.stamp),
        })
        .unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"my edit\n");
        assert_eq!(stamp.sha256, textfile::sha256_hex(b"my edit\n"));

        // A file that was deleted under an expectation is also a conflict.
        std::fs::remove_file(&p).unwrap();
        let gone = imp::write_text(&WriteTextArgs {
            path: p.clone(),
            text: "x".into(),
            eol: Eol::Lf,
            bom: false,
            expect: Some(stamp),
        })
        .unwrap_err();
        assert_eq!(gone.kind(), "conflict");

        // With no expectation it is a plain create.
        imp::write_text(&WriteTextArgs {
            path: p.clone(),
            text: "x".into(),
            eol: Eol::Lf,
            bom: false,
            expect: None,
        })
        .unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"x");
    }

    /// The other side of the conflict rule: a rewrite with identical bytes
    /// moves the mtime but is not a conflict, or an agent that reformatted
    /// and changed nothing would make the editor cry wolf.
    #[test]
    fn an_identical_rewrite_is_not_a_conflict() {
        let d = tmp();
        let p = s(d.path().join("touched.txt"));
        std::fs::write(&p, b"same\n").unwrap();
        let read = imp::read_text(&p).unwrap();

        // Rewrite the very same bytes; the mtime moves, the hash does not.
        std::fs::write(&p, b"same\n").unwrap();

        imp::write_text(&WriteTextArgs {
            path: p.clone(),
            text: "my edit\n".into(),
            eol: Eol::Lf,
            bom: false,
            expect: Some(read.stamp),
        })
        .expect("an identical rewrite must not block the save");
        assert_eq!(std::fs::read(&p).unwrap(), b"my edit\n");
    }

    #[test]
    fn a_binary_file_is_refused_not_loaded_as_text() {
        let d = tmp();
        let p = s(d.path().join("a.bin"));
        std::fs::write(&p, [0x50, 0x4b, 0x03, 0x04, 0x00, 0x01]).unwrap();
        let err = imp::read_text(&p).unwrap_err();
        assert_eq!(err.kind(), "not_utf8");
        assert!(err.to_string().contains("binary"));
    }

    #[test]
    fn reading_a_directory_is_an_error_not_a_panic() {
        let d = tmp();
        assert!(imp::read_text(&s(d.path().to_path_buf())).is_err());
    }

    /// `to_trash` on a real file. Trash is unavailable in some CI sandboxes,
    /// so a failure to reach it skips rather than fails.
    #[test]
    fn delete_to_trash_removes_the_file() {
        let d = tmp();
        let p = s(d.path().join("trashme.txt"));
        std::fs::write(&p, b"bye").unwrap();
        match imp::delete(std::slice::from_ref(&p), true) {
            Ok(()) => assert!(!Path::new(&p).exists()),
            Err(e) => eprintln!("skipped: no trash available here ({e})"),
        }
    }

    #[test]
    fn read_head_is_capped_and_returns_raw_bytes() {
        let d = tmp();
        let p = s(d.path().join("big.bin"));
        let mut data = vec![b'a'; MAX_HEAD_BYTES * 2];
        data[10] = 0; // binary content is returned, not refused
        std::fs::write(&p, &data).unwrap();

        let head = imp::read_head(&p, usize::MAX).unwrap();
        assert_eq!(head.len(), MAX_HEAD_BYTES);
        assert_eq!(head[10], 0);
        assert_eq!(imp::read_head(&p, 16).unwrap().len(), 16);

        let small = s(d.path().join("한글.txt"));
        std::fs::write(&small, "안녕".as_bytes()).unwrap();
        assert_eq!(imp::read_head(&small, 1024).unwrap(), "안녕".as_bytes());

        assert_eq!(
            imp::read_head(&s(d.path().join("nope")), 16)
                .unwrap_err()
                .kind(),
            "not_found"
        );
        assert!(imp::read_head(&s(d.path().to_path_buf()), 16).is_err());
    }

    #[test]
    fn peek_dir_is_bounded_and_honours_hidden() {
        let d = tmp();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join(".hidden"), b"").unwrap();
        std::fs::write(d.path().join("a.txt"), b"").unwrap();
        let dir = s(d.path().to_path_buf());

        let mut peek = imp::peek_dir(&dir, false).unwrap();
        peek.entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert!(!peek.more);
        assert_eq!(
            peek.entries,
            vec![
                PeekEntry {
                    name: "a.txt".into(),
                    is_dir: false
                },
                PeekEntry {
                    name: "sub".into(),
                    is_dir: true
                },
            ]
        );
        assert_eq!(imp::peek_dir(&dir, true).unwrap().entries.len(), 3);

        let many = d.path().join("many");
        std::fs::create_dir(&many).unwrap();
        for i in 0..MAX_PEEK_SCAN + 3 {
            std::fs::write(many.join(format!("f{i}")), b"").unwrap();
        }
        let peek = imp::peek_dir(&s(many), false).unwrap();
        assert!(peek.more);
        assert_eq!(peek.entries.len(), MAX_PEEK_SCAN);
    }

    #[test]
    fn roots_and_home_are_real_directories() {
        let roots = imp::roots();
        assert!(!roots.is_empty());
        for r in &roots {
            assert!(Path::new(r).is_dir(), "{r}");
        }
        let home = imp::home_dir().unwrap();
        assert!(Path::new(&home).is_dir());
    }

    /// Nothing on this surface may launch a program. `open_default` is the
    /// only command that reaches the OS handler, and an executable goes to
    /// the file manager instead.
    #[test]
    fn open_default_reveals_an_executable_rather_than_running_it() {
        let d = tmp();
        for name in ["payload.bat", "payload.exe", "payload.ps1", "shortcut.lnk"] {
            let p = d.path().join(name);
            std::fs::write(&p, b"echo pwned").unwrap();
            assert!(
                crate::fspath::should_reveal(&p, false),
                "{name} must be revealed, never opened"
            );
        }
        // A relative path never reaches the opener at all.
        assert!(imp::open_default("relative/x.txt").is_err());
        assert!(imp::reveal("--version").is_err());
    }
}
