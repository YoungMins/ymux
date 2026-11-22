//! The `chat` op (PROTOCOL.md section 5.2): the conversation of the agent in a
//! pane, read from the agent's own transcript file (Claude Code JSONL or a
//! Codex rollout), never from the screen and never through the page.
//!
//! Pure `std` plus `serde_json`: no Tauri, so every rule here runs on Linux CI.
//! Two halves:
//!
//! - [`locate`] turns a pane's recorded agent session into one transcript file
//!   that is allowed to be served: a regular file inside the agent's transcript
//!   root and, when the session id was only *inferred* from a transcript on
//!   disk (`IdSource::Disk`), one whose recorded cwd is the pane's cwd
//!   (`ypath::same_path`, CLAUDE.md rule 15).
//! - [`read`] answers one request from that file. Positions (`start`, `end`,
//!   `after`, `before`) are byte offsets of line starts; an item's `id` is
//!   `<line offset>.<index within the line>`, so ids are stable while the file
//!   only grows. Only complete lines (ending in a newline) count, a request
//!   reads at most [`MAX_READ`] bytes, a line over [`MAX_LINE`] is skipped
//!   without being parsed, and a reply holds at most 40 items and 64 KiB of
//!   text, always whole lines.
//!
//! A line longer than the read budget is crossed in steps: the reply's `end`
//! (forward) or `start` (backward) is then a byte inside that line, everything
//! up to it is examined, and the next request resumes from it, skipping the
//! rest of the line unparsed. Positions therefore always move, and a reply
//! never claims bytes it did not examine.
//!
//! Pictures (the `image` op, [`pics`]) make lines long: a line with a picture
//! is routinely over [`MAX_LINE`]. Such a line, up to [`IMAGE_LINE_MAX`], IS
//! parsed (by the borrowing parser in [`json`], which never copies a picture),
//! and its bytes are charged to a pool of their own ([`BIG_BUDGET`] per
//! request) instead of the page's [`MAX_READ`]. A line over [`IMAGE_LINE_MAX`]
//! is skipped unparsed, and crossed in steps, as before.
//!
//! Descriptors are listed only when the caller says picture sharing is on
//! (`images`); with it off no picture byte is looked at. A picture's id is
//! `<line offset>.<100 + n>`, `n` counting the pictures a line lists, in
//! order; item ids use `<line offset>.<0..20>`, so the two never meet.
//!
//! Nothing here may panic (a release build aborts on panic): every offset sum
//! is saturating or checked, bytes are never sliced as text, and a line that
//! is not valid JSON is simply not an item.

pub mod json;
pub mod pics;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use self::json::J;
use self::pics::{Pic, PicCache};
use super::clip::{self, CHAT_BYTES_MAX, CHAT_ITEMS_MAX};
use super::proto::{ChatPos, CHAT_POS_MAX};
use crate::agent_scan_disk::{
    parse_claude_head, parse_codex_meta, parse_rfc3339_secs, transcript_lookup, MAX_EXISTS_ENTRIES,
    MAX_HEAD_BYTES, MAX_HEAD_LINES,
};
use crate::agent_sessions::{AgentKind, IdSource};
use crate::attach::{open_plain_leaf, plain_path, real_path_of};

/// Most bytes of the file one request reads.
pub const MAX_READ: u64 = 4 * 1024 * 1024;
/// A line longer than this is a "big" line: parsed only up to
/// [`IMAGE_LINE_MAX`], and read out of the [`BIG_BUDGET`] pool.
pub const MAX_LINE: u64 = 2 * 1024 * 1024;
/// A line longer than this is skipped without parsing.
pub const IMAGE_LINE_MAX: u64 = 24 * 1024 * 1024;
/// Most bytes of big lines (past their first [`MAX_LINE`]) one request reads.
/// Two lines of [`IMAGE_LINE_MAX`] fit; a line that is the first of a request
/// always gets its turn, so a page always makes progress.
pub const BIG_BUDGET: u64 = 48 * 1024 * 1024;
/// Chunk for reading a big line, so a 24 MiB line is not 24 small reads (and
/// going backwards, not 24 copies of everything already read).
const BIG_CHUNK: u64 = 4 * 1024 * 1024;
/// A page is sized to the limit minus this much, so that even if the reply
/// check changed a text by a few bytes (it should not: the texts are already
/// final) a page would still fit. Simpler than dropping items in `reply_data`,
/// which would have to keep `start`/`end` and the cursor guarantee truthful.
const PAGE_SLACK: usize = 512;
/// Most bytes the end-of-file scan for the last newline may cover, apart from
/// the page budget: an unfinished last line up to this long is stepped over.
const SCAN_MAX: u64 = 64 * 1024 * 1024;
/// First read size; doubles up to [`CHUNK_MAX`].
const CHUNK: u64 = 64 * 1024;
const CHUNK_MAX: u64 = 1024 * 1024;
/// One line yields at most this many items with text, and at most as many that
/// exist only for their pictures, and this many bytes of text, so any single
/// line fits one reply (20 + 20 <= 40 items, 48 KiB <= 64 KiB) and paging by
/// whole lines always makes progress.
const LINE_ITEMS_MAX: usize = 20;
const LINE_BYTES_MAX: usize = 48 * 1024;

/// The reply for "nothing to show".
pub fn none() -> Value {
    json!({"conv": ""})
}

// ------------------------------------------------------------------ locate

/// What the session tracker knows about a pane's agent.
pub struct Binding<'a> {
    pub agent: AgentKind,
    pub session_id: &'a str,
    /// The cwd the record carries (a hint for Claude's project directory).
    pub record_cwd: &'a str,
    pub source: IdSource,
}

/// The transcript file of `b` under `root` (the agent's `projects` / `sessions`
/// directory), or `None` when it may not be served: not found, not a regular
/// file, outside `root` once links are resolved, or (for a `Disk` binding) not
/// recorded as having run in `pane_cwd`. An unknown pane cwd refuses a `Disk`
/// binding: the record's own cwd is the very thing being checked.
pub fn locate(b: &Binding<'_>, root: &Path, pane_cwd: &str) -> Option<PathBuf> {
    let found = find(b, root).ok()??;
    open(b, root, &found, pane_cwd).ok().map(|o| o.path)
}

/// Walk the transcript root for the session's file (the slow part: callers
/// that poll keep the result, see [`answer`]). `Ok(None)` is "not there";
/// `Err(Io)` is a read error while looking.
pub fn find(b: &Binding<'_>, root: &Path) -> Result<Option<PathBuf>, OpenErr> {
    transcript_lookup(
        b.agent,
        root,
        b.session_id,
        b.record_cwd,
        MAX_EXISTS_ENTRIES,
    )
    .map_err(|_| OpenErr::Io)
}

/// Why a candidate file is not served.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenErr {
    /// There is no such file (any more), or it is not one this session may
    /// show: another session's file behind a link, outside the root, not a
    /// regular file, a `Disk` binding in another directory. Nothing to show.
    Refused,
    /// It exists but could not be opened or read this time.
    Io,
}

/// An opened, vetted transcript. Everything after the checks reads through
/// `file`, so a path swapped afterwards changes nothing.
#[derive(Debug)]
pub struct Opened {
    file: File,
    pub path: PathBuf,
    len: u64,
    id: Option<(u64, u64)>,
}

/// Is `path` (canonical) the transcript of `session_id`? Claude names the file
/// `<session id>.jsonl`; Codex ends it `-<session id>.jsonl` (the rule
/// `codex_rollout_path` finds it by). A link inside the root that points at
/// another session's file fails this.
pub fn is_session_file(agent: AgentKind, path: &Path, session_id: &str) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    match agent {
        AgentKind::Claude => name.strip_suffix(".jsonl") == Some(session_id),
        AgentKind::Codex => name.ends_with(&format!("-{session_id}.jsonl")),
    }
}

/// Canonicalise `candidate` (every time: a link retargeted since it was found
/// is noticed), check it, open it once without following a link at the leaf and
/// check the handle: a regular file with one hard link whose real path, read
/// back from the handle, is inside the canonical root and is this session's
/// file. A leaf that is not a regular file is never opened (no hang on a FIFO).
pub fn open(
    b: &Binding<'_>,
    root: &Path,
    candidate: &Path,
    pane_cwd: &str,
) -> Result<Opened, OpenErr> {
    let io = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::NotFound => OpenErr::Refused,
        _ => OpenErr::Io,
    };
    let root = root.canonicalize().map_err(io)?;
    let path = candidate.canonicalize().map_err(io)?;
    if !path.starts_with(&root) || !is_session_file(b.agent, &path, b.session_id) {
        return Err(OpenErr::Refused);
    }
    let Some(mut file) = open_plain_leaf(&path).map_err(io)? else {
        return Err(OpenErr::Refused);
    };
    // Where the handle really is, which a swap after `canonicalize` cannot
    // change: inside the root, and still this session's file.
    let real = real_path_of(&file).map_err(|_| OpenErr::Io)?;
    let root_plain = plain_path(&root.to_string_lossy());
    if !crate::fsx::is_within(&root_plain, &real)
        || !is_session_file(b.agent, Path::new(&real), b.session_id)
    {
        return Err(OpenErr::Refused);
    }
    let md = file.metadata().map_err(io)?;
    if b.source == IdSource::Disk && !recorded_in(b.agent, &mut file, pane_cwd)? {
        return Err(OpenErr::Refused);
    }
    let id = file_identity(&file);
    Ok(Opened {
        file,
        path,
        len: md.len(),
        id,
    })
}

/// The whole answer for a pane: find (or reuse `cached`), open and vet, read.
/// `Some({"conv":""})` when nothing may be shown, `None` when the conversation
/// exists but could not be read this time (a read error while looking, opening
/// or reading). `cached` is the path the directory walk found last time, as
/// found (not canonicalised); it is replaced by what this request found, and
/// dropped when it no longer opens (then the walk is done again once). Every
/// request canonicalises and vets the file, cached path or not.
///
/// `images`: list the pictures of the conversation (picture sharing is on).
/// `work` is increased by the length of each big line (over [`MAX_LINE`]) this
/// call read, for the per-device work budget. `big`: big lines may be read.
/// Without it the answer holds what can be served without one and stops in
/// front of the first big line (`newer` / `older` true, cursors honest); when
/// the very first line needed is big, the answer is [`busy`].
#[allow(clippy::too_many_arguments)]
pub fn answer(
    b: &Binding<'_>,
    root: &Path,
    pane_cwd: &str,
    conv: &str,
    pos: Option<ChatPos>,
    images: bool,
    big: bool,
    cached: &mut Option<PathBuf>,
    work: &mut u64,
) -> Option<Value> {
    match resolve(b, root, pane_cwd, cached) {
        Resolved::Open(opened) => match read_opened(
            Rd {
                agent: b.agent,
                images,
                big,
            },
            b.session_id,
            opened,
            conv,
            pos,
            work,
        ) {
            Ok(v) => Some(v),
            // Nothing can be served without a big line the device may not
            // have read just now.
            Err(Fail::Denied) => Some(busy()),
            Err(_) => None,
        },
        Resolved::Nothing => Some(none()),
        Resolved::Io => None,
    }
}

/// What [`resolve`] made of a binding.
enum Resolved {
    Open(Opened),
    /// Nothing that may be shown.
    Nothing,
    /// It exists but could not be read this time.
    Io,
}

/// Find (or reuse `cached`), open and vet the transcript. The path that
/// worked is left in `cached`.
fn resolve(b: &Binding<'_>, root: &Path, pane_cwd: &str, cached: &mut Option<PathBuf>) -> Resolved {
    let mut walked = false;
    let (opened, found) = loop {
        let path = match cached.take() {
            Some(p) => p,
            None => {
                walked = true;
                match find(b, root) {
                    Ok(Some(p)) => p,
                    Ok(None) => return Resolved::Nothing,
                    Err(_) => return Resolved::Io,
                }
            }
        };
        match open(b, root, &path, pane_cwd) {
            Ok(o) => break (o, path),
            Err(e) if walked => {
                return match e {
                    OpenErr::Refused => Resolved::Nothing,
                    OpenErr::Io => Resolved::Io,
                }
            }
            // A remembered path that no longer works: look again, once.
            Err(_) => {}
        }
    };
    *cached = Some(found);
    Resolved::Open(opened)
}

/// Does the transcript's own first records say it ran in `pane_cwd`?
fn recorded_in(agent: AgentKind, file: &mut File, pane_cwd: &str) -> Result<bool, OpenErr> {
    if pane_cwd.trim().is_empty() {
        return Ok(false);
    }
    let mut head = Vec::new();
    file.seek(SeekFrom::Start(0)).map_err(|_| OpenErr::Io)?;
    Read::by_ref(file)
        .take(MAX_HEAD_BYTES as u64)
        .read_to_end(&mut head)
        .map_err(|_| OpenErr::Io)?;
    let text = String::from_utf8_lossy(&head);
    Ok(match agent {
        AgentKind::Claude => {
            let head: String = text
                .lines()
                .take(MAX_HEAD_LINES)
                .collect::<Vec<_>>()
                .join("\n");
            parse_claude_head(&head)
                .cwd
                .is_some_and(|c| ypath::same_path(&c, pane_cwd))
        }
        AgentKind::Codex => text
            .split_inclusive('\n')
            .next()
            .filter(|l| l.ends_with('\n'))
            .and_then(parse_codex_meta)
            .is_some_and(|m| ypath::same_path(&m.cwd, pane_cwd)),
    })
}

#[cfg(unix)]
fn file_identity(f: &File) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let md = f.metadata().ok()?;
    Some((md.dev(), md.ino()))
}

/// Volume serial number and file index: `std` has no stable accessor.
#[cfg(windows)]
fn file_identity(f: &File) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut bi = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is owned by `f` for the whole call and `bi` is a
    // valid, writable out-parameter.
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle() as isize), &mut bi) }.ok()?;
    Some((
        u64::from(bi.dwVolumeSerialNumber),
        (u64::from(bi.nFileIndexHigh) << 32) | u64::from(bi.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_: &File) -> Option<(u64, u64)> {
    None
}

/// `conv`: the first 12 bytes of SHA-256 over the agent, the session id, the
/// path string and (when the system gives one) the file's identity, separated
/// by NUL, base64url without padding. A file replaced by another one at the
/// same path is another conversation.
pub fn conv_id(
    agent: AgentKind,
    session_id: &str,
    path: &Path,
    identity: Option<(u64, u64)>,
) -> String {
    let mut h = Sha256::new();
    h.update(agent.as_str().as_bytes());
    h.update([0]);
    h.update(session_id.as_bytes());
    h.update([0]);
    h.update(path.to_string_lossy().as_bytes());
    if let Some((dev, ino)) = identity {
        h.update([0]);
        h.update(dev.to_le_bytes());
        h.update(ino.to_le_bytes());
    }
    super::keys::b64u(&h.finalize()[..12])
}

// ------------------------------------------------------------------- items

/// A picture an item lists: its ordinal in the line (the number in its id),
/// its type (from the file signature) and decoded size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Img {
    ord: u32,
    kind: &'static str,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    role: &'static str,
    text: String,
    ts_ms: u64,
    /// The number after the dot in the item's id: 0..20 for an item with text,
    /// 20..40 for one that exists only for its pictures.
    idx: u32,
    images: Vec<Img>,
}

/// An item while the line it came from is still borrowed: its pictures are
/// still base64.
struct ItemP<'t> {
    item: Item,
    pics: Vec<(u32, Pic<'t>)>,
}

/// Milliseconds since the epoch of an RFC 3339 timestamp.
fn rfc3339_ms(s: &str) -> Option<u64> {
    let secs = parse_rfc3339_secs(s)?;
    let frac = s
        .get(19..)
        .and_then(|r| r.strip_prefix('.'))
        .map_or(0, |r| {
            let mut digits = r.bytes().take_while(u8::is_ascii_digit);
            (0..3).fold(0u64, |ms, _| {
                ms * 10 + digits.next().map_or(0, |c| u64::from(c - b'0'))
            })
        });
    secs.checked_mul(1000)?.checked_add(frac)
}

fn flag(v: &J<'_>, key: &str) -> bool {
    v.get(key).and_then(J::as_bool) == Some(true)
}

/// Text that only carries the agent harness's own markup: slash-command echoes,
/// injected reminders, notifications, session context. Not a message of the user.
fn is_wrapper(agent: AgentKind, text: &str) -> bool {
    let t = text.trim_start();
    let prefixes: &[&str] = match agent {
        AgentKind::Claude => &[
            "<command-name>",
            "<command-message>",
            "<command-args>",
            "<local-command-stdout>",
            "<local-command-stderr>",
            "<local-command-caveat>",
            "<bash-stdout>",
            "<bash-stderr>",
            "<system-reminder>",
            "<task-notification>",
            "<ide_opened_file>",
            "<ide_selection>",
        ],
        AgentKind::Codex => &[
            "<environment_context>",
            "<user_instructions>",
            "<user_action>",
            "<turn_aborted>",
            "<permissions instructions>",
            "<collaboration_mode>",
            "<user_shell_command>",
            "# AGENTS.md instructions",
        ],
    };
    prefixes.iter().any(|p| t.starts_with(p))
}

/// The main input of a tool call, as one short string.
fn summarize(input: &J<'_>) -> String {
    const KEYS: [&str; 9] = [
        "command",
        "cmd",
        "file_path",
        "path",
        "pattern",
        "query",
        "url",
        "description",
        "prompt",
    ];
    for k in KEYS {
        match input.get(k) {
            Some(J::Str(s)) if !s.trim().is_empty() => return shell_words(s),
            Some(J::Arr(a)) => {
                let words: Vec<&str> = a.iter().filter_map(J::as_str).collect();
                if !words.is_empty() {
                    return shell_argv(&words);
                }
            }
            _ => {}
        }
    }
    String::new()
}

/// The field as it is; `clip::chat_tool_line` redacts it whole before cutting.
fn shell_words(s: &str) -> String {
    s.to_string()
}

/// `["bash","-lc","cargo test"]` reads as `cargo test`.
fn shell_argv(words: &[&str]) -> String {
    let shown = if words.len() >= 3 && words[1].starts_with('-') && words[1].ends_with('c') {
        words[2..].join(" ")
    } else {
        words.join(" ")
    };
    shell_words(&shown)
}

/// What a line holds before cleaning: message text, or a tool call's name and
/// its main input (cut to one line later, after redaction).
///
/// A message can carry pictures (an empty text is fine then). `ToolPics` is a
/// tool's result that handed the agent pictures: an item of role `tool` with no
/// text (the call it answers is on another line, and its own text output is
/// never shown).
enum Raw<'t> {
    Text(&'static str, String, Vec<Pic<'t>>),
    Tool(String, String),
    ToolPics(Vec<Pic<'t>>),
}

fn tool<'t>(name: &str, summary: &str) -> Raw<'t> {
    let name = if name.is_empty() { "tool" } else { name };
    Raw::Tool(name.to_string(), summary.to_string())
}

/// `<bash-input>cmd</bash-input>`: Claude Code's record of a `!cmd` the user
/// ran. Its output arrives as `<bash-stdout>` / `<bash-stderr>` records.
fn bash_input(text: &str) -> Option<&str> {
    let rest = text.trim_start().strip_prefix("<bash-input>")?;
    Some(command_end(
        rest,
        &["</bash-input>", "<bash-stdout>", "<bash-stderr>"],
    ))
}

/// Codex's record of a user shell command: only the command, never the result.
fn user_shell_command(text: &str) -> Option<String> {
    let rest = text.trim_start().strip_prefix("<user_shell_command>")?;
    let cmd = rest.split_once("<command>").map_or("", |(_, c)| {
        command_end(c, &["</command>", "<result>", "</user_shell_command>"])
    });
    Some(cmd.to_string())
}

/// The command in `rest`: up to the first of `enders` (a closing tag, or the
/// start of the output that follows); with none of them, its first line. A
/// missing closing tag must never turn the output into the command.
fn command_end<'a>(rest: &'a str, enders: &[&str]) -> &'a str {
    let end = enders
        .iter()
        .filter_map(|e| rest.find(e))
        .min()
        .unwrap_or_else(|| {
            rest.trim_start().find('\n').map_or(rest.len(), |i| {
                // `i` is in the trimmed string: map back to `rest`.
                rest.len() - rest.trim_start().len() + i
            })
        });
    rest[..end].trim()
}

/// Joined text blocks: blank ones and the harness's wrappers are dropped.
fn join_texts<'a>(agent: AgentKind, texts: impl Iterator<Item = &'a str>) -> Option<String> {
    let kept: Vec<&str> = texts
        .filter(|t| !t.trim().is_empty() && !is_wrapper(agent, t))
        .collect();
    (!kept.is_empty()).then(|| kept.join("\n\n"))
}

fn block_type<'t>(b: &'t J<'_>) -> Option<&'t str> {
    b.get("type").and_then(J::as_str)
}

/// `images`: look for pictures (picture sharing is on).
fn claude_raw<'t>(v: &'t J<'_>, images: bool) -> Vec<Raw<'t>> {
    if flag(v, "isSidechain") {
        return Vec::new();
    }
    let content = v.get("message").and_then(|m| m.get("content"));
    match v.get("type").and_then(J::as_str) {
        Some("user") => {
            if flag(v, "isMeta") || flag(v, "isCompactSummary") {
                return Vec::new();
            }
            let mut pics: Vec<Pic<'t>> = Vec::new();
            let texts: Vec<&str> = match content {
                Some(J::Str(s)) => vec![&**s],
                Some(J::Arr(blocks)) => {
                    // A tool result line is the harness talking back, whatever
                    // else rides along in it: no text of it is shown. What a
                    // tool handed the agent as a picture (a file it opened, a
                    // screenshot) is, one item per result.
                    if blocks.iter().any(|b| block_type(b) == Some("tool_result")) {
                        if !images {
                            return Vec::new();
                        }
                        return blocks
                            .iter()
                            .filter(|b| block_type(b) == Some("tool_result"))
                            .filter_map(|b| match b.get("content") {
                                Some(J::Arr(inner)) => Some(pics::claude_pics(inner)),
                                _ => None,
                            })
                            .filter(|p| !p.is_empty())
                            .map(Raw::ToolPics)
                            .collect();
                    }
                    if images {
                        pics = pics::claude_pics(blocks);
                    }
                    blocks
                        .iter()
                        .filter(|b| block_type(b) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(J::as_str))
                        .collect()
                }
                _ => return Vec::new(),
            };
            // A `!cmd` the user ran is a tool item; its output is a wrapper.
            let (shell, plain): (Vec<&str>, Vec<&str>) =
                texts.into_iter().partition(|t| bash_input(t).is_some());
            let text = join_texts(AgentKind::Claude, plain.into_iter());
            let mut out: Vec<Raw<'t>> = Vec::new();
            if text.is_some() || !pics.is_empty() {
                out.push(Raw::Text("user", text.unwrap_or_default(), pics));
            }
            out.extend(
                shell
                    .into_iter()
                    .filter_map(bash_input)
                    .map(|c| tool("Bash", c)),
            );
            out
        }
        Some("assistant") => {
            let mut out: Vec<Raw<'t>> = Vec::new();
            let mut run: Vec<&str> = Vec::new();
            let flush = |run: &mut Vec<&str>, out: &mut Vec<Raw<'t>>| {
                if let Some(t) = join_texts(AgentKind::Claude, run.drain(..)) {
                    out.push(Raw::Text("assistant", t, Vec::new()));
                }
            };
            match content {
                Some(J::Str(s)) => run.push(&**s),
                Some(J::Arr(blocks)) => {
                    for b in blocks {
                        match b.get("type").and_then(J::as_str) {
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(J::as_str) {
                                    run.push(t);
                                }
                            }
                            Some("tool_use") => {
                                flush(&mut run, &mut out);
                                let name = b.get("name").and_then(J::as_str).unwrap_or("");
                                let input = b.get("input").unwrap_or(&J::Null);
                                out.push(tool(name, &summarize(input)));
                            }
                            // Reasoning, results, anything new: never shown.
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            flush(&mut run, &mut out);
            out
        }
        _ => Vec::new(),
    }
}

fn codex_raw<'t>(v: &'t J<'_>, images: bool) -> Vec<Raw<'t>> {
    if v.get("type").and_then(J::as_str) != Some("response_item") {
        return Vec::new();
    }
    let Some(p) = v.get("payload") else {
        return Vec::new();
    };
    match p.get("type").and_then(J::as_str) {
        Some("message") => {
            let role = match p.get("role").and_then(J::as_str) {
                Some("user") => "user",
                Some("assistant") => "assistant",
                _ => return Vec::new(),
            };
            let Some(blocks) = p.get("content").and_then(J::as_array) else {
                return Vec::new();
            };
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|b| {
                    matches!(
                        b.get("type").and_then(J::as_str),
                        Some("input_text" | "output_text")
                    )
                })
                .filter_map(|b| b.get("text").and_then(J::as_str))
                .collect();
            // Only what the user attached; a picture the model wrote is not
            // part of what this view shows.
            let pics = if images && role == "user" {
                pics::codex_pics(blocks)
            } else {
                Vec::new()
            };
            let text = join_texts(AgentKind::Codex, texts.iter().copied());
            let mut out: Vec<Raw<'t>> = Vec::new();
            if text.is_some() || !pics.is_empty() {
                out.push(Raw::Text(role, text.unwrap_or_default(), pics));
            }
            if role == "user" {
                out.extend(
                    texts
                        .iter()
                        .filter_map(|t| user_shell_command(t))
                        .map(|c| tool("shell", &c)),
                );
            }
            out
        }
        Some("function_call") => {
            let name = p.get("name").and_then(J::as_str).unwrap_or("");
            let summary = match p.get("arguments") {
                Some(J::Str(s)) => match json::parse_str(s) {
                    Some(args) => summarize(&args),
                    None => shell_words(s),
                },
                Some(args) => summarize(args),
                None => String::new(),
            };
            vec![tool(name, &summary)]
        }
        Some("local_shell_call") => {
            let words: Vec<&str> = p
                .get("action")
                .and_then(|a| a.get("command"))
                .and_then(J::as_array)
                .map(|a| a.iter().filter_map(J::as_str).collect())
                .unwrap_or_default();
            vec![tool("shell", &shell_argv(&words))]
        }
        Some("custom_tool_call") => {
            let name = p.get("name").and_then(J::as_str).unwrap_or("");
            let first = p
                .get("input")
                .and_then(J::as_str)
                .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or("");
            vec![tool(name, &shell_words(first))]
        }
        // What a call handed back: its text is never shown, a picture in it
        // (an array of content items) is.
        Some("function_call_output" | "custom_tool_call_output") if images => {
            match p.get("output").and_then(J::as_array) {
                Some(items) => {
                    let pics = pics::codex_pics(items);
                    if pics.is_empty() {
                        Vec::new()
                    } else {
                        vec![Raw::ToolPics(pics)]
                    }
                }
                None => Vec::new(),
            }
        }
        // Reasoning, outputs of calls, developer messages: never shown.
        _ => Vec::new(),
    }
}

/// The items of one transcript line, cleaned, redacted and clipped. At most
/// [`LINE_ITEMS_MAX`] items and [`LINE_BYTES_MAX`] bytes, in order. `images`:
/// list the pictures too (an item may then have an empty text).
fn line_items(agent: AgentKind, line: &[u8], images: bool) -> Vec<Item> {
    with_items(agent, line, images, |items| {
        items.into_iter().map(|i| i.item).collect()
    })
    .unwrap_or_default()
}

/// Parse `line` and hand its items, pictures still borrowed from the line, to
/// `f`. `None` when the line is not JSON (or is too wild to parse). The line
/// is parsed by [`json`]: a picture's base64 is never copied.
fn with_items<R>(
    agent: AgentKind,
    line: &[u8],
    images: bool,
    f: impl for<'t> FnOnce(Vec<ItemP<'t>>) -> R,
) -> Option<R> {
    // A CRLF file leaves a `\r` on every line.
    let end = line
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    let line = line.get(..end)?;
    if line.is_empty() {
        return None;
    }
    // A big line holds few values (a picture is one string); one that holds
    // hundreds of thousands is skipped rather than built into a tree.
    let v = json::parse_with(
        line,
        if line.len() as u64 > MAX_LINE {
            json::BIG_NODE_MAX
        } else {
            json::NODE_MAX
        },
    )?;
    let ts_ms = v
        .get("timestamp")
        .and_then(J::as_str)
        .and_then(rfc3339_ms)
        .unwrap_or(0);
    let raw = match agent {
        AgentKind::Claude => claude_raw(&v, images),
        AgentKind::Codex => codex_raw(&v, images),
    };
    let mut out: Vec<ItemP<'_>> = Vec::new();
    let mut bytes = 0usize;
    let mut ord = 0u32;
    let (mut n_text, mut n_pic) = (0usize, 0usize);
    for r in raw {
        let (role, text, pics) = match r {
            Raw::Text(role, t, pics) => (role, clip::chat_text(&t), pics),
            Raw::Tool(name, summary) => ("tool", clip::chat_tool_line(&name, &summary), Vec::new()),
            Raw::ToolPics(pics) => ("tool", String::new(), pics),
        };
        if text.trim().is_empty() && pics.is_empty() {
            continue;
        }
        // An item that exists only for its pictures has an index of its own
        // (20..40), so every item that is there with sharing off keeps the
        // index it has then: ids do not move when the switch is flipped. Such
        // an item over its cap is skipped; a text item over its cap ends the
        // line (as it always did), whatever came before it.
        let pic_only = text.trim().is_empty();
        let idx = if pic_only {
            if n_pic >= LINE_ITEMS_MAX {
                continue;
            }
            n_pic += 1;
            LINE_ITEMS_MAX + n_pic - 1
        } else {
            if n_text >= LINE_ITEMS_MAX || bytes + text.len() > LINE_BYTES_MAX {
                break;
            }
            n_text += 1;
            n_text - 1
        };
        bytes += text.len();
        // Numbered in the order the line lists them, so the same line always
        // gives the same ids (the `image` op finds a picture by running this
        // very function again).
        let numbered: Vec<(u32, Pic<'_>)> = pics
            .into_iter()
            .take(pics::PER_ITEM)
            .map(|p| {
                ord += 1;
                (ord - 1, p)
            })
            .collect();
        let listed = numbered
            .iter()
            .map(|(ord, p)| Img {
                ord: *ord,
                kind: p.kind,
                size: p.size,
            })
            .collect();
        out.push(ItemP {
            item: Item {
                role,
                text,
                ts_ms,
                idx: idx as u32,
                images: listed,
            },
            pics: numbered,
        });
    }
    Some(f(out))
}

// ------------------------------------------------------------------ reader

#[derive(Debug)]
enum Fail {
    /// The file could not be read.
    Io,
    /// The read budget ran out.
    Budget,
    /// The request cannot make any progress without reading a big line, and the
    /// device is over its big-line work budget: the answer is `busy`.
    Denied,
}

/// What one request reads with: the agent's file format, whether to list
/// pictures, and whether big lines may be read at all (`big`: the device is
/// inside its big-line work budget). Without `big` the reader serves what it
/// can and stops, truthfully, in front of the first big line it would need.
#[derive(Clone, Copy)]
struct Rd {
    agent: AgentKind,
    images: bool,
    big: bool,
}

struct Src {
    file: File,
    len: u64,
    /// Bytes read for the page (everything except the big part of big lines).
    used: u64,
    /// Bytes read for big lines, past their first [`MAX_LINE`] (the per-request
    /// pool, [`BIG_BUDGET`]).
    big_used: u64,
    /// What the request cost the device's work budget: the length of each big
    /// line it read (once per line), nothing for a line of [`MAX_LINE`] or less.
    work: u64,
}

impl Src {
    fn new(file: File, len: u64) -> Self {
        Src {
            file,
            len,
            used: 0,
            big_used: 0,
            work: 0,
        }
    }

    /// A line of `len` bytes was read (or, mid-line, that much of it is in
    /// hand): charge it if it is big. Lines over [`IMAGE_LINE_MAX`] cost what
    /// was read of them as big (their first [`IMAGE_LINE_MAX`] bytes).
    fn charge_line(&mut self, len: u64) {
        if len > MAX_LINE {
            self.work += len.min(IMAGE_LINE_MAX);
        }
    }
}

/// The `so_far` of a line whose start is not in hand (a resumed position):
/// its rest is skipped unparsed and paid out of the page budget.
const SKIPPING: u64 = u64::MAX;

impl Src {
    fn left(&self) -> u64 {
        MAX_READ.saturating_sub(self.used)
    }

    /// How much of the next read of a line that already has `so_far` bytes in
    /// hand is allowed: the chunk, but never past the point where the line
    /// changes class (normal / big / too long to parse), and bigger chunks for
    /// a big line.
    fn line_chunk(chunk: u64, so_far: u64) -> u64 {
        if so_far < MAX_LINE {
            chunk.min(MAX_LINE - so_far)
        } else if so_far < IMAGE_LINE_MAX {
            chunk.max(BIG_CHUNK).min(IMAGE_LINE_MAX - so_far)
        } else {
            chunk
        }
    }

    /// What is left of the pool that pays for the next bytes of a line with
    /// `so_far` bytes in hand: the big pool for a big line, else the page's.
    fn pool_left(&self, so_far: u64) -> u64 {
        if (MAX_LINE..IMAGE_LINE_MAX).contains(&so_far) {
            BIG_BUDGET.saturating_sub(self.big_used)
        } else {
            self.left()
        }
    }

    /// Charge `n` bytes read for a line that had `so_far` bytes in hand.
    fn charge(&mut self, so_far: u64, n: usize) {
        if (MAX_LINE..IMAGE_LINE_MAX).contains(&so_far) {
            self.big_used += n as u64;
        } else {
            self.used += n as u64;
        }
    }

    /// Up to `n` bytes at `off`, clamped to the file only: not charged to any
    /// budget (the callers are the end-of-file scan, one-byte probes, and the
    /// page readers, which [`Src::charge`] what they read themselves).
    fn raw(&mut self, off: u64, n: u64) -> Result<Vec<u8>, Fail> {
        let n = n.min(self.len.saturating_sub(off));
        if n == 0 {
            return Ok(Vec::new());
        }
        self.file.seek(SeekFrom::Start(off)).map_err(|_| Fail::Io)?;
        let mut buf = vec![0u8; usize::try_from(n).map_err(|_| Fail::Io)?];
        self.file.read_exact(&mut buf).map_err(|_| Fail::Io)?;
        Ok(buf)
    }

    /// The offset just after the last newline in the file: where the last
    /// complete line ends (0 when there is none). The scan has a budget of its
    /// own ([`SCAN_MAX`]) so an unfinished last line never takes anything from
    /// the page that follows. If no newline turns up within it, the answer is
    /// the position the scan stopped at when that is a line start, else an
    /// error (the conversation exists but cannot be read this time).
    fn last_complete_end(&mut self) -> Result<u64, Fail> {
        let mut hi = self.len;
        let mut scanned = 0u64;
        let mut chunk = CHUNK;
        while hi > 0 {
            if scanned >= SCAN_MAX {
                return if self.raw(hi - 1, 1)?.first() == Some(&b'\n') {
                    Ok(hi)
                } else {
                    Err(Fail::Budget)
                };
            }
            let n = chunk.min(hi);
            let lo = hi - n;
            let buf = self.raw(lo, n)?;
            if let Some(i) = buf.iter().rposition(|b| *b == b'\n') {
                return Ok(lo + i as u64 + 1);
            }
            scanned += n;
            chunk = (chunk * 2).min(CHUNK_MAX);
            hi = lo;
        }
        Ok(0)
    }

    /// Is `p` the start of a line (the file start, or just after a newline)?
    fn at_line_start(&mut self, p: u64) -> Result<bool, Fail> {
        if p == 0 {
            return Ok(true);
        }
        Ok(self.raw(p - 1, 1)?.first() == Some(&b'\n'))
    }
}

/// Whole lines, newest last in `lines` after the final reverse.
struct Page {
    /// `(line offset, items)` of each line that has items, in file order.
    lines: Vec<(u64, Vec<Item>)>,
    /// Everything from `start` to `end` has been examined. A position is a
    /// line start, or a byte inside a line that was too long for the budget to
    /// step over in one request (the next request resumes from it).
    start: u64,
    end: u64,
    older: bool,
    newer: bool,
}

/// Would adding `n` items of `b` bytes to a page that holds `count` / `bytes`
/// break a reply bound?
fn overflows(count: usize, bytes: usize, items: &[Item]) -> bool {
    count + items.len() > CHAT_ITEMS_MAX
        || bytes + items.iter().map(|i| i.text.len()).sum::<usize>() > CHAT_BYTES_MAX - PAGE_SLACK
}

fn text_bytes(items: &[Item]) -> usize {
    items.iter().map(|i| i.text.len()).sum()
}

/// Items of the line `[ls, le)` (`le` exclusive of its newline) held in `buf`
/// from `base`. A line over [`IMAGE_LINE_MAX`] is not looked at.
fn items_of(rd: Rd, buf: &[u8], base: u64, ls: u64, le: u64) -> Vec<Item> {
    if le.saturating_sub(ls) > IMAGE_LINE_MAX {
        return Vec::new();
    }
    let (Ok(a), Ok(b)) = (
        usize::try_from(ls.saturating_sub(base)),
        usize::try_from(le.saturating_sub(base)),
    ) else {
        return Vec::new();
    };
    buf.get(a..b)
        .map(|l| line_items(rd.agent, l, rd.images))
        .unwrap_or_default()
}

impl Src {
    /// The lines just before `hi` (a line start), newest first scanned, from
    /// the end of the file towards its start.
    ///
    /// `partial`: `hi` is inside a line (a resumed position), whose head is
    /// skipped unparsed.
    fn page_back(&mut self, rd: Rd, hi: u64, mut partial: bool) -> Result<Page, Fail> {
        let mut buf: Vec<u8> = Vec::new();
        // `buf` holds the file bytes [lo, hi).
        let mut lo = hi;
        // Everything from `pe` to `hi` is examined.
        let mut pe = hi;
        let mut chunk = CHUNK;
        let mut exhausted = false;
        let mut lines: Vec<(u64, Vec<Item>)> = Vec::new();
        let (mut count, mut bytes) = (0usize, 0usize);
        let (older, start) = loop {
            if pe == 0 {
                break (false, 0);
            }
            // The text of the line before `pe` ends at its newline (pe - 1),
            // or at `pe` itself for a position inside a line.
            let text_end = if partial { pe } else { pe - 1 };
            let ls = loop {
                if text_end > lo {
                    let hay_end = usize::try_from(text_end - lo).map_err(|_| Fail::Io)?;
                    if let Some(i) = buf
                        .get(..hay_end)
                        .and_then(|h| h.iter().rposition(|b| *b == b'\n'))
                    {
                        break Some(lo + i as u64 + 1);
                    }
                }
                if lo == 0 {
                    break Some(0);
                }
                if exhausted {
                    break None;
                }
                // What the budget still allows, taken from the end of the
                // unread part (the bytes nearest to `lo`). A big line is paid
                // for by the big pool, the head of a resumed line by the page.
                let so_far = if partial {
                    SKIPPING
                } else {
                    text_end.saturating_sub(lo)
                };
                if !rd.big && so_far >= MAX_LINE && so_far != SKIPPING {
                    // The line has MAX_LINE bytes in hand and no start yet: it
                    // is big unless the byte before them is its predecessor's
                    // newline (a line of exactly MAX_LINE bytes).
                    if self.raw(lo - 1, 1)?.first() == Some(&b'\n') {
                        let mut got = self.raw(lo - 1, 1)?;
                        lo -= 1;
                        got.extend_from_slice(&buf);
                        buf = got;
                        continue;
                    }
                    // Not allowed to read it: after whole lines the page stops
                    // in front of it; as the first thing there is no progress.
                    if pe < hi {
                        break None;
                    }
                    return Err(Fail::Denied);
                }
                let want = Self::line_chunk(chunk, so_far).min(lo);
                let n = self.pool_left(so_far).min(want);
                if n < want {
                    exhausted = true;
                }
                if n == 0 {
                    break None;
                }
                let mut got = self.raw(lo - n, n)?;
                self.charge(so_far, got.len());
                lo -= n;
                got.extend_from_slice(&buf);
                buf = got;
                chunk = (chunk * 2).min(CHUNK_MAX);
            };
            let Some(ls) = ls else {
                // The line before `pe` is longer than the budget has left.
                // After whole lines the page stops at a line start; as the
                // first thing it stops inside the line, and the next request
                // resumes from there (so a long line is crossed in steps).
                if pe >= hi && !partial {
                    self.charge_line(text_end.saturating_sub(lo));
                }
                break if pe < hi { (true, pe) } else { (lo > 0, lo) };
            };
            if partial {
                partial = false;
                pe = ls;
                continue;
            }
            let items = items_of(rd, &buf, lo, ls, pe - 1);
            if !items.is_empty() && overflows(count, bytes, &items) {
                // Not consumed into this reply: the next request reads it
                // again, and is charged for it then.
                break (true, pe);
            }
            self.charge_line(pe - 1 - ls);
            count += items.len();
            bytes += text_bytes(&items);
            if !items.is_empty() {
                lines.push((ls, items));
            }
            pe = ls;
        };
        lines.reverse();
        Ok(Page {
            lines,
            start,
            end: hi,
            older,
            newer: false,
        })
    }

    /// The lines from `from` (a line start) up to `lc` (the end of the last
    /// complete line), oldest first.
    ///
    /// `partial`: `from` is inside a line (a resumed position): the rest of
    /// that line is skipped unparsed.
    fn page_fwd(&mut self, rd: Rd, from: u64, lc: u64, mut partial: bool) -> Result<Page, Fail> {
        let mut buf: Vec<u8> = Vec::new();
        // `buf` holds the file bytes [from, from + buf.len()).
        let mut cur = from;
        let mut chunk = CHUNK;
        let mut exhausted = false;
        let mut lines: Vec<(u64, Vec<Item>)> = Vec::new();
        let (mut count, mut bytes) = (0usize, 0usize);
        let (end, newer) = loop {
            if cur >= lc {
                break (cur, false);
            }
            let rel = usize::try_from(cur - from).map_err(|_| Fail::Io)?;
            let nl = buf
                .get(rel..)
                .and_then(|b| b.iter().position(|c| *c == b'\n'));
            let Some(p) = nl else {
                let have = from + buf.len() as u64;
                if exhausted || have >= lc {
                    // The line at `cur` does not end within the budget. After
                    // whole lines stop at its start; as the first thing, stop
                    // inside it (everything read is examined) and let the
                    // next request resume there.
                    if cur == from && !partial {
                        self.charge_line(have - cur);
                    }
                    break if cur > from {
                        (cur, true)
                    } else {
                        (have, have < lc)
                    };
                }
                // The line at `cur` has `so_far` bytes in hand: a big line is
                // paid for by the big pool, the rest of a resumed one by the
                // page.
                let so_far = if partial { SKIPPING } else { have - cur };
                if !rd.big && !partial && so_far >= MAX_LINE {
                    // MAX_LINE bytes and no newline: big, unless the very next
                    // byte ends a line of exactly MAX_LINE.
                    if self.raw(have, 1)?.first() == Some(&b'\n') {
                        buf.push(b'\n');
                        continue;
                    }
                    // Not allowed to read it: stop in front of it, or, with
                    // nothing before it, there is no progress to be made.
                    if cur > from {
                        break (cur, true);
                    }
                    return Err(Fail::Denied);
                }
                let want = Self::line_chunk(chunk, so_far).min(lc - have);
                let n = self.pool_left(so_far).min(want);
                if n < want {
                    exhausted = true;
                }
                let got = self.raw(have, n)?;
                self.charge(so_far, got.len());
                buf.extend_from_slice(&got);
                chunk = (chunk * 2).min(CHUNK_MAX);
                continue;
            };
            let le = cur + p as u64;
            if partial {
                partial = false;
                cur = le + 1;
                continue;
            }
            let items = items_of(rd, &buf, from, cur, le);
            if !items.is_empty() && overflows(count, bytes, &items) {
                // Not consumed into this reply: the next request reads it
                // again, and is charged for it then.
                break (cur, true);
            }
            self.charge_line(le - cur);
            count += items.len();
            bytes += text_bytes(&items);
            if !items.is_empty() {
                lines.push((cur, items));
            }
            cur = le + 1;
        };
        Ok(Page {
            lines,
            start: from,
            end,
            older: from > 0,
            newer,
        })
    }
}

fn reply(agent: AgentKind, conv: &str, page: Page) -> Value {
    let items: Vec<Value> = page
        .lines
        .iter()
        .flat_map(|(off, items)| {
            items.iter().map(move |it| {
                let mut v = json!({"id": format!("{off}.{}", it.idx), "role": it.role,
                                   "text": it.text, "ts_ms": it.ts_ms});
                if !it.images.is_empty() {
                    v["images"] = it
                        .images
                        .iter()
                        .map(|m| {
                            json!({"id": pics::make_id(*off, m.ord), "type": m.kind,
                                   "size": m.size})
                        })
                        .collect();
                }
                v
            })
        })
        .collect();
    json!({
        "conv": conv, "agent": agent.as_str(), "items": items,
        "start": page.start.min(CHAT_POS_MAX), "end": page.end.min(CHAT_POS_MAX),
        "older": page.older, "newer": page.newer,
    })
}

/// Answer one `chat` request from the transcript at `path`. `conv` and `pos`
/// are the device's; a `conv` that is not this file's, or a position beyond
/// the complete lines or not at a line start, is ignored and the newest items
/// are answered. A file that cannot be read is [`none`].
///
/// This opens `path` itself and vets nothing (tests, and callers that did):
/// the app goes through [`answer`].
pub fn read(
    agent: AgentKind,
    session_id: &str,
    path: &Path,
    conv: &str,
    pos: Option<ChatPos>,
) -> Value {
    read_with(agent, session_id, path, conv, pos, false)
}

/// [`read`], listing pictures when `images` is set.
pub fn read_with(
    agent: AgentKind,
    session_id: &str,
    path: &Path,
    conv: &str,
    pos: Option<ChatPos>,
    images: bool,
) -> Value {
    let opened = File::open(path).and_then(|file| {
        let md = file.metadata()?;
        if !md.is_file() {
            return Err(std::io::Error::other("not a regular file"));
        }
        Ok(Opened {
            id: file_identity(&file),
            len: md.len(),
            path: path.to_path_buf(),
            file,
        })
    });
    opened
        .ok()
        .and_then(|o| {
            let rd = Rd {
                agent,
                images,
                big: true,
            };
            read_opened(rd, session_id, o, conv, pos, &mut 0).ok()
        })
        .unwrap_or_else(none)
}

/// Reply rules: a reply that honoured `after: e` has `start == e`, one that
/// honoured `before: s` has `end == s`; every other reply is the newest page
/// (its own `start` and `end`). A position inside a line (one this reader
/// handed out after a line too long to cross in one request) is honoured too:
/// the rest of that line is skipped unparsed.
fn read_opened(
    rd: Rd,
    session_id: &str,
    opened: Opened,
    conv: &str,
    pos: Option<ChatPos>,
    work: &mut u64,
) -> Result<Value, Fail> {
    let Opened {
        file,
        path,
        len,
        id,
    } = opened;
    let agent = rd.agent;
    let mut src = Src::new(file, len);
    let me = conv_id(agent, session_id, &path, id);
    let lc = src.last_complete_end()?;
    // A position only counts for this very conversation.
    let pos = pos.filter(|_| conv == me);
    let page = match pos {
        Some(ChatPos::After(e)) if e <= lc => {
            let partial = !src.at_line_start(e)?;
            src.page_fwd(rd, e, lc, partial)
        }
        Some(ChatPos::Before(s)) if s <= lc => {
            let partial = !src.at_line_start(s)?;
            src.page_back(rd, s, partial).map(|mut p| {
                p.newer = s < lc;
                p
            })
        }
        _ => src.page_back(rd, lc, false),
    };
    // What big lines cost this request, whether or not it succeeded.
    *work += src.work;
    Ok(reply(agent, &me, page?))
}

// ------------------------------------------------------------------- image

/// `{"result":"gone"}`: the picture cannot be had.
pub fn gone() -> Value {
    json!({"result": "gone"})
}

/// The host's answer when the request cannot be served without reading a big
/// line and the device is over its big-line work budget: the engine turns it
/// into a `busy` reply (it never reaches a device as data).
pub fn busy() -> Value {
    json!({"busy": true})
}

pub fn is_busy(v: &Value) -> bool {
    v.get("busy") == Some(&Value::Bool(true))
}

/// The `image` op: one part of one picture of the conversation a pane is on.
/// `Some({"result":"gone"})` for anything that cannot be served (no transcript,
/// another conversation, an id that is not a picture of a complete line, a
/// picture that fails the checks, a part out of range), `None` for a read
/// error ([`Engine::chat_failed`]'s `internal`).
///
/// The transcript is found and vetted exactly like [`answer`]'s. `pane` is only
/// the key of `cache`, which keeps the last decoded pictures so that the parts
/// of one picture cost one read. Callers check that picture sharing is on.
/// `work` is increased by the length of the big line (over [`MAX_LINE`]) this
/// call read, for the per-device work budget. `big`: big lines may be read; a
/// cache hit is served either way, a miss that needs a big line without it
/// answers [`busy`].
#[allow(clippy::too_many_arguments)]
pub fn image(
    b: &Binding<'_>,
    root: &Path,
    pane_cwd: &str,
    pane: &str,
    conv: &str,
    id: &str,
    part: u32,
    big: bool,
    cached: &mut Option<PathBuf>,
    cache: &mut PicCache,
    work: &mut u64,
) -> Option<Value> {
    match resolve(b, root, pane_cwd, cached) {
        Resolved::Open(opened) => image_opened(
            b.agent,
            b.session_id,
            opened,
            pane,
            conv,
            id,
            part,
            big,
            cache,
            work,
        ),
        Resolved::Nothing => Some(gone()),
        Resolved::Io => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn image_opened(
    agent: AgentKind,
    session_id: &str,
    opened: Opened,
    pane: &str,
    conv: &str,
    id: &str,
    part: u32,
    big: bool,
    cache: &mut PicCache,
    work: &mut u64,
) -> Option<Value> {
    let Opened {
        file,
        path,
        len,
        id: ident,
    } = opened;
    let me = conv_id(agent, session_id, &path, ident);
    // The pane moved on to another conversation: what was kept for it goes.
    cache.forget_other_conv(pane, &me);
    if conv != me {
        return Some(gone());
    }
    let Some((off, ord)) = pics::parse_id(id) else {
        return Some(gone());
    };
    let mut src = Src::new(file, len);
    if let Some((kind, data, stamp)) = cache.get_stamped(pane, &me, id) {
        // Only while the bytes are still in the file: it did not shrink, and
        // the line at `off` still has the length it had when it was decoded.
        match src.line_is_unchanged(off, stamp) {
            Ok(true) => {
                return Some(pics::part_reply(&me, id, kind, &data, part).unwrap_or_else(gone))
            }
            Ok(false) => cache.remove(pane, &me, id),
            Err(_) => return None,
        }
    }
    let line = src.whole_line(off, big);
    *work += src.work;
    let line = match line {
        Ok(Line::Found(l)) => l,
        Ok(Line::No) => return Some(gone()),
        Ok(Line::Denied) => return Some(busy()),
        Err(_) => return None,
    };
    let stamp = pics::Stamp {
        file_len: src.len,
        line_len: line.len() as u64,
    };
    // The same parse as the listing, so ordinal `ord` is the same picture.
    let found = with_items(agent, &line, true, |items| {
        items
            .into_iter()
            .flat_map(|i| i.pics)
            .find(|(o, _)| *o == ord)
            .and_then(|(_, p)| pics::decode(&p).map(|d| (p.kind, d)))
    })
    .flatten();
    drop(line);
    let Some((kind, data)) = found else {
        return Some(gone());
    };
    let data = std::sync::Arc::new(data);
    let reply = pics::part_reply(&me, id, kind, &data, part).unwrap_or_else(gone);
    cache.put_stamped(pane, &me, id, kind, data, stamp);
    Some(reply)
}

/// What [`Src::whole_line`] found.
enum Line {
    Found(Vec<u8>),
    /// Not a line start, no newline yet, or longer than [`IMAGE_LINE_MAX`].
    No,
    /// The line is big and the device is over its big-line budget.
    Denied,
}

impl Src {
    /// The complete line that starts at `off` (without its newline). Read in
    /// growing steps, so a short line costs a short read; a line over
    /// [`MAX_LINE`] is [`Line::Denied`] unless `big`, and is charged by its
    /// length once it is read.
    fn whole_line(&mut self, off: u64, big: bool) -> Result<Line, Fail> {
        if off >= self.len || !self.at_line_start(off)? {
            return Ok(Line::No);
        }
        let mut line: Vec<u8> = Vec::new();
        let mut chunk = CHUNK;
        loop {
            let have = off.saturating_add(line.len() as u64);
            let got = self.raw(have, chunk)?;
            chunk = (chunk * 2).min(BIG_CHUNK);
            if got.is_empty() {
                return Ok(Line::No);
            }
            if let Some(i) = got.iter().position(|c| *c == b'\n') {
                line.extend_from_slice(got.get(..i).unwrap_or_default());
                let n = line.len() as u64;
                if n > MAX_LINE && !big {
                    return Ok(Line::Denied);
                }
                self.charge_line(n);
                return Ok(if n <= IMAGE_LINE_MAX {
                    Line::Found(line)
                } else {
                    Line::No
                });
            }
            line.extend_from_slice(&got);
            let n = line.len() as u64;
            if n > MAX_LINE && !big {
                return Ok(Line::Denied);
            }
            if n > IMAGE_LINE_MAX {
                self.charge_line(n);
                return Ok(Line::No);
            }
        }
    }

    /// Is the line a cached picture came from still there as it was: the file
    /// has not shrunk, `off` still starts a line, and that line still ends
    /// where it did?
    fn line_is_unchanged(&mut self, off: u64, stamp: pics::Stamp) -> Result<bool, Fail> {
        if self.len < stamp.file_len || !self.at_line_start(off)? {
            return Ok(false);
        }
        let end = off.saturating_add(stamp.line_len);
        Ok(self.raw(end, 1)?.first() == Some(&b'\n'))
    }
}

#[cfg(test)]
mod pic_tests;
#[cfg(test)]
mod tests;
