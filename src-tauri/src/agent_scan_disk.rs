//! Finding an agent's session id by reading the transcripts the CLI keeps for
//! itself — the baseline source, because it works for a user who never turned
//! agent tracking on (spec §2). Hooks are faster and exact when present; this
//! is what makes resume work without them.
//!
//! Tauri-free, so the parsers *and* the directory walk are covered by
//! `cargo test --no-default-features --lib -p ymux` on Linux CI (rule 1). The
//! walk takes its root directory as an argument, exactly as
//! `scrollback::save_blob_under` does, so its tests build a fake transcript
//! tree in a temp directory and never read the developer's real `~/.claude`.
//!
//! ## What the formats actually are
//!
//! Both were checked against the real data on a machine with 24 Claude
//! transcripts and 847 Codex rollouts; the surprises are documented at each
//! parser.
//!
//! ## The scan is bounded
//!
//! 847 rollout files is normal and parsing them all on every pane spawn is
//! not. The walk only ever *stats* directory entries, then reads the head of
//! at most [`MAX_CANDIDATES`] files whose mtime is inside the freshness
//! window — on the machine this was written on, 3 of 847.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::agent_sessions::{is_valid_session_id, AgentKind, FRESH_WINDOW};

/// Directory entries the walk will visit before giving up. A guard against a
/// pathological `~/.codex/sessions` (or a symlink loop), not a tuning knob:
/// the real tree has a few hundred.
pub const MAX_DIR_ENTRIES: usize = 20_000;

/// Transcripts whose head is actually read and parsed, after the mtime filter
/// has run and the survivors have been sorted newest-first.
pub const MAX_CANDIDATES: usize = 64;

/// Bytes read from one transcript. Codex embeds its full system prompt in the
/// `session_meta` line — the longest first line across the 847 rollouts here
/// is 44 KB — so the cap has room, but a corrupt file with no newline at all
/// still cannot pull an unbounded read into memory.
pub const MAX_HEAD_BYTES: usize = 256 * 1024;

/// Lines read from a Claude transcript while looking for its `cwd`. The field
/// appears on the first *conversation* record, after a handful of small
/// metadata lines; across every transcript here it was at index 2–5.
pub const MAX_HEAD_LINES: usize = 40;

/// What a transcript scan found for one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskSession {
    /// The id to hand to the CLI's resume command.
    pub session_id: String,
    /// The cwd as the transcript itself recorded it (raw spelling).
    pub cwd: String,
    pub modified: SystemTime,
    /// When the conversation began, in seconds since the Unix epoch, as the
    /// transcript itself records it — the first top-level `timestamp` of a
    /// Claude transcript, `session_meta.payload.timestamp` of a Codex rollout.
    ///
    /// Deliberately *not* the file's creation time. Claude rewrites the head
    /// of a live transcript (the `last-prompt` / `mode` / `ai-title` records
    /// sit above the first conversation record, and move as the session
    /// goes on), so a file's birth time is the last rewrite, not the start of
    /// the conversation. The recorded timestamp is written once and carried
    /// along. `None` when the head has none; such a transcript can never be
    /// bound by timestamp (see `agent_binding`).
    pub created: Option<u64>,
}

// ---------------------------------------------------------------------------
// Claude: ~/.claude/projects/<mangled cwd>/<session-id>.jsonl
// ---------------------------------------------------------------------------

/// Claude Code's project-directory name for `cwd`.
///
/// The rule is `cwd.replace(/[^a-zA-Z0-9]/g, "-")` — lifted verbatim from the
/// installed CLI binary, not guessed:
///
/// ```text
/// function O(e){let r=e.replace(/[^a-zA-Z0-9]/g,"-");if(r.length<=C)return r;
///               return `${r.slice(0,C)}-${Math.abs(xJ(e)).toString(36)}`}   // C = 200
/// ```
///
/// So **every** character outside `[A-Za-z0-9]` becomes one `-`, not just the
/// separators: `D:\Git\TouchDesigner\2609_Exhibition` is stored under
/// `D--Git-TouchDesigner-2609-Exhibition`, with the underscore folded too.
/// The regex has no `/u` flag, so JS applies it per UTF-16 code unit and an
/// astral character becomes *two* dashes; [`mangle`] matches that.
///
/// Past 200 characters Claude truncates and appends a hash of the original,
/// so the name is not reconstructible; [`claude_dir_key`] handles that by
/// comparing only the part before the truncation point, and the in-file `cwd`
/// check below is what actually decides a match.
pub fn claude_project_dir_name(cwd: &str) -> String {
    mangle(cwd)
}

/// Truncation point Claude applies to a project directory name.
const CLAUDE_DIR_NAME_CAP: usize = 200;

fn mangle(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            // One dash per UTF-16 code unit, because the JS regex is not
            // Unicode-aware: a non-BMP character is a surrogate pair there.
            for _ in 0..c.len_utf16() {
                out.push('-');
            }
        }
    }
    out
}

/// A comparison key for a Claude project directory name.
///
/// Mangling is lossy in two ways that this key absorbs, leaving the in-file
/// `cwd` check to make the real decision:
///
/// * **Runs of dashes are not countable.** macOS reports decomposed (NFD)
///   filenames, and `한` is one code unit composed but three decomposed —
///   so the same directory mangles to a different number of dashes depending
///   on which side spelled it. Runs collapse to one.
/// * **Case.** `D:\Git\ymux` and `d:\git\ymux` are one directory that mangles
///   to two names; `/srv/A` and `/srv/a` are two directories that must not be
///   merged. Which rule applies is a property of the path's *syntax*, so the
///   answer comes from `ypath::is_windows_path_like`, never `cfg!(windows)`
///   (rule 15). Mangled names are pure ASCII, so ASCII case folding is exact.
///
/// `windows_syntax` comes from the *path* being matched, not from the
/// directory name (which no longer has a syntax to inspect).
pub fn claude_dir_key(dir_name: &str, windows_syntax: bool) -> String {
    let mut out = String::with_capacity(dir_name.len());
    let mut last_dash = false;
    for c in dir_name.chars().take(CLAUDE_DIR_NAME_CAP) {
        if c == '-' {
            if !last_dash {
                out.push('-');
            }
            last_dash = true;
        } else {
            out.push(if windows_syntax {
                c.to_ascii_lowercase()
            } else {
                c
            });
            last_dash = false;
        }
    }
    out
}

/// Whether `dir_name` is plausibly the project directory for `cwd`. A cheap
/// index into the projects tree — every hit is confirmed against the `cwd`
/// recorded inside the transcript before it is believed.
pub fn claude_dir_matches(cwd: &str, dir_name: &str) -> bool {
    let win = ypath::is_windows_path_like(cwd);
    claude_dir_key(&claude_project_dir_name(cwd), win) == claude_dir_key(dir_name, win)
}

/// What a Claude transcript's head says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClaudeMeta {
    pub cwd: Option<String>,
    /// `"cli"` for a terminal session, `"claude-desktop"` for the desktop app.
    /// Only a `cli` session can have been the thing running in a ymux pane.
    pub entrypoint: Option<String>,
    /// The first top-level `timestamp` in the head, in epoch seconds.
    pub created: Option<u64>,
}

/// Parse the head of a Claude transcript (JSONL). Stops as soon as all three
/// fields are known, and ignores any line that is not a JSON object — the
/// first few records are small control entries (`last-prompt`, `mode`,
/// `permission-mode`, `atis-latch`) that carry neither field.
pub fn parse_claude_head(head: &str) -> ClaudeMeta {
    let mut meta = ClaudeMeta::default();
    for line in head.lines().take(MAX_HEAD_LINES) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if meta.cwd.is_none() {
            if let Some(s) = v.get("cwd").and_then(|c| c.as_str()) {
                if !s.is_empty() {
                    meta.cwd = Some(s.to_string());
                }
            }
        }
        if meta.entrypoint.is_none() {
            if let Some(s) = v.get("entrypoint").and_then(|c| c.as_str()) {
                meta.entrypoint = Some(s.to_string());
            }
        }
        if meta.created.is_none() {
            meta.created = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(parse_rfc3339_secs);
        }
        if meta.cwd.is_some() && meta.entrypoint.is_some() && meta.created.is_some() {
            break;
        }
    }
    meta
}

// ---------------------------------------------------------------------------
// Codex: ~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl
// ---------------------------------------------------------------------------

/// What a Codex rollout's `session_meta` line says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexMeta {
    pub id: String,
    pub cwd: String,
    /// `payload.source`: the string `"cli"`, `"exec"` or `"vscode"`, or — for
    /// a subagent rollout — the object `{"subagent": {…}}`, which reads back
    /// here as `None`.
    pub source: Option<String>,
    /// `payload.timestamp` (falling back to the line's own `timestamp`), in
    /// epoch seconds: when the session was created. A resumed session keeps
    /// appending to its original rollout, so this stays the original date.
    pub created: Option<u64>,
}

/// Parse a Codex rollout's first line.
///
/// Two things the spec's table does not say, both found by surveying all 847
/// rollouts on this machine:
///
/// * **`payload.session_id` is the *thread root*, not this file's id.** For a
///   subagent rollout it is the parent's id, so 561 files here would resume
///   their parent. `payload.id` always equals the id in the filename; that is
///   the one to use. On 145 older `codex_exec` rollouts `session_id` is
///   absent entirely, which is why it is only a fallback.
/// * **`payload.source` is the reliable discriminator, and it is not always a
///   string.** It is `{"subagent": …}` for every one of the 561 subagent
///   rollouts and a plain string otherwise. Checking `session_id == id`
///   instead would be wrong in both directions here: 110 subagent rollouts
///   satisfy it and 145 non-subagent ones do not.
pub fn parse_codex_meta(first_line: &str) -> Option<CodexMeta> {
    let v: serde_json::Value = serde_json::from_str(first_line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    let p = v.get("payload")?;
    let id = p
        .get("id")
        .and_then(|x| x.as_str())
        .or_else(|| p.get("session_id").and_then(|x| x.as_str()))?
        .to_string();
    let cwd = p.get("cwd").and_then(|x| x.as_str())?.to_string();
    if cwd.is_empty() {
        return None;
    }
    let created = p
        .get("timestamp")
        .or_else(|| v.get("timestamp"))
        .and_then(|t| t.as_str())
        .and_then(parse_rfc3339_secs);
    Some(CodexMeta {
        id,
        cwd,
        source: p.get("source").and_then(|x| x.as_str()).map(str::to_string),
        created,
    })
}

/// Whether this rollout is one a ymux pane could have been running.
///
/// Only `source == "cli"` — the interactive TUI. `"exec"` is `codex exec`
/// (non-interactive), `"vscode"` and the `Codex Desktop` originator are other
/// front ends, and the object-valued `source` is a subagent thread. Resuming
/// any of those in a pane would drop the user into a conversation they never
/// had in that terminal.
pub fn codex_is_resumable(meta: &CodexMeta) -> bool {
    meta.source.as_deref() == Some("cli") && is_valid_session_id(&meta.id)
}

// ---------------------------------------------------------------------------
// The bounded walk
// ---------------------------------------------------------------------------

/// Read at most `MAX_HEAD_BYTES` from `path`, stopping early at `max_lines`
/// complete lines. Any IO error is "no transcript here", not a failure: the
/// agent may be rewriting the file right now.
fn read_head(path: &Path, max_lines: usize) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file.take(MAX_HEAD_BYTES as u64));
    let mut out = String::new();
    for _ in 0..max_lines {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => out.push_str(&line),
        }
    }
    Some(out)
}

/// One candidate transcript: its path and mtime, before anything is read.
struct Candidate {
    path: PathBuf,
    modified: SystemTime,
}

/// Collect `.jsonl` files under `dir` (non-recursive) whose mtime is within
/// `window` of `now`, newest first, at most `MAX_CANDIDATES`.
///
/// Non-recursive on purpose for Claude: a session's *subagent* transcripts
/// live in `<session-id>/subagents/agent-*.jsonl`, and none of those is a
/// conversation a user can resume. Recursing would offer 45 candidates in
/// this repo's project directory instead of 1.
fn candidates_in(
    dir: &Path,
    now: SystemTime,
    window: Duration,
    budget: &mut usize,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if now.duration_since(modified).unwrap_or_default() > window {
            continue;
        }
        out.push(Candidate { path, modified });
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.modified));
    out.truncate(MAX_CANDIDATES);
    out
}

/// Every resumable Claude transcript for `cwd` under a `.claude/projects`
/// root, newest (by mtime) first.
///
/// The project directory name is derived from `cwd`, so this is one
/// `read_dir` of the projects root (22 entries here) plus one of the matching
/// project directory. Every candidate is confirmed against the `cwd` recorded
/// *inside* the transcript with `ypath::same_path` (rule 15) — mangling folds
/// `_`, `.` and `-` together, so the directory name alone cannot prove a
/// match.
///
/// This only says which transcripts *could* belong to a pane in `cwd`; which
/// one actually does is `agent_binding`'s decision, made against the agent
/// process running in the pane. "The newest one here" is not an answer: it
/// is whichever conversation anyone touched last in that directory.
pub fn claude_candidates(projects_root: &Path, cwd: &str, now: SystemTime) -> Vec<DiskSession> {
    let mut budget = MAX_DIR_ENTRIES;
    let mut dirs: Vec<PathBuf> = Vec::new();
    let Ok(entries) = std::fs::read_dir(projects_root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if claude_dir_matches(cwd, name) {
            dirs.push(entry.path());
        }
    }
    let mut all: Vec<Candidate> = Vec::new();
    for dir in dirs {
        all.extend(candidates_in(&dir, now, FRESH_WINDOW, &mut budget));
    }
    all.sort_by_key(|c| std::cmp::Reverse(c.modified));
    all.truncate(MAX_CANDIDATES);
    let mut out = Vec::new();
    for c in all {
        let Some(stem) = c.path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_valid_session_id(stem) {
            continue;
        }
        let Some(head) = read_head(&c.path, MAX_HEAD_LINES) else {
            continue;
        };
        let meta = parse_claude_head(&head);
        // A transcript with no recorded cwd cannot be confirmed, and one
        // started from the desktop app was never in a terminal.
        let Some(found_cwd) = meta.cwd else { continue };
        if meta.entrypoint.as_deref() != Some("cli") {
            continue;
        }
        if !ypath::same_path(cwd, &found_cwd) {
            continue;
        }
        out.push(DiskSession {
            session_id: stem.to_string(),
            cwd: found_cwd,
            modified: c.modified,
            created: meta.created,
        });
    }
    out
}

/// Every resumable Codex rollout for `cwd` under a `.codex/sessions` root,
/// newest (by mtime) first. See [`claude_candidates`] for why this is a list.
///
/// The tree is `YYYY/MM/DD/`, but the date directory is *not* a usable filter:
/// 44 of the 847 rollouts here have an mtime on a later day than their
/// directory, because a resumed session keeps appending to its original file.
/// Narrowing by directory date would therefore skip exactly the sessions most
/// worth resuming. Instead every day directory is listed — `read_dir` only,
/// no file is opened — and the mtime window does the narrowing: 3 of 847 here.
pub fn codex_candidates(sessions_root: &Path, cwd: &str, now: SystemTime) -> Vec<DiskSession> {
    let mut budget = MAX_DIR_ENTRIES;
    let mut all: Vec<Candidate> = Vec::new();
    // YYYY / MM / DD
    let mut level: Vec<PathBuf> = vec![sessions_root.to_path_buf()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for dir in &level {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    next.push(entry.path());
                }
            }
        }
        level = next;
    }
    for dir in &level {
        all.extend(candidates_in(dir, now, FRESH_WINDOW, &mut budget));
    }
    all.sort_by_key(|c| std::cmp::Reverse(c.modified));
    all.truncate(MAX_CANDIDATES);

    let mut out = Vec::new();
    for c in all {
        let Some(head) = read_head(&c.path, 1) else {
            continue;
        };
        let Some(meta) = parse_codex_meta(head.trim_end()) else {
            continue;
        };
        if !codex_is_resumable(&meta) || !ypath::same_path(cwd, &meta.cwd) {
            continue;
        }
        out.push(DiskSession {
            session_id: meta.id,
            cwd: meta.cwd,
            modified: c.modified,
            created: meta.created,
        });
    }
    out
}

/// Seconds since the Unix epoch for an RFC 3339 timestamp as both CLIs write
/// them (`2026-09-18T12:31:01.889Z`, or with a `±HH:MM` offset). Fractional
/// seconds are dropped — rounding *down* — which is the safe direction for
/// "was this conversation created at or after that process started?": the
/// process start time is whole seconds rounded down too, so a transcript
/// written after the process began can never compare as earlier.
///
/// Hand-rolled because the crate has no date library and this is the only
/// place that needs one. `None` for anything it does not fully understand.
pub fn parse_rfc3339_secs(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = s.get(r)?;
        if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, min, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    // Skip fractional seconds, then read the zone.
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
    }
    let offset = match *b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let off = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
            if sign == b'+' {
                off
            } else {
                -off
            }
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + min * 60 + sec - offset;
    u64::try_from(secs).ok()
}

/// `~/.claude/projects`, or `None` if the OS has no home directory.
pub fn claude_projects_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// `~/.codex/sessions`.
pub fn codex_sessions_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
}

/// Every transcript for `agent` in `cwd` that a pane could be running,
/// against the real transcript roots in the user's home directory.
pub fn candidate_sessions(agent: AgentKind, cwd: &str) -> Vec<DiskSession> {
    let root = match agent {
        AgentKind::Claude => claude_projects_root(),
        AgentKind::Codex => codex_sessions_root(),
    };
    let Some(root) = root else {
        return Vec::new();
    };
    let now = SystemTime::now();
    match agent {
        AgentKind::Claude => claude_candidates(&root, cwd, now),
        AgentKind::Codex => codex_candidates(&root, cwd, now),
    }
}

/// `~/.claude/sessions`: one `<pid>.json` per running interactive Claude.
pub fn claude_pid_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("sessions"))
}

/// Largest pid file believed. The real ones are under 1 KB.
const MAX_PID_FILE_BYTES: u64 = 64 * 1024;

/// Read `<dir>/<pid>.json` — by name, never by listing the directory. Missing,
/// oversized or unparseable is `None`: the file is undocumented, so anything
/// unexpected means "don't know".
pub fn read_claude_pid_file(dir: &Path, pid: u32) -> Option<crate::agent_binding::ClaudePidFile> {
    let path = dir.join(format!("{pid}.json"));
    if std::fs::metadata(&path).ok()?.len() > MAX_PID_FILE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    crate::agent_binding::parse_claude_pid_file(&text).filter(|f| f.pid == pid)
}

/// The session id Claude's own pid file names for `proc`, when it is
/// believable (`agent_binding::registry_session_id`).
pub fn claude_pid_file_id(proc: &crate::agent_binding::PaneProcess) -> Option<String> {
    let file = read_claude_pid_file(&claude_pid_dir()?, proc.pid)?;
    crate::agent_binding::registry_session_id(proc, &file)
}

/// Directory entries one existence check may visit. The check runs as a pane
/// spawns, so it is bounded well below [`MAX_DIR_ENTRIES`]; the fast paths
/// below make the typical answer a single `stat`.
pub const MAX_EXISTS_ENTRIES: usize = 5_000;

/// Whether the transcript for `session_id` is still on disk.
///
/// A *search by id*, not only a path rebuilt from the record's `cwd`. Two
/// reasons, one per agent:
///
/// * Claude's directory name is derived from the cwd, and the spelling in the
///   record came from whatever produced it — a hook-borne record carries the
///   shell's OSC 7 spelling, which from Git Bash is `/d/Git/ymux`, not
///   `D:\Git\ymux`. So the directory `cwd_hint` mangles to is tried first
///   (one `stat`, and right for every disk-scanned record), and only then
///   every project directory.
/// * Codex's filename embeds a timestamp *before* the id, so there is no path
///   to rebuild at all; and asking "is this still the newest session here?"
///   would decline a perfectly good resume as soon as any later Codex run
///   touched the same directory. The date tree is walked newest first, since
///   a resumable session was active in the last day.
///
/// Both read directory entries only and open nothing, and both give up
/// (answering "gone") after `budget` entries.
pub fn transcript_exists_within(
    agent: AgentKind,
    root: &Path,
    session_id: &str,
    cwd_hint: &str,
    mut budget: usize,
) -> bool {
    if !is_valid_session_id(session_id) {
        return false;
    }
    let file_name = format!("{session_id}.jsonl");
    match agent {
        AgentKind::Claude => {
            if !cwd_hint.is_empty()
                && root
                    .join(claude_project_dir_name(cwd_hint))
                    .join(&file_name)
                    .is_file()
            {
                return true;
            }
            let Ok(entries) = std::fs::read_dir(root) else {
                return false;
            };
            for entry in entries.flatten() {
                if budget == 0 {
                    return false;
                }
                budget -= 1;
                if entry.path().join(&file_name).is_file() {
                    return true;
                }
            }
            false
        }
        AgentKind::Codex => {
            let suffix = format!("-{file_name}");
            codex_find(root, 0, &suffix, &mut budget)
        }
    }
}

/// Depth-first over `YYYY/MM/DD/rollout-*.jsonl`, newest name first at every
/// level.
fn codex_find(dir: &Path, depth: usize, suffix: &str, budget: &mut usize) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if depth == 3 {
            if name.ends_with(suffix) {
                return true;
            }
        } else if entry.file_type().is_ok_and(|t| t.is_dir()) {
            names.push((name, entry.path()));
        }
    }
    names.sort_by(|a, b| b.0.cmp(&a.0));
    names
        .iter()
        .any(|(_, path)| codex_find(path, depth + 1, suffix, budget))
}

/// [`transcript_exists_within`] with the default budget.
pub fn transcript_exists_under(
    agent: AgentKind,
    root: &Path,
    session_id: &str,
    cwd_hint: &str,
) -> bool {
    transcript_exists_within(agent, root, session_id, cwd_hint, MAX_EXISTS_ENTRIES)
}

/// [`transcript_exists_under`] against the real roots in the user's home
/// directory. A machine with no home directory has no transcripts either, so
/// nothing is resumable there.
pub fn transcript_exists(agent: AgentKind, session_id: &str, cwd_hint: &str) -> bool {
    let root = match agent {
        AgentKind::Claude => claude_projects_root(),
        AgentKind::Codex => codex_sessions_root(),
    };
    root.is_some_and(|r| transcript_exists_under(agent, &r, session_id, cwd_hint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// No session ids claimed by other panes.
    fn none() -> HashSet<String> {
        HashSet::new()
    }

    /// The newest unclaimed candidate — what these ordering and filtering
    /// tests look at. Which candidate a pane actually gets is decided in
    /// `agent_binding`, against the process running in it.
    fn newest_claude_session(
        root: &Path,
        cwd: &str,
        now: SystemTime,
        claimed: &HashSet<String>,
    ) -> Option<DiskSession> {
        claude_candidates(root, cwd, now)
            .into_iter()
            .find(|d| !claimed.contains(&d.session_id))
    }

    fn newest_codex_session(
        root: &Path,
        cwd: &str,
        now: SystemTime,
        claimed: &HashSet<String>,
    ) -> Option<DiskSession> {
        codex_candidates(root, cwd, now)
            .into_iter()
            .find(|d| !claimed.contains(&d.session_id))
    }

    #[test]
    fn rfc3339_timestamps_parse_to_epoch_seconds() {
        // 2026-09-18T12:31:01Z = 1789734661 (`date -u -d … +%s`).
        assert_eq!(
            parse_rfc3339_secs("2026-09-18T12:31:01.889Z"),
            Some(1_789_734_661)
        );
        assert_eq!(
            parse_rfc3339_secs("2026-09-18T12:31:01Z"),
            Some(1_789_734_661)
        );
        assert_eq!(
            parse_rfc3339_secs("2026-09-18T21:31:01+09:00"),
            Some(1_789_734_661),
            "an offset is applied"
        );
        assert_eq!(parse_rfc3339_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_secs("2000-02-29T00:00:00Z"),
            Some(951_782_400)
        );
        for bad in [
            "",
            "yesterday",
            "2026-09-18",
            "2026-09-18T12:31:01",
            "2026-13-18T12:31:01Z",
            "2026-09-18T12:31:01.Z",
            "2026-09-18T12:31:01Zjunk",
        ] {
            assert_eq!(parse_rfc3339_secs(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn claude_head_reports_when_the_conversation_began() {
        // The control records at the top carry no timestamp; the first one
        // that does is the conversation's first record.
        let head = concat!(
            r#"{"type":"ai-title","aiTitle":"x","sessionId":"20aebce7"}"#,
            "\n",
            r#"{"type":"mode","mode":"normal","sessionId":"20aebce7"}"#,
            "\n",
            r#"{"parentUuid":null,"type":"attachment","timestamp":"2026-09-18T12:31:01.889Z","entrypoint":"cli","cwd":"/w"}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-09-18T12:40:00.000Z"}"#,
            "\n",
        );
        assert_eq!(parse_claude_head(head).created, Some(1_789_734_661));
    }

    #[test]
    fn codex_meta_reports_when_the_session_began() {
        let line = r#"{"timestamp":"2026-09-23T01:54:14.590Z","type":"session_meta","payload":{"id":"01a07644-42b3-7183-a7fd-70379b88af1f","timestamp":"2026-09-23T01:54:14.479Z","cwd":"/w","source":"cli"}}"#;
        let meta = parse_codex_meta(line).expect("parses");
        assert_eq!(meta.created, parse_rfc3339_secs("2026-09-23T01:54:14Z"));
    }

    fn tempdir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ymux-scan-disk-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("mkdir");
        base
    }

    /// Every one of these pairs is a real directory in `~/.claude/projects` on
    /// the machine this was written on, checked against the `cwd` recorded
    /// inside its own transcripts.
    #[test]
    fn claude_dir_name_matches_real_directories() {
        for (cwd, dir) in [
            ("D:\\Git\\ymux", "D--Git-ymux"),
            ("C:\\Git\\ScoreServer", "C--Git-ScoreServer"),
            (
                "C:\\Git\\Unity\\CubeStealthEscape",
                "C--Git-Unity-CubeStealthEscape",
            ),
            ("D:\\Git\\Project\\Tactica", "D--Git-Project-Tactica"),
            (
                "D:\\Git\\Project\\clicktolink",
                "D--Git-Project-clicktolink",
            ),
            (
                "D:\\git\\Project\\AutoThreads",
                "D--git-Project-AutoThreads",
            ),
            // The one that proves it is not just the separators: the `_` in
            // `2609_Exhibition` is folded to `-` too.
            (
                "D:\\Git\\TouchDesigner\\2609_Exhibition",
                "D--Git-TouchDesigner-2609-Exhibition",
            ),
        ] {
            assert_eq!(claude_project_dir_name(cwd), dir, "mangling {cwd}");
            assert!(claude_dir_matches(cwd, dir), "matching {cwd}");
        }
    }

    #[test]
    fn claude_mangling_folds_dots_spaces_and_forward_slashes() {
        assert_eq!(
            claude_project_dir_name("/home/alice/my project/.config"),
            "-home-alice-my-project--config"
        );
        assert_eq!(claude_project_dir_name("C:/Git/a.b"), "C--Git-a-b");
    }

    #[test]
    fn claude_mangling_is_per_utf16_code_unit() {
        // The CLI's regex has no `/u` flag, so JS walks UTF-16 code units and
        // an astral character (here U+1F600) becomes two dashes, not one.
        assert_eq!(claude_project_dir_name("a\u{1F600}b"), "a--b");
        // A BMP non-ASCII character is one code unit, so one dash.
        assert_eq!(claude_project_dir_name("a\u{AC00}b"), "a-b");
    }

    #[test]
    fn claude_dir_matching_folds_case_only_for_windows_paths() {
        // Rule 15: a drive path is one directory however it is cased.
        assert!(claude_dir_matches("D:\\Git\\ymux", "d--git-ymux"));
        assert!(claude_dir_matches("d:/git/YMUX", "D--Git-ymux"));
        // A POSIX path is not: `/srv/A` and `/srv/a` are two directories, and
        // folding them would silently merge distinct projects.
        assert!(claude_dir_matches("/srv/A", "-srv-A"));
        assert!(!claude_dir_matches("/srv/A", "-srv-a"));
    }

    #[test]
    fn claude_dir_matching_absorbs_nfd_vs_nfc() {
        // macOS reports decomposed filenames. "한" is one UTF-16 code unit
        // composed and three decomposed, so the same directory mangles to a
        // different number of dashes depending on who spelled it. The key
        // collapses dash runs; the in-file `cwd` check decides the rest.
        let nfc = "D:\\Git\\\u{D55C}\u{AE00}";
        let nfd = "D:\\Git\\\u{1112}\u{1161}\u{AB03}";
        let dir_from_nfc = claude_project_dir_name(nfc);
        assert!(
            claude_dir_matches(nfd, &dir_from_nfc),
            "{nfd} should match the directory {dir_from_nfc} spelled from NFC"
        );
    }

    #[test]
    fn parse_claude_head_reads_a_real_transcript_head() {
        // Verbatim shape of a real `~/.claude/projects/D--Git-ymux/*.jsonl`
        // head: four control records with no `cwd`, then the first
        // conversation record that carries both fields.
        let head = concat!(
            r#"{"type":"last-prompt","leafUuid":"f76b0648","sessionId":"20aebce7"}"#,
            "\n",
            r#"{"type":"mode","mode":"normal","sessionId":"20aebce7"}"#,
            "\n",
            r#"{"type":"permission-mode","permissionMode":"bypassPermissions","sessionId":"20aebce7"}"#,
            "\n",
            r#"{"type":"atis-latch","atis":"43c31c4f","sessionId":"20aebce7"}"#,
            "\n",
            r#"{"parentUuid":null,"type":"attachment","timestamp":"2026-09-18T12:31:01.889Z","userType":"external","entrypoint":"cli","cwd":"D:\\Git\\ymux","sessionId":"20aebce7","version":"2.1.276","gitBranch":"main"}"#,
            "\n",
        );
        let meta = parse_claude_head(head);
        assert_eq!(meta.cwd.as_deref(), Some("D:\\Git\\ymux"));
        assert_eq!(meta.entrypoint.as_deref(), Some("cli"));
    }

    #[test]
    fn parse_claude_head_survives_junk_and_missing_fields() {
        assert_eq!(parse_claude_head(""), ClaudeMeta::default());
        assert_eq!(
            parse_claude_head("not json\n{\"type\":\"mode\"}\n"),
            ClaudeMeta::default()
        );
    }

    #[test]
    fn parse_codex_meta_reads_a_real_session_meta_line() {
        // Trimmed from a real `~/.codex/sessions/2026/09/23/rollout-*.jsonl`.
        let line = r#"{"timestamp":"2026-09-23T01:54:14.590Z","ordinal":0,"type":"session_meta","payload":{"session_id":"01a07644-42b3-7183-a7fd-70379b88af1f","id":"01a07644-42b3-7183-a7fd-70379b88af1f","timestamp":"2026-09-23T01:54:14.479Z","cwd":"C:\\Git\\Unity\\CubeStealthEscape","originator":"codex-tui","cli_version":"0.155.1","source":"cli","thread_source":"user"}}"#;
        let meta = parse_codex_meta(line).expect("real session_meta must parse");
        assert_eq!(meta.id, "01a07644-42b3-7183-a7fd-70379b88af1f");
        // Rule 15's point: the field really is a Windows path with backslashes.
        assert_eq!(meta.cwd, "C:\\Git\\Unity\\CubeStealthEscape");
        assert_eq!(meta.source.as_deref(), Some("cli"));
        assert!(codex_is_resumable(&meta));
    }

    #[test]
    fn a_codex_subagent_rollout_is_not_resumable() {
        // The real shape: `source` is an object, and `session_id` is the
        // *parent* thread — resuming it would drop the user into someone
        // else's conversation. 561 of the 847 rollouts here look like this.
        let line = r#"{"timestamp":"2026-09-23T01:54:14.590Z","type":"session_meta","payload":{"session_id":"01a07644-42b3-7183-a7fd-70379b88af1f","id":"01a0cbf8-7b21-7d50-b2b3-1cef6aba1698","parent_thread_id":"01a07644-42b3-7183-a7fd-70379b88af1f","cwd":"C:\\Git\\Unity\\CubeStealthEscape","originator":"codex-tui","source":{"subagent":{"depth":1}},"thread_source":"subagent"}}"#;
        let meta = parse_codex_meta(line).expect("parses");
        // `payload.id`, never `payload.session_id`.
        assert_eq!(meta.id, "01a0cbf8-7b21-7d50-b2b3-1cef6aba1698");
        assert_eq!(meta.source, None, "an object source reads back as None");
        assert!(!codex_is_resumable(&meta));
    }

    #[test]
    fn a_codex_exec_rollout_is_not_resumable() {
        // `codex exec` is non-interactive; 167 rollouts here. It also has no
        // `session_id` at all, which is why `payload.id` is the primary.
        let line = r#"{"type":"session_meta","payload":{"id":"019d83fd-05f3-7282-a670-9f09375875b7","cwd":"D:\\Git\\ymux","originator":"codex_exec","source":"exec"}}"#;
        let meta = parse_codex_meta(line).expect("parses");
        assert_eq!(meta.id, "019d83fd-05f3-7282-a670-9f09375875b7");
        assert!(!codex_is_resumable(&meta));
    }

    #[test]
    fn parse_codex_meta_rejects_a_non_meta_first_line() {
        assert_eq!(parse_codex_meta(r#"{"type":"event_msg"}"#), None);
        assert_eq!(parse_codex_meta("garbage"), None);
        assert_eq!(
            parse_codex_meta(r#"{"type":"session_meta","payload":{"id":"a-1"}}"#),
            None,
            "no cwd means nothing to match against"
        );
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    fn claude_transcript(cwd: &str, entrypoint: &str) -> String {
        format!(
            "{}\n{}\n",
            r#"{"type":"mode","mode":"normal"}"#,
            serde_json::json!({
                "type": "user",
                "entrypoint": entrypoint,
                "cwd": cwd,
            })
        )
    }

    #[test]
    fn claude_scan_picks_the_newest_transcript_for_the_cwd() {
        let root = tempdir("claude-newest");
        let proj = root.join("D--Git-ymux");
        let old = proj.join("aaaaaaaa-0000-0000-0000-000000000001.jsonl");
        let new = proj.join("bbbbbbbb-0000-0000-0000-000000000002.jsonl");
        write(&old, &claude_transcript("D:\\Git\\ymux", "cli"));
        write(&new, &claude_transcript("D:\\Git\\ymux", "cli"));
        let now = SystemTime::now();
        filetime_set(&old, now - Duration::from_secs(3600));
        filetime_set(&new, now - Duration::from_secs(60));

        let found =
            newest_claude_session(&root, "D:\\Git\\ymux", now, &none()).expect("should find one");
        assert_eq!(found.session_id, "bbbbbbbb-0000-0000-0000-000000000002");
        assert_eq!(found.cwd, "D:\\Git\\ymux");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claude_scan_ignores_subagent_transcripts_and_the_desktop_app() {
        let root = tempdir("claude-filter");
        let proj = root.join("D--Git-ymux");
        let now = SystemTime::now();
        // A subagent transcript, in the nested directory the CLI really uses.
        let sub = proj
            .join("cccccccc-0000-0000-0000-000000000003")
            .join("subagents")
            .join("agent-a0d95a789b278dd14.jsonl");
        write(&sub, &claude_transcript("D:\\Git\\ymux", "cli"));
        filetime_set(&sub, now - Duration::from_secs(10));
        // A desktop-app session, at the top level but never in a terminal.
        let desk = proj.join("dddddddd-0000-0000-0000-000000000004.jsonl");
        write(&desk, &claude_transcript("D:\\Git\\ymux", "claude-desktop"));
        filetime_set(&desk, now - Duration::from_secs(20));

        assert_eq!(
            newest_claude_session(&root, "D:\\Git\\ymux", now, &none()),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claude_scan_rejects_a_directory_whose_in_file_cwd_disagrees() {
        // Mangling folds `_`, `.` and `-` together, so `Gadodaeng_3rd_2` and
        // `Gadodaeng-3rd-2` share a directory name. The in-file `cwd` is what
        // actually decides, via `ypath::same_path`.
        let root = tempdir("claude-collide");
        let proj = root.join("D--git-Project-Gadodaeng-3rd-2");
        let f = proj.join("eeeeeeee-0000-0000-0000-000000000005.jsonl");
        write(
            &f,
            &claude_transcript("D:\\git\\Project\\Gadodaeng_3rd_2", "cli"),
        );
        let now = SystemTime::now();
        filetime_set(&f, now - Duration::from_secs(10));

        assert_eq!(
            newest_claude_session(&root, "D:\\git\\Project\\Gadodaeng-3rd-2", now, &none()),
            None,
            "a different real directory must not match"
        );
        let found = newest_claude_session(&root, "d:/git/project/gadodaeng_3rd_2", now, &none())
            .expect("the real one matches, case and separators folded (rule 15)");
        assert_eq!(found.session_id, "eeeeeeee-0000-0000-0000-000000000005");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claude_scan_ignores_a_transcript_older_than_the_window() {
        let root = tempdir("claude-stale");
        let f = root
            .join("D--Git-ymux")
            .join("ffffffff-0000-0000-0000-000000000006.jsonl");
        write(&f, &claude_transcript("D:\\Git\\ymux", "cli"));
        let now = SystemTime::now();
        filetime_set(&f, now - FRESH_WINDOW - Duration::from_secs(60));
        assert_eq!(
            newest_claude_session(&root, "D:\\Git\\ymux", now, &none()),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_scan_walks_the_date_tree_and_picks_the_newest_cli_session() {
        let root = tempdir("codex-newest");
        let now = SystemTime::now();
        let meta = |id: &str, cwd: &str, source: serde_json::Value| {
            serde_json::json!({
                "type": "session_meta",
                "payload": { "id": id, "session_id": id, "cwd": cwd, "source": source }
            })
            .to_string()
        };
        // An older CLI session in one day directory...
        let a = root
            .join("2026/09/21")
            .join("rollout-2026-09-21T10-00-00-01a00000-0000-0000-0000-00000000000a.jsonl");
        write(
            &a,
            &meta(
                "01a00000-0000-0000-0000-00000000000a",
                "D:\\Git\\ymux",
                serde_json::json!("cli"),
            ),
        );
        filetime_set(&a, now - Duration::from_secs(7200));
        // ...and a newer one in another. A resumed session keeps appending to
        // its original file, so its mtime outruns its directory's date — the
        // reason the walk cannot narrow by directory date.
        let b = root
            .join("2026/09/20")
            .join("rollout-2026-09-20T10-00-00-01a00000-0000-0000-0000-00000000000b.jsonl");
        write(
            &b,
            &meta(
                "01a00000-0000-0000-0000-00000000000b",
                "D:\\Git\\ymux",
                serde_json::json!("cli"),
            ),
        );
        filetime_set(&b, now - Duration::from_secs(60));
        // A subagent rollout, newer than both, for the same cwd.
        let c = root
            .join("2026/09/23")
            .join("rollout-2026-09-23T10-00-00-01a00000-0000-0000-0000-00000000000c.jsonl");
        write(
            &c,
            &meta(
                "01a00000-0000-0000-0000-00000000000c",
                "D:\\Git\\ymux",
                serde_json::json!({ "subagent": { "depth": 1 } }),
            ),
        );
        filetime_set(&c, now - Duration::from_secs(5));
        // A CLI session for a different directory, newest of all.
        let d = root
            .join("2026/09/23")
            .join("rollout-2026-09-23T11-00-00-01a00000-0000-0000-0000-00000000000d.jsonl");
        write(
            &d,
            &meta(
                "01a00000-0000-0000-0000-00000000000d",
                "C:\\Git\\ScoreServer",
                serde_json::json!("cli"),
            ),
        );
        filetime_set(&d, now - Duration::from_secs(1));

        let found =
            newest_codex_session(&root, "D:\\Git\\ymux", now, &none()).expect("should find one");
        assert_eq!(found.session_id, "01a00000-0000-0000-0000-00000000000b");
        // Case and separators fold for a drive path (rule 15).
        let found2 =
            newest_codex_session(&root, "d:/git/YMUX", now, &none()).expect("same directory");
        assert_eq!(found2.session_id, found.session_id);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_scan_returns_nothing_when_no_cwd_matches() {
        let root = tempdir("codex-miss");
        let now = SystemTime::now();
        let f = root
            .join("2026/09/23")
            .join("rollout-2026-09-23T10-00-00-01a00000-0000-0000-0000-00000000000e.jsonl");
        write(
            &f,
            &serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "id": "01a00000-0000-0000-0000-00000000000e",
                    "cwd": "/srv/other",
                    "source": "cli"
                }
            })
            .to_string(),
        );
        filetime_set(&f, now - Duration::from_secs(5));
        assert_eq!(newest_codex_session(&root, "/srv/mine", now, &none()), None);
        // A POSIX path is case-sensitive: `/srv/Other` is a different place.
        assert_eq!(
            newest_codex_session(&root, "/srv/Other", now, &none()),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_claimed_transcript_is_skipped_so_two_panes_keep_their_own() {
        // Two panes in one directory. Without the exclusion the second pane's
        // rescan returns the same newest transcript the first already holds —
        // and worse, the *first* pane's next rescan picks up the second's
        // newer conversation.
        let root = tempdir("claude-claimed");
        let proj = root.join("D--Git-ymux");
        let mine = "aaaaaaaa-0000-0000-0000-000000000001";
        let theirs = "bbbbbbbb-0000-0000-0000-000000000002";
        let now = SystemTime::now();
        for (id, age) in [(mine, 600u64), (theirs, 60)] {
            let f = proj.join(format!("{id}.jsonl"));
            write(&f, &claude_transcript("D:\\Git\\ymux", "cli"));
            filetime_set(&f, now - Duration::from_secs(age));
        }
        // Nothing claimed: the newest wins.
        assert_eq!(
            newest_claude_session(&root, "D:\\Git\\ymux", now, &none()).map(|d| d.session_id),
            Some(theirs.to_string())
        );
        // With the newest claimed by another pane, this one keeps its own.
        let claimed: HashSet<String> = [theirs.to_string()].into_iter().collect();
        assert_eq!(
            newest_claude_session(&root, "D:\\Git\\ymux", now, &claimed).map(|d| d.session_id),
            Some(mine.to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_scan_skips_a_claimed_rollout() {
        let root = tempdir("codex-claimed");
        let now = SystemTime::now();
        let mine = "01a00000-0000-0000-0000-00000000001a";
        let theirs = "01a00000-0000-0000-0000-00000000001b";
        for (id, age) in [(mine, 600u64), (theirs, 60)] {
            let f = root
                .join("2026/09/23")
                .join(format!("rollout-2026-09-23T10-00-00-{id}.jsonl"));
            write(
                &f,
                &serde_json::json!({
                    "type": "session_meta",
                    "payload": { "id": id, "cwd": "D:\\Git\\ymux", "source": "cli" }
                })
                .to_string(),
            );
            filetime_set(&f, now - Duration::from_secs(age));
        }
        let claimed: HashSet<String> = [theirs.to_string()].into_iter().collect();
        assert_eq!(
            newest_codex_session(&root, "D:\\Git\\ymux", now, &claimed).map(|d| d.session_id),
            Some(mine.to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claude_transcript_is_found_by_id_whatever_the_cwd_was_spelled_like() {
        // A hook-borne record carries the shell's OSC 7 spelling, which from
        // Git Bash is `/d/Git/ymux` — mangling *that* gives `-d-Git-ymux`,
        // a directory that does not exist. Searching by id finds it anyway.
        let root = tempdir("claude-exists");
        let id = "aaaaaaaa-0000-0000-0000-00000000000f";
        write(
            &root.join("D--Git-ymux").join(format!("{id}.jsonl")),
            &claude_transcript("D:\\Git\\ymux", "cli"),
        );
        assert!(transcript_exists_under(AgentKind::Claude, &root, id, ""));
        assert!(!transcript_exists_under(
            AgentKind::Claude,
            &root,
            "cccccccc-0000-0000-0000-00000000000f",
            ""
        ));
        // And it refuses an id that is not safe to build a filename from.
        assert!(!transcript_exists_under(
            AgentKind::Claude,
            &root,
            "../evil",
            ""
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_transcript_is_found_by_id_even_when_a_newer_session_exists() {
        // The filename embeds a timestamp before the id, so there is no path
        // to rebuild; and "is this still the newest session here?" would
        // decline a good resume the moment any later Codex run touched the
        // same directory.
        let root = tempdir("codex-exists");
        let mine = "01a00000-0000-0000-0000-00000000002a";
        let newer = "01a00000-0000-0000-0000-00000000002b";
        for (id, day) in [(mine, "2026/09/20"), (newer, "2026/09/23")] {
            write(
                &root
                    .join(day)
                    .join(format!("rollout-2026-09-20T10-00-00-{id}.jsonl")),
                "{}",
            );
        }
        assert!(transcript_exists_under(AgentKind::Codex, &root, mine, ""));
        assert!(transcript_exists_under(AgentKind::Codex, &root, newer, ""));
        assert!(!transcript_exists_under(
            AgentKind::Codex,
            &root,
            "01a00000-0000-0000-0000-00000000002c",
            ""
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_existence_check_is_one_stat_when_the_cwd_says_where_to_look() {
        let root = tempdir("claude-exists-fast");
        let id = "aaaaaaaa-0000-0000-0000-00000000001f";
        for n in 0..20 {
            std::fs::create_dir_all(root.join(format!("C--other-{n}"))).expect("mkdir");
        }
        write(
            &root.join("D--Work-proj").join(format!("{id}.jsonl")),
            &claude_transcript("D:\\Work\\proj", "cli"),
        );
        // No directory walk allowed at all: only the derived path is tried.
        assert!(transcript_exists_within(
            AgentKind::Claude,
            &root,
            id,
            "D:\\Work\\proj",
            0
        ));
        // A hint spelled differently (Git Bash's OSC 7) misses the fast path
        // and falls back to the bounded walk.
        assert!(!transcript_exists_within(
            AgentKind::Claude,
            &root,
            id,
            "/d/Work/proj",
            0
        ));
        assert!(transcript_exists_within(
            AgentKind::Claude,
            &root,
            id,
            "/d/Work/proj",
            100
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_codex_existence_check_walks_newest_first_and_is_bounded() {
        let root = tempdir("codex-exists-bounded");
        let recent = "01a00000-0000-0000-0000-00000000003a";
        // Many old days, and the session in the newest one.
        for day in 1..=28 {
            std::fs::create_dir_all(root.join(format!("2025/01/{day:02}"))).expect("mkdir");
        }
        write(
            &root
                .join("2026/09/23")
                .join(format!("rollout-2026-09-23T10-00-00-{recent}.jsonl")),
            "{}",
        );
        // Found with a budget far below the size of the tree...
        assert!(transcript_exists_within(
            AgentKind::Codex,
            &root,
            recent,
            "",
            8
        ));
        // ...and a budget of nothing finds nothing, rather than walking on.
        assert!(!transcript_exists_within(
            AgentKind::Codex,
            &root,
            recent,
            "",
            1
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_head_is_capped() {
        let dir = tempdir("head-cap");
        let f = dir.join("big.jsonl");
        // No newline anywhere: a single line far larger than the cap.
        std::fs::write(&f, "x".repeat(MAX_HEAD_BYTES * 2)).expect("write");
        let head = read_head(&f, 1).expect("should read something");
        assert!(head.len() <= MAX_HEAD_BYTES, "read must stop at the cap");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Set a file's mtime. `std::fs` has no portable setter, so this goes
    /// through the platform APIs directly — the scan is mtime-driven, and a
    /// test that cannot control mtime cannot test it.
    fn filetime_set(path: &Path, when: SystemTime) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open for mtime");
        set_mtime(&file, when);
    }

    #[cfg(windows)]
    fn set_mtime(file: &std::fs::File, when: SystemTime) {
        use std::os::windows::io::AsRawHandle;
        let d = when
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("post-epoch");
        // FILETIME: 100 ns ticks since 1601-01-01, which is 11644473600 s
        // before the Unix epoch.
        let ticks = (d.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(d.subsec_nanos()) / 100;
        #[repr(C)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        extern "system" {
            fn SetFileTime(
                handle: *mut std::ffi::c_void,
                creation: *const FileTime,
                access: *const FileTime,
                write: *const FileTime,
            ) -> i32;
        }
        let ft = FileTime {
            low: (ticks & 0xFFFF_FFFF) as u32,
            high: (ticks >> 32) as u32,
        };
        let ok = unsafe {
            SetFileTime(
                file.as_raw_handle().cast(),
                std::ptr::null(),
                std::ptr::null(),
                &ft,
            )
        };
        assert_ne!(ok, 0, "SetFileTime failed");
    }

    #[cfg(unix)]
    fn set_mtime(file: &std::fs::File, when: SystemTime) {
        use std::os::fd::AsRawFd;
        let d = when
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("post-epoch");
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct TimeSpec {
            sec: i64,
            nsec: i64,
        }
        extern "C" {
            fn futimens(fd: i32, times: *const TimeSpec) -> i32;
        }
        let ts = TimeSpec {
            sec: d.as_secs() as i64,
            nsec: i64::from(d.subsec_nanos()),
        };
        let times = [ts, ts];
        let ok = unsafe { futimens(file.as_raw_fd(), times.as_ptr()) };
        assert_eq!(ok, 0, "futimens failed");
    }
}
