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
//! Nothing here may panic (a release build aborts on panic): every offset sum
//! is saturating or checked, bytes are never sliced as text, and a line that
//! is not valid JSON is simply not an item.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

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
/// A line longer than this is skipped without parsing.
pub const MAX_LINE: u64 = 2 * 1024 * 1024;
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
/// One line yields at most this many items and this many bytes of text, so any
/// single line fits one reply (20 <= 40 items, 48 KiB <= 64 KiB) and paging by
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
pub fn answer(
    b: &Binding<'_>,
    root: &Path,
    pane_cwd: &str,
    conv: &str,
    pos: Option<ChatPos>,
    cached: &mut Option<PathBuf>,
) -> Option<Value> {
    let mut walked = false;
    let (opened, found) = loop {
        let path = match cached.take() {
            Some(p) => p,
            None => {
                walked = true;
                match find(b, root) {
                    Ok(Some(p)) => p,
                    Ok(None) => return Some(none()),
                    Err(_) => return None,
                }
            }
        };
        match open(b, root, &path, pane_cwd) {
            Ok(o) => break (o, path),
            Err(e) if walked => {
                return match e {
                    OpenErr::Refused => Some(none()),
                    OpenErr::Io => None,
                }
            }
            // A remembered path that no longer works: look again, once.
            Err(_) => {}
        }
    };
    *cached = Some(found);
    read_opened(b.agent, b.session_id, opened, conv, pos).ok()
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    role: &'static str,
    text: String,
    ts_ms: u64,
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

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool) == Some(true)
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
fn summarize(input: &Value) -> String {
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
            Some(Value::String(s)) if !s.trim().is_empty() => return shell_words(s),
            Some(Value::Array(a)) => {
                let words: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
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
enum Raw {
    Text(&'static str, String),
    Tool(String, String),
}

fn tool(name: &str, summary: &str) -> Raw {
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

fn block_type(b: &Value) -> Option<&str> {
    b.get("type").and_then(Value::as_str)
}

fn claude_raw(v: &Value) -> Vec<Raw> {
    if flag(v, "isSidechain") {
        return Vec::new();
    }
    let content = v.get("message").and_then(|m| m.get("content"));
    match v.get("type").and_then(Value::as_str) {
        Some("user") => {
            if flag(v, "isMeta") || flag(v, "isCompactSummary") {
                return Vec::new();
            }
            let texts: Vec<&str> = match content {
                Some(Value::String(s)) => vec![s.as_str()],
                Some(Value::Array(blocks)) => {
                    // A tool result line is the harness talking back, whatever
                    // else rides along in it.
                    if blocks.iter().any(|b| block_type(b) == Some("tool_result")) {
                        return Vec::new();
                    }
                    blocks
                        .iter()
                        .filter(|b| block_type(b) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect()
                }
                _ => return Vec::new(),
            };
            // A `!cmd` the user ran is a tool item; its output is a wrapper.
            let (shell, plain): (Vec<&str>, Vec<&str>) =
                texts.into_iter().partition(|t| bash_input(t).is_some());
            let mut out: Vec<Raw> = join_texts(AgentKind::Claude, plain.into_iter())
                .map(|t| Raw::Text("user", t))
                .into_iter()
                .collect();
            out.extend(
                shell
                    .into_iter()
                    .filter_map(bash_input)
                    .map(|c| tool("Bash", c)),
            );
            out
        }
        Some("assistant") => {
            let mut out: Vec<Raw> = Vec::new();
            let mut run: Vec<&str> = Vec::new();
            let flush = |run: &mut Vec<&str>, out: &mut Vec<Raw>| {
                if let Some(t) = join_texts(AgentKind::Claude, run.drain(..)) {
                    out.push(Raw::Text("assistant", t));
                }
            };
            match content {
                Some(Value::String(s)) => run.push(s.as_str()),
                Some(Value::Array(blocks)) => {
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(Value::as_str) {
                                    run.push(t);
                                }
                            }
                            Some("tool_use") => {
                                flush(&mut run, &mut out);
                                let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                                let input = b.get("input").unwrap_or(&Value::Null);
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

fn codex_raw(v: &Value) -> Vec<Raw> {
    if v.get("type").and_then(Value::as_str) != Some("response_item") {
        return Vec::new();
    }
    let Some(p) = v.get("payload") else {
        return Vec::new();
    };
    match p.get("type").and_then(Value::as_str) {
        Some("message") => {
            let role = match p.get("role").and_then(Value::as_str) {
                Some("user") => "user",
                Some("assistant") => "assistant",
                _ => return Vec::new(),
            };
            let Some(blocks) = p.get("content").and_then(Value::as_array) else {
                return Vec::new();
            };
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|b| {
                    matches!(
                        b.get("type").and_then(Value::as_str),
                        Some("input_text" | "output_text")
                    )
                })
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            let mut out: Vec<Raw> = join_texts(AgentKind::Codex, texts.iter().copied())
                .map(|t| Raw::Text(role, t))
                .into_iter()
                .collect();
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
            let name = p.get("name").and_then(Value::as_str).unwrap_or("");
            let summary = match p.get("arguments") {
                Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
                    Ok(args) => summarize(&args),
                    Err(_) => shell_words(s),
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
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            vec![tool("shell", &shell_argv(&words))]
        }
        Some("custom_tool_call") => {
            let name = p.get("name").and_then(Value::as_str).unwrap_or("");
            let first = p
                .get("input")
                .and_then(Value::as_str)
                .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or("");
            vec![tool(name, &shell_words(first))]
        }
        // Reasoning, outputs of calls, developer messages: never shown.
        _ => Vec::new(),
    }
}

/// The items of one transcript line, cleaned, redacted and clipped. At most
/// [`LINE_ITEMS_MAX`] items and [`LINE_BYTES_MAX`] bytes, in order.
fn line_items(agent: AgentKind, line: &[u8]) -> Vec<Item> {
    // A CRLF file leaves a `\r` on every line.
    let end = line
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    let line = &line[..end];
    if line.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_slice::<Value>(line) else {
        return Vec::new();
    };
    let ts_ms = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(rfc3339_ms)
        .unwrap_or(0);
    let raw = match agent {
        AgentKind::Claude => claude_raw(&v),
        AgentKind::Codex => codex_raw(&v),
    };
    let mut out = Vec::new();
    let mut bytes = 0usize;
    for r in raw {
        let (role, text) = match r {
            Raw::Text(role, t) => (role, clip::chat_text(&t)),
            Raw::Tool(name, summary) => ("tool", clip::chat_tool_line(&name, &summary)),
        };
        if text.trim().is_empty() {
            continue;
        }
        if out.len() >= LINE_ITEMS_MAX || bytes + text.len() > LINE_BYTES_MAX {
            break;
        }
        bytes += text.len();
        out.push(Item { role, text, ts_ms });
    }
    out
}

// ------------------------------------------------------------------ reader

#[derive(Debug)]
enum Fail {
    /// The file could not be read.
    Io,
    /// The read budget ran out.
    Budget,
}

struct Src {
    file: File,
    len: u64,
    used: u64,
}

impl Src {
    fn left(&self) -> u64 {
        MAX_READ.saturating_sub(self.used)
    }

    /// Up to `n` bytes at `off`, clamped to the file and to what is left of the
    /// budget (so the answer may be shorter than asked).
    fn read(&mut self, off: u64, n: u64) -> Result<Vec<u8>, Fail> {
        let n = n.min(self.left());
        let buf = self.raw(off, n)?;
        self.used += buf.len() as u64;
        Ok(buf)
    }

    /// Up to `n` bytes at `off`, clamped to the file only: not charged to the
    /// page budget (the callers are the end-of-file scan and one-byte probes).
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
/// from `base`. A line over [`MAX_LINE`] is not looked at.
fn items_of(agent: AgentKind, buf: &[u8], base: u64, ls: u64, le: u64) -> Vec<Item> {
    if le.saturating_sub(ls) > MAX_LINE {
        return Vec::new();
    }
    let (Ok(a), Ok(b)) = (
        usize::try_from(ls.saturating_sub(base)),
        usize::try_from(le.saturating_sub(base)),
    ) else {
        return Vec::new();
    };
    buf.get(a..b)
        .map(|l| line_items(agent, l))
        .unwrap_or_default()
}

impl Src {
    /// The lines just before `hi` (a line start), newest first scanned, from
    /// the end of the file towards its start.
    ///
    /// `partial`: `hi` is inside a line (a resumed position), whose head is
    /// skipped unparsed.
    fn page_back(&mut self, agent: AgentKind, hi: u64, mut partial: bool) -> Result<Page, Fail> {
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
                // unread part (the bytes nearest to `lo`).
                let want = chunk.min(lo);
                let n = want.min(self.left());
                if n < want {
                    exhausted = true;
                }
                if n == 0 {
                    break None;
                }
                let mut got = self.read(lo - n, n)?;
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
                break if pe < hi { (true, pe) } else { (lo > 0, lo) };
            };
            if partial {
                partial = false;
                pe = ls;
                continue;
            }
            let items = items_of(agent, &buf, lo, ls, pe - 1);
            if !items.is_empty() && overflows(count, bytes, &items) {
                break (true, pe);
            }
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
    fn page_fwd(
        &mut self,
        agent: AgentKind,
        from: u64,
        lc: u64,
        mut partial: bool,
    ) -> Result<Page, Fail> {
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
                    break if cur > from {
                        (cur, true)
                    } else {
                        (have, have < lc)
                    };
                }
                let n = chunk.min(lc - have);
                let got = self.read(have, n)?;
                if (got.len() as u64) < n {
                    exhausted = true;
                }
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
            let items = items_of(agent, &buf, from, cur, le);
            if !items.is_empty() && overflows(count, bytes, &items) {
                break (cur, true);
            }
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
            items.iter().enumerate().map(move |(i, it)| {
                json!({"id": format!("{off}.{i}"), "role": it.role, "text": it.text,
                       "ts_ms": it.ts_ms})
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
        .and_then(|o| read_opened(agent, session_id, o, conv, pos).ok())
        .unwrap_or_else(none)
}

/// Reply rules: a reply that honoured `after: e` has `start == e`, one that
/// honoured `before: s` has `end == s`; every other reply is the newest page
/// (its own `start` and `end`). A position inside a line (one this reader
/// handed out after a line too long to cross in one request) is honoured too:
/// the rest of that line is skipped unparsed.
fn read_opened(
    agent: AgentKind,
    session_id: &str,
    opened: Opened,
    conv: &str,
    pos: Option<ChatPos>,
) -> Result<Value, Fail> {
    let Opened {
        file,
        path,
        len,
        id,
    } = opened;
    let mut src = Src { file, len, used: 0 };
    let me = conv_id(agent, session_id, &path, id);
    let lc = src.last_complete_end()?;
    // A position only counts for this very conversation.
    let pos = pos.filter(|_| conv == me);
    match pos {
        Some(ChatPos::After(e)) if e <= lc => {
            let partial = !src.at_line_start(e)?;
            let page = src.page_fwd(agent, e, lc, partial)?;
            return Ok(reply(agent, &me, page));
        }
        Some(ChatPos::Before(s)) if s <= lc => {
            let partial = !src.at_line_start(s)?;
            let mut page = src.page_back(agent, s, partial)?;
            page.newer = s < lc;
            return Ok(reply(agent, &me, page));
        }
        _ => {}
    }
    let page = src.page_back(agent, lc, false)?;
    Ok(reply(agent, &me, page))
}

#[cfg(test)]
mod tests;
