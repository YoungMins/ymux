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
}

/// Parse the head of a Claude transcript (JSONL). Stops as soon as both
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
        if meta.cwd.is_some() && meta.entrypoint.is_some() {
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
    Some(CodexMeta {
        id,
        cwd,
        source: p.get("source").and_then(|x| x.as_str()).map(str::to_string),
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

/// Newest resumable Claude session for `cwd` under a `.claude/projects` root.
///
/// The project directory name is derived from `cwd`, so this is one
/// `read_dir` of the projects root (22 entries here) plus one of the matching
/// project directory. Every candidate is confirmed against the `cwd` recorded
/// *inside* the transcript with `ypath::same_path` (rule 15) — mangling folds
/// `_`, `.` and `-` together, so the directory name alone cannot prove a
/// match.
pub fn newest_claude_session(
    projects_root: &Path,
    cwd: &str,
    now: SystemTime,
) -> Option<DiskSession> {
    let mut budget = MAX_DIR_ENTRIES;
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(projects_root).ok()?.flatten() {
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
    let mut best: Option<DiskSession> = None;
    for dir in dirs {
        for c in candidates_in(&dir, now, FRESH_WINDOW, &mut budget) {
            if best.as_ref().is_some_and(|b| b.modified >= c.modified) {
                continue;
            }
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
            best = Some(DiskSession {
                session_id: stem.to_string(),
                cwd: found_cwd,
                modified: c.modified,
            });
        }
    }
    best
}

/// Newest resumable Codex session for `cwd` under a `.codex/sessions` root.
///
/// The tree is `YYYY/MM/DD/`, but the date directory is *not* a usable filter:
/// 44 of the 847 rollouts here have an mtime on a later day than their
/// directory, because a resumed session keeps appending to its original file.
/// Narrowing by directory date would therefore skip exactly the sessions most
/// worth resuming. Instead every day directory is listed — `read_dir` only,
/// no file is opened — and the mtime window does the narrowing: 3 of 847 here.
pub fn newest_codex_session(
    sessions_root: &Path,
    cwd: &str,
    now: SystemTime,
) -> Option<DiskSession> {
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
        return Some(DiskSession {
            session_id: meta.id,
            cwd: meta.cwd,
            modified: c.modified,
        });
    }
    None
}

/// `~/.claude/projects`, or `None` if the OS has no home directory.
pub fn claude_projects_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// `~/.codex/sessions`.
pub fn codex_sessions_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
}

/// Newest resumable session for `agent` in `cwd`, against the real transcript
/// roots in the user's home directory.
pub fn newest_session(agent: AgentKind, cwd: &str) -> Option<DiskSession> {
    let now = SystemTime::now();
    match agent {
        AgentKind::Claude => newest_claude_session(&claude_projects_root()?, cwd, now),
        AgentKind::Codex => newest_codex_session(&codex_sessions_root()?, cwd, now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let found = newest_claude_session(&root, "D:\\Git\\ymux", now).expect("should find one");
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

        assert_eq!(newest_claude_session(&root, "D:\\Git\\ymux", now), None);
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
            newest_claude_session(&root, "D:\\git\\Project\\Gadodaeng-3rd-2", now),
            None,
            "a different real directory must not match"
        );
        let found = newest_claude_session(&root, "d:/git/project/gadodaeng_3rd_2", now)
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
        assert_eq!(newest_claude_session(&root, "D:\\Git\\ymux", now), None);
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

        let found = newest_codex_session(&root, "D:\\Git\\ymux", now).expect("should find one");
        assert_eq!(found.session_id, "01a00000-0000-0000-0000-00000000000b");
        // Case and separators fold for a drive path (rule 15).
        let found2 = newest_codex_session(&root, "d:/git/YMUX", now).expect("same directory");
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
        assert_eq!(newest_codex_session(&root, "/srv/mine", now), None);
        // A POSIX path is case-sensitive: `/srv/Other` is a different place.
        assert_eq!(newest_codex_session(&root, "/srv/Other", now), None);
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
