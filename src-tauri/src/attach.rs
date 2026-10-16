//! Which files may travel with a webhook delivery.
//!
//! [`eligible`] is the only gate: every candidate (a path the turn touched, or
//! a file dropped into the per-repo report folder) is checked against the
//! repo root on its *canonical* spelling (CLAUDE.md rule 15: containment goes
//! through [`fsx::is_within`], never `starts_with`), must be a regular file
//! reached without a symlink, must not match the deny list, must have an
//! allowed extension whose magic bytes agree, and must fit the caps. Anything
//! that fails becomes a [`ListedFile`] (name, size, reason) and the delivery
//! carries on.
//!
//! Every file is opened exactly once per check, with the leaf *not followed*
//! (unix `O_NOFOLLOW`, Windows `FILE_FLAG_OPEN_REPARSE_POINT`), and everything
//! after that is decided from the open handle: it must be a regular file with
//! one link (a hard link could alias a protected file under an innocent name),
//! the same file `canonicalize` saw, and it is sniffed from that handle. The
//! planner records the file's identity (device + inode, or volume serial +
//! file index), size and mtime in the [`AttachmentRef`]. Delivery does not
//! reopen by path: it calls [`open_verified`] at send time, which repeats the
//! no-follow open, rejects any change of identity, size, mtime or link count,
//! and returns the bytes read from that verified handle. A swap between the
//! plan and the send therefore costs a listed file, never a different file's
//! content. Text kinds are additionally refused when they contain a
//! high-confidence secret (PEM, vendor token, JWT, `key=value`, ...).
//!
//! A handle proves *which file* it is, not *where* it lives: a parent
//! directory swapped for a link between `canonicalize` and the open would
//! still hand over a regular file. So after every open the real path is read
//! back from the handle ([`handle_path`]) and judged again: at plan time it
//! must be inside the canonical root and clear of the deny list; at send time
//! it must be the very path the planner approved, and clear of the deny list
//! again. If the handle's path cannot be read the file is refused.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::activity::redact::contains_high_confidence_secret;
use crate::fsx::is_within;
use crate::webhooks::{AttachmentRef, ListedFile};

/// Bytes read for the magic-byte / UTF-8 sniff.
const SNIFF_BYTES: u64 = 64 * 1024;
/// Clock slack allowed past the turn end for report-folder files.
const REPORT_SLACK_MS: u64 = 2000;
/// Report-folder entries looked at (a runaway folder cannot stall a turn end).
const MAX_SCANNED: usize = 1000;

/// Backend-supplied context. `root` comes from backend state (the pane's
/// repo/worktree), never from a hook claim.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub root: PathBuf,
    /// Relative to `root`, no `..` (e.g. `.ymux/report`). Empty = no folder.
    pub report_dir: String,
    pub turn_start_ms: u64,
    /// Unix ms the turn ended; 0 = unknown (no upper bound). Report-folder
    /// files newer than this plus `REPORT_SLACK_MS` belong to a later turn.
    pub turn_end_ms: u64,
    /// Lowercase extensions without the dot; empty allows nothing.
    pub allow_types: Vec<String>,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_files: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Pdf,
    Mp3,
    Wav,
    Ogg,
    Mp4,
    Webm,
    Zip,
    Text,
}

/// extension -> (mime, sniff kind)
fn type_of(ext: &str) -> Option<(&'static str, Kind)> {
    Some(match ext {
        "png" => ("image/png", Kind::Png),
        "jpg" | "jpeg" => ("image/jpeg", Kind::Jpeg),
        "gif" => ("image/gif", Kind::Gif),
        "webp" => ("image/webp", Kind::Webp),
        "pdf" => ("application/pdf", Kind::Pdf),
        "mp3" => ("audio/mpeg", Kind::Mp3),
        "wav" => ("audio/wav", Kind::Wav),
        "ogg" | "oga" | "opus" => ("audio/ogg", Kind::Ogg),
        "mp4" | "m4a" => ("video/mp4", Kind::Mp4),
        "webm" => ("video/webm", Kind::Webm),
        "docx" => (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Kind::Zip,
        ),
        "xlsx" => (
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Kind::Zip,
        ),
        "pptx" => (
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            Kind::Zip,
        ),
        "md" => ("text/markdown", Kind::Text),
        "txt" => ("text/plain", Kind::Text),
        "csv" => ("text/csv", Kind::Text),
        _ => return None,
    })
}

fn sniff_ok(kind: Kind, b: &[u8]) -> bool {
    match kind {
        Kind::Png => b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        Kind::Jpeg => b.starts_with(&[0xFF, 0xD8, 0xFF]),
        Kind::Gif => b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a"),
        Kind::Webp => b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP"),
        Kind::Pdf => b.starts_with(b"%PDF-"),
        Kind::Mp3 => {
            b.starts_with(b"ID3")
                || (b.first() == Some(&0xFF) && b.get(1).is_some_and(|x| x & 0xE0 == 0xE0))
        }
        Kind::Wav => b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WAVE"),
        Kind::Ogg => b.starts_with(b"OggS"),
        Kind::Mp4 => b.get(4..8) == Some(b"ftyp"),
        Kind::Webm => b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]),
        Kind::Zip => b.starts_with(b"PK\x03\x04"),
        Kind::Text => is_text(b),
    }
}

/// UTF-8 without NULs. The sniff window may cut a character in half, which is
/// not an error.
fn is_text(b: &[u8]) -> bool {
    if b.contains(&0) {
        return false;
    }
    match std::str::from_utf8(b) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    }
}

/// Case-insensitive `*` / `?` glob over the whole text.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some(pi);
                mark = ti;
                pi += 1;
            }
            Some(&c) if c == '?' || Some(&c) == t.get(ti) => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some(s) => {
                    mark += 1;
                    ti = mark;
                    pi = s + 1;
                }
                None => return false,
            },
        }
    }
    while p.get(pi) == Some(&'*') {
        pi += 1;
    }
    pi == p.len()
}

const DENY_NAMES: [&str; 17] = [
    ".env*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa*",
    "id_ed25519*",
    "*.kdbx",
    ".npmrc",
    ".pypirc",
    ".netrc",
    "*credentials*",
    "*secret*",
    "*.keystore",
    "*.jks",
    "*.ppk",
    "*.gpg",
];
const DENY_DIRS: [&str; 3] = [".git", ".ssh", ".aws"];

/// Why a canonical path is on the deny list, if it is.
fn denied(canon: &Path, name: &str) -> Option<&'static str> {
    if DENY_NAMES.iter().any(|g| glob_match(g, name)) {
        return Some("sensitive file name");
    }
    let in_denied_dir = canon.components().any(|c| match c {
        Component::Normal(s) => {
            let s = s.to_string_lossy();
            DENY_DIRS.iter().any(|d| s.eq_ignore_ascii_case(d))
        }
        _ => false,
    });
    in_denied_dir.then_some("inside a protected folder")
}

/// A relative path with only normal components.
fn clean_relative(rel: &str) -> bool {
    let p = Path::new(rel);
    !rel.is_empty()
        && p.components().all(|c| matches!(c, Component::Normal(_)))
        && !rel.contains("..")
}

fn mtime_ms(md: &std::fs::Metadata) -> Option<u64> {
    let d = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    u64::try_from(d.as_millis()).ok()
}

fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// What an open handle says about its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Info {
    dev: u64,
    ino: u64,
    nlink: u64,
    len: u64,
    mtime_ms: u64,
}

/// Opens `p` read-only without following a link at the leaf. `lock` (Windows
/// only) denies every other opener write and delete access while the handle
/// lives, so a send-time check cannot be raced by an in-place rewrite.
fn open_nofollow(p: &Path, lock: bool) -> std::io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.custom_flags(windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT.0);
        if lock {
            o.share_mode(windows::Win32::Storage::FileSystem::FILE_SHARE_READ.0);
        }
    }
    #[cfg(not(windows))]
    let _ = lock;
    o.open(p)
}

#[cfg(unix)]
fn info_of(f: &File) -> std::io::Result<Info> {
    use std::os::unix::fs::MetadataExt;
    let md = f.metadata()?;
    if !md.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(Info {
        dev: md.dev(),
        ino: md.ino(),
        nlink: md.nlink(),
        len: md.len(),
        mtime_ms: mtime_ms(&md).unwrap_or(0),
    })
}

#[cfg(windows)]
fn info_of(f: &File) -> std::io::Result<Info> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT,
    };
    let md = f.metadata()?;
    let mut bi = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is owned by `f` for the whole call and `bi` is a
    // valid, writable out-parameter.
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle() as isize), &mut bi) }
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if bi.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_DIRECTORY.0) != 0 {
        return Err(std::io::Error::other("not a plain regular file"));
    }
    Ok(Info {
        dev: u64::from(bi.dwVolumeSerialNumber),
        ino: (u64::from(bi.nFileIndexHigh) << 32) | u64::from(bi.nFileIndexLow),
        nlink: u64::from(bi.nNumberOfLinks),
        len: md.len(),
        mtime_ms: mtime_ms(&md).unwrap_or(0),
    })
}

/// The no-follow open plus the handle checks every path in this module shares.
fn open_checked(p: &Path, lock: bool) -> Result<(File, Info), &'static str> {
    let f = open_nofollow(p, lock).map_err(|_| "unreadable")?;
    let info = info_of(&f).map_err(|_| "not a regular file")?;
    if info.nlink > 1 {
        return Err("hard-linked files are not attached");
    }
    Ok((f, info))
}

/// Is the handle the file `canonicalize` + `symlink_metadata` described?
#[cfg(unix)]
fn same_as_leaf(info: &Info, canon: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(canon)
        .is_ok_and(|leaf| info.dev == leaf.dev() && info.ino == leaf.ino())
}

/// `std` exposes no file index for `Metadata` on Windows, so the leaf's
/// identity is read the same way the handle's is: a second no-follow open of
/// the canonical path and `GetFileInformationByHandle` (volume serial + file
/// index), compared with the first handle's.
#[cfg(windows)]
fn same_as_leaf(info: &Info, canon: &Path) -> bool {
    open_nofollow(canon, false)
        .and_then(|f| info_of(&f))
        .is_ok_and(|other| (info.dev, info.ino) == (other.dev, other.ino))
}

/// `\\?\C:\x` -> `C:\x`, `\\?\UNC\h\s` -> `\\h\s`. Other spellings pass
/// through. (A POSIX absolute path starts with `/`, so it is never touched.)
fn plain_path(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

/// The real path of an open handle, as the OS reports it now (not the path it
/// was opened by).
#[cfg(windows)]
fn handle_path(f: &File) -> std::io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
    let h = HANDLE(f.as_raw_handle() as isize);
    let mut buf = vec![0u16; 1024];
    for _ in 0..2 {
        // SAFETY: `h` is owned by `f` for the whole call and `buf` is a valid,
        // writable slice the API fills up to its length.
        let n = unsafe { GetFinalPathNameByHandleW(h, &mut buf, FILE_NAME_NORMALIZED) } as usize;
        if n == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n < buf.len() {
            return Ok(plain_path(&String::from_utf16_lossy(&buf[..n])));
        }
        // Too small: `n` is the required length including the NUL.
        buf = vec![0u16; n.saturating_add(1)];
    }
    Err(std::io::Error::other("path length kept changing"))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn handle_path(f: &File) -> std::io::Result<String> {
    use std::os::unix::io::AsRawFd;
    let p = std::fs::read_link(format!("/proc/self/fd/{}", f.as_raw_fd()))?;
    Ok(p.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn handle_path(f: &File) -> std::io::Result<String> {
    use std::os::unix::io::AsRawFd;
    let mut buf = [0 as libc::c_char; libc::PATH_MAX as usize];
    // SAFETY: `F_GETPATH` writes a NUL-terminated path of at most MAXPATHLEN
    // bytes into `buf`, which is that large; the fd is owned by `f`.
    let rc = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if rc == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the buffer was NUL-terminated by the kernel on success.
    let c = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Ok(c.to_string_lossy().into_owned())
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "macos"
)))]
fn handle_path(_: &File) -> std::io::Result<String> {
    Err(std::io::Error::other("no handle path on this platform"))
}

/// The deny list, on the path the handle really lives at.
fn deny_check(real: &str) -> Result<(), &'static str> {
    let p = Path::new(real);
    match denied(p, &display_name(p)) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// Plan time: the handle's real path must be inside the canonical root and
/// clear of the deny list.
fn check_within_root(root: &str, real: &str) -> Result<(), &'static str> {
    if !is_within(&plain_path(root), &plain_path(real)) {
        return Err("outside the repository");
    }
    deny_check(real)
}

/// Send time: the handle's real path must be the canonical path the planner
/// approved (which it proved inside the root), and clear of the deny list.
fn check_planned_path(planned: &str, real: &str) -> Result<(), &'static str> {
    if !ypath::same_path(&plain_path(planned), &plain_path(real)) {
        return Err("changed");
    }
    deny_check(real)
}

fn read_head(f: &File) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    f.take(SNIFF_BYTES).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/")
}

/// Opens the file a planned attachment names and checks it is still the same
/// one: no-follow, regular, single link, identical identity, size and mtime.
fn open_matching(want: &AttachmentRef) -> Result<(File, Info), &'static str> {
    let (f, info) = open_checked(Path::new(&want.path), true)?;
    if (info.dev, info.ino) != (want.dev, want.ino)
        || info.len != want.size
        || info.mtime_ms != want.mtime_ms
    {
        return Err("changed");
    }
    // The identity matched, but a swapped parent directory would still lead
    // to the same file by another route: ask the handle where it really is.
    let real = handle_path(&f).map_err(|_| "unreadable")?;
    check_planned_path(&want.path, &real)?;
    Ok((f, info))
}

/// Checks (without reading the content) that `want` is still the file the
/// planner approved.
pub fn verify(want: &AttachmentRef) -> Result<(), &'static str> {
    open_matching(want).map(|_| ())
}

/// The bytes of a planned attachment, read from a freshly opened and verified
/// handle. `Err` carries the reason to list the file with instead.
pub fn open_verified(want: &AttachmentRef) -> Result<Vec<u8>, &'static str> {
    let (f, _) = open_matching(want)?;
    let mut data = Vec::new();
    (&f).take(want.size.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|_| "unreadable")?;
    if data.len() as u64 != want.size {
        return Err("changed");
    }
    // Rewritten while we read?
    match info_of(&f) {
        Ok(after) if after.len == want.size && after.mtime_ms == want.mtime_ms => {}
        _ => return Err("changed"),
    }
    // The type sniff again, on the bytes actually being sent.
    let ext = Path::new(&want.name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let Some((mime, kind)) = type_of(&ext) else {
        return Err("file type not allowed");
    };
    let head = &data[..data.len().min(SNIFF_BYTES as usize)];
    if mime != want.mime || !sniff_ok(kind, head) {
        return Err("content does not match its file type");
    }
    if is_text_mime(mime) {
        let Ok(text) = std::str::from_utf8(&data) else {
            return Err("content does not match its file type");
        };
        if contains_high_confidence_secret(text) {
            return Err("contains secrets");
        }
    }
    Ok(data)
}

/// Files directly inside `<root>/<report_dir>` modified at or after the turn
/// started. Non-recursive; a symlinked folder or entry is skipped.
fn report_candidates(root: &Path, ctx: &Ctx, listed: &mut Vec<ListedFile>) -> Vec<PathBuf> {
    if ctx.report_dir.is_empty() {
        return Vec::new();
    }
    if !clean_relative(&ctx.report_dir) {
        listed.push(ListedFile {
            name: ctx.report_dir.clone(),
            size: 0,
            reason: "report folder must be a relative path without '..'".into(),
        });
        return Vec::new();
    }
    let dir = root.join(&ctx.report_dir);
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => {}
        _ => return Vec::new(),
    }
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .take(MAX_SCANNED)
        .filter_map(|e| {
            let md = std::fs::symlink_metadata(e.path()).ok()?;
            (md.is_file()
                && mtime_ms(&md).is_some_and(|t| {
                    t >= ctx.turn_start_ms
                        && (ctx.turn_end_ms == 0
                            || t <= ctx.turn_end_ms.saturating_add(REPORT_SLACK_MS))
                }))
            .then(|| e.path())
        })
        .collect();
    out.sort();
    out
}

/// Split `candidates` (plus the report folder's fresh files) into what may be
/// attached and what is only listed. Never panics; never touches the network.
pub fn eligible(candidates: &[String], ctx: &Ctx) -> (Vec<AttachmentRef>, Vec<ListedFile>) {
    let mut attach = Vec::new();
    let mut listed = Vec::new();

    let Ok(root) = std::fs::canonicalize(&ctx.root) else {
        for c in candidates {
            listed.push(ListedFile {
                name: display_name(Path::new(c)),
                size: 0,
                reason: "repository root unavailable".into(),
            });
        }
        return (attach, listed);
    };
    let root_s = root.to_string_lossy().into_owned();

    let mut all: Vec<PathBuf> = candidates
        .iter()
        .map(|c| {
            let p = PathBuf::from(c);
            if p.is_absolute() {
                p
            } else {
                root.join(p)
            }
        })
        .collect();
    all.extend(report_candidates(&root, ctx, &mut listed));

    let allow: HashSet<String> = ctx
        .allow_types
        .iter()
        .map(|t| t.trim_start_matches('.').to_lowercase())
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut total: u64 = 0;

    for orig in all {
        let name = display_name(&orig);
        let mut skip = |size: u64, reason: &str| {
            listed.push(ListedFile {
                name: name.clone(),
                size,
                reason: reason.to_string(),
            });
        };

        // The path as given: it must exist and must not be a link.
        let Ok(orig_md) = std::fs::symlink_metadata(&orig) else {
            skip(0, "file not found");
            continue;
        };
        if orig_md.file_type().is_symlink() {
            skip(0, "symbolic links are not attached");
            continue;
        }
        let Ok(canon) = std::fs::canonicalize(&orig) else {
            skip(0, "file not found");
            continue;
        };
        let Ok(leaf) = std::fs::symlink_metadata(&canon) else {
            skip(0, "file not found");
            continue;
        };
        if leaf.file_type().is_symlink() {
            skip(0, "symbolic links are not attached");
            continue;
        }
        let canon_s = canon.to_string_lossy().into_owned();
        if !is_within(&root_s, &canon_s) {
            skip(0, "outside the repository");
            continue;
        }
        if !leaf.is_file() {
            skip(0, "not a regular file");
            continue;
        }
        if !seen.insert(ypath::comparison_key(&canon_s)) {
            continue;
        }
        let size = leaf.len();
        let cname = display_name(&canon);
        if let Some(why) = denied(&canon, &cname) {
            skip(size, why);
            continue;
        }
        let ext = Path::new(&cname)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let Some((mime, kind)) = type_of(&ext).filter(|_| allow.contains(&ext)) else {
            skip(size, "file type not allowed");
            continue;
        };
        let (file, info) = match open_checked(&canon, false) {
            Ok(x) => x,
            Err(why) => {
                skip(size, why);
                continue;
            }
        };
        if !same_as_leaf(&info, &canon) {
            skip(size, "changed");
            continue;
        }
        // Where the handle really lives, not where it was opened by: a parent
        // directory swapped after `canonicalize` ends up here.
        let Ok(real) = handle_path(&file) else {
            skip(size, "unreadable");
            continue;
        };
        if let Err(why) = check_within_root(&root_s, &real) {
            skip(size, why);
            continue;
        }
        let size = info.len;
        match read_head(&file) {
            Some(head) if sniff_ok(kind, &head) => {
                let text = String::from_utf8_lossy(&head);
                if is_text_mime(mime) && contains_high_confidence_secret(&text) {
                    skip(size, "contains secrets");
                    continue;
                }
            }
            Some(_) => {
                skip(size, "content does not match its file type");
                continue;
            }
            None => {
                skip(size, "unreadable");
                continue;
            }
        }
        if size > ctx.max_file_bytes {
            skip(size, "exceeds the per-file size limit");
            continue;
        }
        if attach.len() >= ctx.max_files {
            skip(size, "too many files");
            continue;
        }
        if total.saturating_add(size) > ctx.max_total_bytes {
            skip(size, "exceeds the total size limit");
            continue;
        }
        total += size;
        attach.push(AttachmentRef {
            name: cname,
            size,
            mime: mime.to_string(),
            path: canon_s,
            dev: info.dev,
            ino: info.ino,
            mtime_ms: info.mtime_ms,
        });
    }
    (attach, listed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    fn ctx(root: &Path) -> Ctx {
        Ctx {
            root: root.to_path_buf(),
            report_dir: ".ymux/report".into(),
            turn_start_ms: 0,
            turn_end_ms: 0,
            allow_types: ["png", "pdf", "txt", "md", "csv", "key", "env", "zip"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            max_file_bytes: 1024,
            max_total_bytes: 4096,
            max_files: 10,
        }
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn reasons(l: &[ListedFile]) -> Vec<(&str, &str)> {
        l.iter()
            .map(|f| (f.name.as_str(), f.reason.as_str()))
            .collect()
    }

    #[test]
    fn glob_matcher() {
        assert!(glob_match(".env*", ".env"));
        assert!(glob_match(".env*", ".ENV.local"));
        assert!(!glob_match(".env*", "my.env"));
        assert!(glob_match("*.pem", "a.PEM"));
        assert!(!glob_match("*.pem", "a.pem.txt"));
        assert!(glob_match("*secret*", "my-Secrets.txt"));
        assert!(glob_match("id_rsa*", "id_rsa.pub"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
    }

    #[test]
    fn attaches_valid_png_and_text() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("shot.png");
        let b = d.path().join("notes.txt");
        fs::write(&a, PNG).unwrap();
        fs::write(&b, "héllo").unwrap();
        let (ok, no) = eligible(&[s(&a), s(&b), s(&a)], &ctx(d.path()));
        assert!(no.is_empty(), "{no:?}");
        assert_eq!(ok.len(), 2, "duplicate must be deduped");
        assert_eq!(ok[0].mime, "image/png");
        assert_eq!(ok[0].size, PNG.len() as u64);
        assert_eq!(ok[1].mime, "text/plain");
    }

    #[test]
    fn relative_candidate_resolves_against_root() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("a.png"), PNG).unwrap();
        let (ok, no) = eligible(&["a.png".into()], &ctx(d.path()));
        assert_eq!((ok.len(), no.len()), (1, 0));
    }

    #[test]
    fn dotdot_traversal_is_outside() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        fs::create_dir(&root).unwrap();
        fs::write(d.path().join("outside.png"), PNG).unwrap();
        let (ok, no) = eligible(&["../outside.png".into()], &ctx(&root));
        assert!(ok.is_empty());
        assert_eq!(
            reasons(&no),
            vec![("outside.png", "outside the repository")]
        );
    }

    #[test]
    fn sibling_prefix_is_not_inside() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        let evil = d.path().join("repo-evil");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&evil).unwrap();
        let f = evil.join("a.png");
        fs::write(&f, PNG).unwrap();
        let (ok, no) = eligible(&[s(&f)], &ctx(&root));
        assert!(ok.is_empty());
        assert_eq!(no.len(), 1);
    }

    #[test]
    fn invalid_report_dir_is_listed_not_scanned() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        fs::create_dir(&root).unwrap();
        fs::write(d.path().join("x.png"), PNG).unwrap();
        for bad in ["../", "..", "a/../..", "/abs"] {
            let mut c = ctx(&root);
            c.report_dir = bad.into();
            let (ok, no) = eligible(&[], &c);
            assert!(ok.is_empty(), "{bad}");
            assert_eq!(no.len(), 1, "{bad}");
        }
    }

    #[test]
    fn report_dir_picks_fresh_files_only() {
        let d = tempfile::tempdir().unwrap();
        let rep = d.path().join(".ymux").join("report");
        fs::create_dir_all(&rep).unwrap();
        fs::write(rep.join("new.png"), PNG).unwrap();
        fs::write(rep.join("sub.txt"), "x").unwrap();
        fs::create_dir(rep.join("nested")).unwrap();
        let mut c = ctx(d.path());
        let (ok, _) = eligible(&[], &c);
        assert_eq!(ok.len(), 2);
        // A turn that started in the far future saw none of them.
        c.turn_start_ms = u64::MAX / 2;
        let (ok, no) = eligible(&[], &c);
        assert!(ok.is_empty() && no.is_empty());
    }

    #[test]
    fn report_dir_skips_files_newer_than_turn_end() {
        let d = tempfile::tempdir().unwrap();
        let rep = d.path().join(".ymux").join("report");
        fs::create_dir_all(&rep).unwrap();
        fs::write(rep.join("a.txt"), "x").unwrap();
        let mut c = ctx(d.path());
        // The turn ended long before the file was written.
        c.turn_end_ms = 1000;
        let (ok, _) = eligible(&[], &c);
        assert!(ok.is_empty());
        // Ended just now: kept.
        c.turn_end_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();
        let (ok, _) = eligible(&[], &c);
        assert_eq!(ok.len(), 1);
    }

    #[test]
    fn deny_globs_and_dirs() {
        let d = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for n in [
            ".env",
            ".env.local",
            "server.key",
            "x.pem",
            "my_credentials.txt",
            "top-secret.txt",
        ] {
            let p = d.path().join(n);
            fs::write(&p, "k").unwrap();
            paths.push(s(&p));
        }
        let git = d.path().join(".git");
        fs::create_dir(&git).unwrap();
        fs::write(git.join("note.txt"), "k").unwrap();
        paths.push(s(&git.join("note.txt")));
        let ssh = d.path().join("sub").join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(ssh.join("a.txt"), "k").unwrap();
        paths.push(s(&ssh.join("a.txt")));
        let (ok, no) = eligible(&paths, &ctx(d.path()));
        assert!(ok.is_empty(), "{ok:?}");
        assert_eq!(no.len(), paths.len());
        assert!(no
            .iter()
            .all(|f| f.reason == "sensitive file name" || f.reason == "inside a protected folder"));
    }

    #[test]
    fn type_allowlist_and_magic_sniff() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("run.exe");
        let fake = d.path().join("fake.png");
        let bin = d.path().join("bin.txt");
        let pdf = d.path().join("doc.pdf");
        let noext = d.path().join("README");
        fs::write(&exe, "MZ").unwrap();
        fs::write(&fake, "not a png").unwrap();
        fs::write(&bin, [0u8, 1, 2, 3]).unwrap();
        fs::write(&pdf, b"%PDF-1.7 ...").unwrap();
        fs::write(&noext, "hi").unwrap();
        let (ok, no) = eligible(
            &[s(&exe), s(&fake), s(&bin), s(&pdf), s(&noext)],
            &ctx(d.path()),
        );
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].name, "doc.pdf");
        assert_eq!(
            reasons(&no),
            vec![
                ("run.exe", "file type not allowed"),
                ("fake.png", "content does not match its file type"),
                ("bin.txt", "content does not match its file type"),
                ("README", "file type not allowed"),
            ]
        );
        // Empty allowlist allows nothing.
        let mut c = ctx(d.path());
        c.allow_types.clear();
        assert!(eligible(&[s(&pdf)], &c).0.is_empty());
    }

    #[test]
    fn sniff_table() {
        assert!(sniff_ok(Kind::Jpeg, &[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(sniff_ok(Kind::Gif, b"GIF89a.."));
        assert!(sniff_ok(Kind::Webp, b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!sniff_ok(Kind::Wav, b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(sniff_ok(Kind::Wav, b"RIFF\0\0\0\0WAVEfmt "));
        assert!(sniff_ok(Kind::Mp3, b"ID3\x04"));
        assert!(sniff_ok(Kind::Mp3, &[0xFF, 0xFB, 0x90]));
        assert!(sniff_ok(Kind::Ogg, b"OggS\0"));
        assert!(sniff_ok(Kind::Mp4, b"\0\0\0\x18ftypisom"));
        assert!(sniff_ok(Kind::Webm, &[0x1A, 0x45, 0xDF, 0xA3, 1]));
        assert!(sniff_ok(Kind::Zip, b"PK\x03\x04rest"));
        assert!(!sniff_ok(Kind::Png, b""));
        // A UTF-8 character cut at the window edge is still text.
        assert!(sniff_ok(Kind::Text, &"é".as_bytes()[..1]));
        assert!(!sniff_ok(Kind::Text, &[0xFF, 0xFE, b'a']));
    }

    #[test]
    fn caps_per_file_total_and_count() {
        let d = tempfile::tempdir().unwrap();
        let mut ps = Vec::new();
        for (n, len) in [
            ("a.txt", 600usize),
            ("b.txt", 600),
            ("c.txt", 2000),
            ("d.txt", 10),
        ] {
            let p = d.path().join(n);
            fs::write(&p, "ab ".repeat(len).chars().take(len).collect::<String>()).unwrap();
            ps.push(s(&p));
        }
        let mut c = ctx(d.path());
        c.max_total_bytes = 1000;
        let (ok, no) = eligible(&ps, &c);
        assert_eq!(
            ok.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            vec!["a.txt", "d.txt"]
        );
        assert_eq!(
            reasons(&no),
            vec![
                ("b.txt", "exceeds the total size limit"),
                ("c.txt", "exceeds the per-file size limit"),
            ]
        );
        c.max_total_bytes = u64::MAX;
        c.max_files = 1;
        let (ok, no) = eligible(&ps, &c);
        assert_eq!(ok.len(), 1);
        assert_eq!(
            no.iter().filter(|f| f.reason == "too many files").count(),
            2
        );
    }

    #[test]
    fn missing_file_and_directory_are_listed() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("dir.png")).unwrap();
        let (ok, no) = eligible(
            &[s(&d.path().join("gone.png")), s(&d.path().join("dir.png"))],
            &ctx(d.path()),
        );
        assert!(ok.is_empty());
        assert_eq!(
            reasons(&no),
            vec![
                ("gone.png", "file not found"),
                ("dir.png", "not a regular file")
            ]
        );
    }

    #[test]
    fn missing_root_lists_everything() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(&d.path().join("nope"));
        let (ok, no) = eligible(&["a.png".into()], &c);
        assert!(ok.is_empty());
        assert_eq!(no.len(), 1);
    }

    fn try_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            if target.is_dir() {
                std::os::windows::fs::symlink_dir(target, link).is_ok()
            } else {
                std::os::windows::fs::symlink_file(target, link).is_ok()
            }
        }
    }

    #[test]
    fn symlink_to_outside_file_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        fs::create_dir(&root).unwrap();
        let outside = d.path().join("outside.png");
        fs::write(&outside, PNG).unwrap();
        let link = root.join("link.png");
        if !try_symlink(&outside, &link) {
            eprintln!("skipped: cannot create symlinks here");
            return;
        }
        let (ok, no) = eligible(&[s(&link)], &ctx(&root));
        assert!(ok.is_empty());
        assert_eq!(no.len(), 1);
    }

    #[test]
    fn symlinked_directory_escape_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        let out = d.path().join("out");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&out).unwrap();
        fs::write(out.join("a.png"), PNG).unwrap();
        let link = root.join("dirlink");
        if !try_symlink(&out, &link) {
            eprintln!("skipped: cannot create symlinks here");
            return;
        }
        let (ok, no) = eligible(&[s(&link.join("a.png"))], &ctx(&root));
        assert!(ok.is_empty());
        assert_eq!(reasons(&no), vec![("a.png", "outside the repository")]);
    }

    #[test]
    fn symlinked_report_dir_is_not_scanned() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        let out = d.path().join("out");
        fs::create_dir_all(root.join(".ymux")).unwrap();
        fs::create_dir(&out).unwrap();
        fs::write(out.join("a.png"), PNG).unwrap();
        if !try_symlink(&out, &root.join(".ymux").join("report")) {
            eprintln!("skipped: cannot create symlinks here");
            return;
        }
        let (ok, no) = eligible(&[], &ctx(&root));
        assert!(ok.is_empty() && no.is_empty());
    }

    #[test]
    fn hard_linked_files_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.txt");
        let b = d.path().join("innocent.txt");
        fs::write(&a, "hello").unwrap();
        if fs::hard_link(&a, &b).is_err() {
            eprintln!("SKIPPED (check NOT exercised): fs::hard_link failed here");
            return;
        }
        let (ok, no) = eligible(&[s(&b)], &ctx(d.path()));
        assert!(ok.is_empty(), "{ok:?}");
        assert_eq!(
            reasons(&no),
            vec![("innocent.txt", "hard-linked files are not attached")]
        );
    }

    #[test]
    fn a_hard_link_made_after_planning_is_refused_at_send_time() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.txt");
        fs::write(&a, "hello").unwrap();
        let (ok, _) = eligible(&[s(&a)], &ctx(d.path()));
        assert_eq!(open_verified(&ok[0]).unwrap(), b"hello");
        if fs::hard_link(&a, d.path().join("alias.txt")).is_err() {
            eprintln!("SKIPPED (check NOT exercised): fs::hard_link failed here");
            return;
        }
        assert_eq!(
            open_verified(&ok[0]),
            Err("hard-linked files are not attached")
        );
    }

    #[test]
    fn a_swapped_file_is_detected_even_with_same_size_and_mtime() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.txt");
        fs::write(&a, "hello").unwrap();
        let (ok, _) = eligible(&[s(&a)], &ctx(d.path()));
        let planned = ok[0].clone();
        // A different file, same size, same mtime, moved over the path.
        let evil = d.path().join("evil.tmp");
        fs::write(&evil, "HELLO").unwrap();
        let t = fs::metadata(&a).unwrap().modified().unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&evil)
            .unwrap()
            .set_modified(t)
            .unwrap();
        fs::remove_file(&a).unwrap();
        fs::rename(&evil, &a).unwrap();
        assert_eq!(open_verified(&planned), Err("changed"));
        assert_eq!(verify(&planned), Err("changed"));
    }

    #[test]
    fn open_verified_returns_the_planned_bytes_and_detects_edits() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.png");
        fs::write(&a, PNG).unwrap();
        let (ok, _) = eligible(&[s(&a)], &ctx(d.path()));
        assert_eq!(open_verified(&ok[0]).unwrap(), PNG);
        fs::write(&a, [PNG, b"more"].concat()).unwrap();
        assert_eq!(open_verified(&ok[0]), Err("changed"));
        fs::remove_file(&a).unwrap();
        assert_eq!(open_verified(&ok[0]), Err("unreadable"));
    }

    #[test]
    fn text_with_a_secret_is_not_attached() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("notes.txt");
        fs::write(
            &a,
            "fine
password=hunter2
",
        )
        .unwrap();
        let (ok, no) = eligible(&[s(&a)], &ctx(d.path()));
        assert!(ok.is_empty());
        assert_eq!(reasons(&no), vec![("notes.txt", "contains secrets")]);
        // Binary kinds are not text-scanned.
        let p = d.path().join("shot.png");
        fs::write(&p, PNG).unwrap();
        assert_eq!(eligible(&[s(&p)], &ctx(d.path())).0.len(), 1);
    }

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(plain_path(r"\\?\C:\repo\a.png"), r"C:\repo\a.png");
        assert_eq!(plain_path(r"\\?\UNC\srv\share\a"), r"\\srv\share\a");
        assert_eq!(plain_path(r"C:\repo"), r"C:\repo");
        assert_eq!(plain_path("/srv/a"), "/srv/a");
    }

    #[test]
    fn a_handle_path_outside_the_root_is_refused() {
        let root = if cfg!(windows) { r"C:\repo" } else { "/repo" };
        let inside = if cfg!(windows) {
            r"C:\repo\sub\a.png"
        } else {
            "/repo/sub/a.png"
        };
        let outside = if cfg!(windows) {
            r"C:\elsewhere\a.png"
        } else {
            "/elsewhere/a.png"
        };
        let sibling = if cfg!(windows) {
            r"C:\repo-evil\a.png"
        } else {
            "/repo-evil/a.png"
        };
        assert_eq!(check_within_root(root, inside), Ok(()));
        assert_eq!(
            check_within_root(root, outside),
            Err("outside the repository")
        );
        assert_eq!(
            check_within_root(root, sibling),
            Err("outside the repository")
        );
        // Inside the root but on the deny list once the real path is known.
        let denied_name = if cfg!(windows) {
            r"C:\repo\.env"
        } else {
            "/repo/.env"
        };
        let denied_dir = if cfg!(windows) {
            r"C:\repo\.ssh\a.txt"
        } else {
            "/repo/.ssh/a.txt"
        };
        assert_eq!(
            check_within_root(root, denied_name),
            Err("sensitive file name")
        );
        assert_eq!(
            check_within_root(root, denied_dir),
            Err("inside a protected folder")
        );
        // The verbatim spelling Windows hands back is judged the same.
        if cfg!(windows) {
            assert_eq!(
                check_within_root(r"\\?\C:\repo", r"\\?\C:\repo\sub\a.png"),
                Ok(())
            );
            assert_eq!(
                check_within_root(r"\\?\C:\repo", r"C:\elsewhere\a.png"),
                Err("outside the repository")
            );
        }
    }

    #[test]
    fn send_time_requires_the_planned_path_and_the_deny_list() {
        let planned = if cfg!(windows) {
            r"C:\repo\a.txt"
        } else {
            "/repo/a.txt"
        };
        let moved = if cfg!(windows) {
            r"C:\other\a.txt"
        } else {
            "/other/a.txt"
        };
        let env = if cfg!(windows) {
            r"C:\repo\.env"
        } else {
            "/repo/.env"
        };
        assert_eq!(check_planned_path(planned, planned), Ok(()));
        assert_eq!(check_planned_path(planned, moved), Err("changed"));
        assert_eq!(check_planned_path(env, env), Err("sensitive file name"));
    }

    /// The platform's handle -> path call really works and agrees with
    /// `canonicalize` (every other test in this module depends on it, since an
    /// unreadable handle path refuses the file).
    #[test]
    fn handle_path_agrees_with_canonicalize() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("sub").join("a.txt");
        fs::create_dir(d.path().join("sub")).unwrap();
        fs::write(&a, "hello").unwrap();
        let canon = fs::canonicalize(&a).unwrap();
        let f = open_nofollow(&canon, false).unwrap();
        let real = handle_path(&f).expect("handle path");
        assert!(
            ypath::same_path(&plain_path(&canon.to_string_lossy()), &real),
            "{real} vs {}",
            canon.display()
        );
    }

    /// A directory link that needs no privilege: a junction on Windows.
    fn try_dir_link(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|o| o.status.success())
        }
        #[cfg(not(windows))]
        {
            try_symlink(target, link)
        }
    }

    #[test]
    fn a_swapped_parent_directory_is_refused_at_send_time() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        let sub = root.join("sub");
        let other = d.path().join("other");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir(&other).unwrap();
        fs::write(sub.join("a.txt"), "hello").unwrap();
        let (ok, _) = eligible(&[s(&sub.join("a.txt"))], &ctx(&root));
        let planned = ok[0].clone();
        // The same file (same identity, size, mtime), reached through a swapped
        // parent directory that now points elsewhere.
        fs::rename(&sub, d.path().join("sub-moved")).unwrap();
        if !try_dir_link(&other, &sub) {
            eprintln!("SKIPPED (check NOT exercised): cannot create directory symlinks here");
            return;
        }
        fs::rename(
            d.path().join("sub-moved").join("a.txt"),
            other.join("a.txt"),
        )
        .unwrap();
        assert_eq!(open_verified(&planned), Err("changed"));
    }

    #[test]
    fn a_swapped_parent_directory_is_refused_at_plan_time() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        let out = d.path().join("out");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&out).unwrap();
        fs::write(out.join("a.png"), PNG).unwrap();
        let link = root.join("dirlink");
        if !try_dir_link(&out, &link) {
            eprintln!("SKIPPED (check NOT exercised): cannot create directory symlinks here");
            return;
        }
        // canonicalize already resolves this; the handle path must agree.
        let (ok, no) = eligible(&[s(&link.join("a.png"))], &ctx(&root));
        assert!(ok.is_empty(), "{ok:?}");
        assert_eq!(no.len(), 1);
    }

    #[test]
    fn only_high_confidence_secrets_block_a_text_attachment() {
        let d = tempfile::tempdir().unwrap();
        let md = d.path().join("report.md");
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let text = format!("# Report\n\n{}\n\ncommit {sha}\n", "-".repeat(80));
        fs::write(&md, &text).unwrap();
        let (ok, no) = eligible(&[s(&md)], &ctx(d.path()));
        assert!(no.is_empty(), "{no:?}");
        assert_eq!(open_verified(&ok[0]).unwrap(), text.as_bytes());

        let bad = d.path().join("bad.txt");
        fs::write(&bad, "token ghp_abcdefghijklmnopqrstuvwxyz0123456789 here").unwrap();
        let (ok, no) = eligible(&[s(&bad)], &ctx(d.path()));
        assert!(ok.is_empty());
        assert_eq!(reasons(&no), vec![("bad.txt", "contains secrets")]);
    }

    #[test]
    fn a_secret_added_past_the_sniff_window_is_caught_at_send_time() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("big.txt");
        let mut c = ctx(d.path());
        c.max_file_bytes = 1 << 20;
        c.max_total_bytes = 1 << 20;
        let clean = "word ".repeat(20_000); // 100 000 bytes, past the 64 KiB sniff
        fs::write(&a, &clean).unwrap();
        let (ok, no) = eligible(&[s(&a)], &c);
        assert!(no.is_empty(), "{no:?}");
        let planned = ok[0].clone();
        let mut dirty = clean.clone();
        dirty.replace_range(90_000..90_000 + 15, "password=hunter2");
        dirty.truncate(clean.len());
        let t = fs::metadata(&a).unwrap().modified().unwrap();
        fs::write(&a, &dirty).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(t)
            .unwrap();
        assert_eq!(open_verified(&planned), Err("contains secrets"));
    }

    #[test]
    fn invalid_utf8_in_a_text_kind_is_refused_at_send_time() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.txt");
        fs::write(&a, "hello").unwrap();
        let (ok, _) = eligible(&[s(&a)], &ctx(d.path()));
        let t = fs::metadata(&a).unwrap().modified().unwrap();
        fs::write(&a, [0xFFu8, 0xFE, 0xFD, 0xFC, 0xFB]).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(t)
            .unwrap();
        assert_eq!(
            open_verified(&ok[0]),
            Err("content does not match its file type")
        );
    }

    #[test]
    fn a_binary_kind_that_stops_matching_its_magic_is_refused_at_send_time() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.png");
        fs::write(&a, PNG).unwrap();
        let (ok, _) = eligible(&[s(&a)], &ctx(d.path()));
        let t = fs::metadata(&a).unwrap().modified().unwrap();
        fs::write(&a, b"MZ\x90\0not a png").unwrap();
        // Same size so only the sniff can object.
        fs::OpenOptions::new()
            .write(true)
            .open(&a)
            .unwrap()
            .set_len(PNG.len() as u64)
            .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(t)
            .unwrap();
        assert_eq!(
            open_verified(&ok[0]),
            Err("content does not match its file type")
        );
    }
}
