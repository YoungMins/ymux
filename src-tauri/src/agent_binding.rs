//! Which conversation is *this pane's* agent process running?
//!
//! The answer decides what ymux types into the pane on the next launch — and
//! a resumed Claude comes back with `--dangerously-skip-permissions` — so a
//! wrong answer is worse than none. Everything here is pure (no IO, no Tauri)
//! and tested on Linux CI (rule 1); the scan thread gathers the inputs.
//!
//! ## Sources, most exact first
//!
//! 1. **A Claude Code hook** (`y agent-hook`): the agent naming its own id.
//! 2. **`~/.claude/sessions/<pid>.json`**: Claude Code keeps one per running
//!    interactive process, `{"pid", "sessionId", "startedAt", "entrypoint",
//!    …}`, and rewrites it when the process switches conversation (`/resume`,
//!    `/clear`). Undocumented — so it is only believed when its pid is the
//!    pane's agent process and its `startedAt` agrees with that process's
//!    start time ([`registry_session_id`]); anything else falls through.
//! 3. **The process's own argv**: `claude --resume <id>`, `claude
//!    --session-id <id>`, `codex resume <id>` ([`argv_session_id`]). This is
//!    what binds a pane ymux itself resumed.
//! 4. **A transcript created after the process started** ([`guess_transcript`]),
//!    the only source for an older Claude without the pid file, or Codex
//!    started without an id. Guarded so it abstains whenever another agent
//!    process could have written the transcript.
//!
//! Sources 1–3 are exact and may move a pane to a new id. Source 4 is a guess:
//! once made it is never changed while the same process lives
//! (`SessionTracker::observe`), because the only thing a later rescan can add
//! is someone else's conversation.

use std::collections::{BTreeSet, HashSet};

use crate::agent_scan_disk::DiskSession;
use crate::agent_sessions::{is_valid_session_id, AgentKind};

/// The agent process the scan found in a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneProcess {
    pub pid: u32,
    /// Process start, whole seconds since the Unix epoch (sysinfo's
    /// `start_time`, which rounds down).
    pub start_secs: u64,
    pub argv: Vec<String>,
    /// The agent's own process subtree, itself included. A node-hosted Codex
    /// spawns its native binary as a child; that child is the same agent, not
    /// a second one competing for the same transcript.
    pub subtree: BTreeSet<u32>,
}

impl PaneProcess {
    /// What identifies *this* process across scans: a pid alone is reused.
    pub fn key(&self) -> ProcessKey {
        ProcessKey {
            pid: self.pid,
            start_secs: self.start_secs,
        }
    }
}

/// A process's identity: pid plus start time, so a reused pid is a new key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcessKey {
    pub pid: u32,
    pub start_secs: u64,
}

/// Any other agent process on the machine — in another pane, or in a
/// terminal ymux knows nothing about. Only the outermost process of each
/// agent is listed (a wrapper's native child is not a second agent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherAgent {
    pub kind: AgentKind,
    pub pid: u32,
    pub start_secs: u64,
    /// Its working directory, when the OS let us read it. `None` counts as
    /// "could be anywhere", i.e. possibly the pane's directory.
    pub cwd: Option<String>,
    /// The conversation it is known to hold (its own pid file, argv, or the
    /// pane binding ymux already made for it). A process with a known id is
    /// not a candidate author of some *other* transcript.
    pub known_id: Option<String>,
}

/// One `~/.claude/sessions/<pid>.json`, reduced to what is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePidFile {
    pub pid: u32,
    pub session_id: String,
    /// Milliseconds since the Unix epoch.
    pub started_at_ms: u64,
    pub entrypoint: Option<String>,
}

/// Parse a `~/.claude/sessions/<pid>.json`. `None` for anything that is not
/// the shape observed on Claude Code 2.1.281 — the file is undocumented, so
/// a different shape means "don't know", never an error.
pub fn parse_claude_pid_file(json: &str) -> Option<ClaudePidFile> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(ClaudePidFile {
        pid: u32::try_from(v.get("pid")?.as_u64()?).ok()?,
        session_id: v.get("sessionId")?.as_str()?.to_string(),
        started_at_ms: v.get("startedAt")?.as_u64()?,
        entrypoint: v
            .get("entrypoint")
            .and_then(|e| e.as_str())
            .map(str::to_string),
    })
}

/// How long after the OS says a process started its pid file may say it
/// started. Claude writes `startedAt` from JS once the runtime is up, which is
/// after the process exists; two minutes absorbs a slow start while still
/// rejecting a stale file whose pid the OS has since reused.
pub const PID_FILE_START_SLACK_SECS: u64 = 120;

/// The session id `file` names for `proc`, if the file is believable: it is
/// about a pid inside the pane agent's own subtree, it describes a process
/// that started when this one did (so not a dead one whose pid was reused),
/// it comes from the terminal CLI, and the id is safe to type into a shell.
pub fn registry_session_id(proc: &PaneProcess, file: &ClaudePidFile) -> Option<String> {
    if !proc.subtree.contains(&file.pid) {
        return None;
    }
    let started = file.started_at_ms / 1000;
    if started < proc.start_secs || started > proc.start_secs + PID_FILE_START_SLACK_SECS {
        return None;
    }
    if file.entrypoint.as_deref() != Some("cli") {
        return None;
    }
    is_valid_session_id(&file.session_id).then(|| file.session_id.clone())
}

/// The session id a process was *started* on, read off its argv.
///
/// * Claude: `--session-id <id>` names the conversation outright (with
///   `--fork-session` it is the new fork's id — still this process's). Else
///   `--resume <id>` / `-r <id>` names it, unless `--fork-session` is present,
///   in which case the process is running a fresh copy under an id argv does
///   not show. A `--resume` value may be a transcript path; its file stem is
///   the id.
/// * Codex: `codex [global flags] resume <id>`. `fork`, `--last`, a session
///   *name*, or anything else is `None`.
///
/// The CLI may be node-hosted (`node …/cli.js --resume <id>`), so the host
/// and its script are skipped first.
pub fn argv_session_id(agent: AgentKind, argv: &[String]) -> Option<String> {
    let args = cli_args(argv);
    match agent {
        AgentKind::Claude => claude_argv_id(args),
        AgentKind::Codex => codex_argv_id(args),
    }
}

/// The CLI's own arguments: everything after the program, or after the
/// script when the program is a script host.
fn cli_args(argv: &[String]) -> &[String] {
    let Some(first) = argv.first() else {
        return argv;
    };
    let stem = first
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(first)
        .to_ascii_lowercase();
    let stem = stem.strip_suffix(".exe").unwrap_or(&stem);
    if matches!(stem, "node" | "bun") {
        // Host flags, then the script, then the CLI's arguments.
        match argv.iter().skip(1).position(|a| !a.starts_with('-')) {
            Some(i) => &argv[i + 2..],
            None => &[],
        }
    } else {
        &argv[1..]
    }
}

fn id_from_value(v: &str) -> Option<String> {
    if is_valid_session_id(v) {
        return Some(v.to_string());
    }
    // `--resume C:\Users\…\projects\D--x\<id>.jsonl` — seen in the wild on a
    // Claude background session.
    let name = v.rsplit(['/', '\\']).next()?;
    let stem = name.strip_suffix(".jsonl")?;
    is_valid_session_id(stem).then(|| stem.to_string())
}

fn claude_argv_id(args: &[String]) -> Option<String> {
    let value_of = |names: &[&str]| -> Option<String> {
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            for n in names {
                if a == *n {
                    return args.get(i + 1).cloned();
                }
                if let Some(v) = a.strip_prefix(n).and_then(|r| r.strip_prefix('=')) {
                    return Some(v.to_string());
                }
            }
            i += 1;
        }
        None
    };
    if let Some(v) = value_of(&["--session-id"]) {
        return id_from_value(&v);
    }
    if args.iter().any(|a| a == "--fork-session") {
        return None;
    }
    value_of(&["--resume", "-r"]).and_then(|v| id_from_value(&v))
}

/// Codex options (global, and `resume`'s own) that take exactly one value,
/// read off `codex --help` / `codex resume --help` (codex-cli 0.155).
const CODEX_VALUE_FLAGS: &[&str] = &[
    "-c",
    "--config",
    "--enable",
    "--disable",
    "--remote",
    "--remote-auth-token-env",
    "-m",
    "--model",
    "--local-provider",
    "-p",
    "--profile",
    "-s",
    "--sandbox",
    "-C",
    "--cd",
    "--add-dir",
    "-a",
    "--ask-for-approval",
];

fn codex_argv_id(args: &[String]) -> Option<String> {
    let mut i = 0;
    let mut in_resume = false;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "-i" || a == "--image" {
            // Variadic: where its values end cannot be told from argv.
            return None;
        }
        if CODEX_VALUE_FLAGS.contains(&a) {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            if in_resume && a == "--last" {
                return None;
            }
            i += 1;
            continue;
        }
        if !in_resume {
            if a != "resume" {
                // `fork`, `exec`, a prompt, …: not a resume of a known id.
                return None;
            }
            in_resume = true;
            i += 1;
            continue;
        }
        return is_valid_session_id(a).then(|| a.to_string());
    }
    None
}

/// The transcript `proc` most plausibly started, or `None` when that cannot
/// be said with confidence.
///
/// `candidates` are the transcripts recorded for the pane's cwd. The rule:
///
/// 1. Only a transcript whose conversation began **at or after** the
///    process started can be one it started. Yesterday's session in the same
///    folder is older than the process and is out.
/// 2. Not one another pane has claimed, nor one another process is known to
///    be running.
/// 3. Of what is left, the **earliest** — the process's first conversation,
///    not whichever one somebody touched last.
/// 4. And only if **no other agent process could have written it**: one of
///    the same kind, outside this process's own tree, not already known to
///    hold a different conversation, started no later than the transcript,
///    and in the same directory (or one we cannot read). Two unbound Claudes
///    in one folder, or one in a terminal ymux cannot see, make the answer
///    ambiguous — and no resume beats a wrong one.
pub fn guess_transcript(
    agent: AgentKind,
    pane_cwd: &str,
    proc: &PaneProcess,
    candidates: &[DiskSession],
    claimed: &HashSet<String>,
    others: &[OtherAgent],
) -> Option<DiskSession> {
    let held_elsewhere = |id: &str| {
        claimed.contains(id)
            || others
                .iter()
                .any(|o| !proc.subtree.contains(&o.pid) && o.known_id.as_deref() == Some(id))
    };
    let first = candidates
        .iter()
        .filter(|d| d.created.is_some_and(|c| c >= proc.start_secs))
        .filter(|d| !held_elsewhere(&d.session_id))
        .min_by(|a, b| {
            a.created
                .cmp(&b.created)
                .then_with(|| a.session_id.cmp(&b.session_id))
        })?;
    let created = first.created?;
    let contested = others.iter().any(|o| {
        o.kind == agent
            && !proc.subtree.contains(&o.pid)
            && o.known_id.is_none()
            && o.start_secs <= created
            && o.cwd
                .as_deref()
                .map_or(true, |c| ypath::same_path(c, pane_cwd))
    });
    (!contested).then(|| first.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    const CWD: &str = "D:\\Work\\proj";
    const ID_A: &str = "aaaaaaaa-0000-0000-0000-00000000000a";
    const ID_B: &str = "bbbbbbbb-0000-0000-0000-00000000000b";
    const ID_Y: &str = "99999999-0000-0000-0000-000000000099";

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn proc(pid: u32, start: u64) -> PaneProcess {
        PaneProcess {
            pid,
            start_secs: start,
            argv: argv(&["claude"]),
            subtree: [pid].into_iter().collect(),
        }
    }

    fn other(pid: u32, start: u64) -> OtherAgent {
        OtherAgent {
            kind: AgentKind::Claude,
            pid,
            start_secs: start,
            cwd: Some(CWD.to_string()),
            known_id: None,
        }
    }

    fn transcript(id: &str, created: u64) -> DiskSession {
        DiskSession {
            session_id: id.to_string(),
            cwd: CWD.to_string(),
            modified: SystemTime::UNIX_EPOCH,
            created: Some(created),
        }
    }

    fn guess(p: &PaneProcess, c: &[DiskSession], others: &[OtherAgent]) -> Option<String> {
        guess_transcript(AgentKind::Claude, CWD, p, c, &HashSet::new(), others)
            .map(|d| d.session_id)
    }

    #[test]
    fn yesterdays_session_in_the_same_folder_is_not_this_process() {
        let p = proc(10, 100_000);
        assert_eq!(guess(&p, &[transcript(ID_Y, 100_000 - 86_000)], &[]), None);
        // Its own conversation, once written, is.
        assert_eq!(
            guess(
                &p,
                &[
                    transcript(ID_Y, 100_000 - 86_000),
                    transcript(ID_A, 100_030)
                ],
                &[]
            ),
            Some(ID_A.to_string())
        );
    }

    #[test]
    fn a_transcript_with_no_recorded_start_is_never_guessed() {
        let mut t = transcript(ID_A, 1_030);
        t.created = None;
        assert_eq!(guess(&proc(10, 1_000), &[t], &[]), None);
    }

    #[test]
    fn the_earliest_eligible_transcript_wins_not_the_newest() {
        let p = proc(10, 1_000);
        assert_eq!(
            guess(&p, &[transcript(ID_B, 1_500), transcript(ID_A, 1_030)], &[]),
            Some(ID_A.to_string())
        );
    }

    #[test]
    fn two_panes_in_one_folder_never_swap() {
        // A starts at 1000, B at 1100. The user prompts in B first (1200),
        // then in A (1300). Both transcripts are after both starts, so from
        // timestamps alone either could be either: neither pane may guess.
        let a = proc(10, 1_000);
        let b = proc(20, 1_100);
        let disk = [transcript(ID_B, 1_200), transcript(ID_A, 1_300)];
        assert_eq!(guess(&a, &disk, &[other(20, 1_100)]), None);
        assert_eq!(guess(&b, &disk, &[other(10, 1_000)]), None);
    }

    #[test]
    fn two_panes_in_one_folder_each_bind_their_own_when_it_is_decidable() {
        // A prompts (1050) before B even starts (1100). A's transcript can
        // only be A's; once A is bound, B's is the only one left for B.
        let a = proc(10, 1_000);
        let b = proc(20, 1_100);
        let disk = [transcript(ID_A, 1_050), transcript(ID_B, 1_200)];
        assert_eq!(
            guess(&a, &disk, &[other(20, 1_100)]),
            Some(ID_A.to_string())
        );
        // B scanned before A is bound: A might have written ID_B.
        assert_eq!(guess(&b, &disk, &[other(10, 1_000)]), None);
        // After A is bound to ID_A it is no longer a candidate author.
        let mut a_bound = other(10, 1_000);
        a_bound.known_id = Some(ID_A.to_string());
        assert_eq!(guess(&b, &disk, &[a_bound]), Some(ID_B.to_string()));
    }

    #[test]
    fn a_claude_in_an_outside_terminal_is_never_adopted() {
        // The pane's Claude has not written anything yet; one started in
        // another terminal in the same folder has — before or after ours.
        let p = proc(10, 1_000);
        for outside_start in [900, 1_100] {
            let o = other(77, outside_start);
            assert_eq!(
                guess(&p, &[transcript(ID_B, 1_200)], std::slice::from_ref(&o)),
                None,
                "outside process started at {outside_start}"
            );
            // Unreadable cwd counts as "could be here".
            let mut blind = o.clone();
            blind.cwd = None;
            assert_eq!(guess(&p, &[transcript(ID_B, 1_200)], &[blind]), None);
            // One in a different folder is not a competitor.
            let mut elsewhere = o;
            elsewhere.cwd = Some("D:\\Work\\other".into());
            assert_eq!(
                guess(&p, &[transcript(ID_B, 1_200)], &[elsewhere]),
                Some(ID_B.to_string())
            );
        }
    }

    #[test]
    fn a_process_known_to_hold_a_transcript_owns_it() {
        let p = proc(10, 1_000);
        let mut o = other(77, 900);
        o.known_id = Some(ID_B.to_string());
        assert_eq!(guess(&p, &[transcript(ID_B, 1_200)], &[o]), None);
        let claimed: HashSet<String> = [ID_A.to_string()].into_iter().collect();
        assert_eq!(
            guess_transcript(
                AgentKind::Claude,
                CWD,
                &p,
                &[transcript(ID_A, 1_100)],
                &claimed,
                &[]
            ),
            None,
            "another pane's claim"
        );
    }

    #[test]
    fn the_agents_own_child_is_not_a_competitor() {
        // node codex.js (pid 10) runs codex.exe (pid 11): one agent.
        let mut p = proc(10, 1_000);
        p.subtree.insert(11);
        let mut child = other(11, 1_001);
        child.kind = AgentKind::Codex;
        let t = [transcript(ID_A, 1_030)];
        assert_eq!(
            guess_transcript(AgentKind::Codex, CWD, &p, &t, &HashSet::new(), &[child])
                .map(|d| d.session_id),
            Some(ID_A.to_string())
        );
    }

    #[test]
    fn a_different_agent_kind_is_not_a_competitor() {
        let mut codex = other(77, 900);
        codex.kind = AgentKind::Codex;
        assert_eq!(
            guess(&proc(10, 1_000), &[transcript(ID_A, 1_030)], &[codex]),
            Some(ID_A.to_string())
        );
    }

    #[test]
    fn claude_argv_names_the_session() {
        let c = AgentKind::Claude;
        assert_eq!(
            argv_session_id(
                c,
                &argv(&["claude", "--resume", ID_A, "--dangerously-skip-permissions"])
            ),
            Some(ID_A.to_string())
        );
        assert_eq!(
            argv_session_id(c, &argv(&["claude", "-r", ID_A])),
            Some(ID_A.to_string())
        );
        assert_eq!(
            argv_session_id(c, &argv(&["claude", &format!("--resume={ID_A}")])),
            Some(ID_A.to_string())
        );
        // A fork runs under a new id argv does not show...
        assert_eq!(
            argv_session_id(c, &argv(&["claude", "--resume", ID_A, "--fork-session"])),
            None
        );
        // ...unless `--session-id` names it — the exact shape a Claude
        // background session was seen running with.
        let bg = argv(&[
            "C:\\Users\\u\\.local\\bin\\claude.exe",
            "--session-id",
            ID_B,
            "--fork-session",
            "--resume",
            &format!("C:\\Users\\u\\.claude\\projects\\D--x\\{ID_A}.jsonl"),
            "--model",
            "opus",
        ]);
        assert_eq!(argv_session_id(c, &bg), Some(ID_B.to_string()));
        // A transcript path as the resume value.
        assert_eq!(
            argv_session_id(
                c,
                &argv(&[
                    "claude",
                    "--resume",
                    &format!("/h/.claude/projects/-w/{ID_A}.jsonl")
                ])
            ),
            Some(ID_A.to_string())
        );
        // Node-hosted.
        assert_eq!(
            argv_session_id(
                c,
                &argv(&[
                    "node",
                    "--max-old-space-size=4096",
                    "/n/@anthropic-ai/claude-code/cli.js",
                    "--resume",
                    ID_A
                ])
            ),
            Some(ID_A.to_string())
        );
        // Nothing to go on.
        for bare in [
            argv(&["claude"]),
            argv(&["claude", "-c"]),
            argv(&["claude", "--resume"]),
            argv(&["claude", "--resume", "my search"]),
        ] {
            assert_eq!(argv_session_id(c, &bare), None, "{bare:?}");
        }
    }

    #[test]
    fn codex_argv_names_the_session() {
        let k = AgentKind::Codex;
        assert_eq!(
            argv_session_id(k, &argv(&["codex", "resume", ID_A])),
            Some(ID_A.to_string())
        );
        assert_eq!(
            argv_session_id(k, &argv(&["codex", "-m", "o3", "resume", "--search", ID_A])),
            Some(ID_A.to_string())
        );
        assert_eq!(
            argv_session_id(
                k,
                &argv(&["node", "/n/@openai/codex/bin/codex.js", "resume", ID_A])
            ),
            Some(ID_A.to_string())
        );
        for none in [
            argv(&["codex"]),
            argv(&["codex", "resume", "--last"]),
            argv(&["codex", "resume", "my-name"]),
            argv(&["codex", "fork", ID_A]),
            argv(&["codex", "review this"]),
            argv(&["codex", "-i", "a.png", "resume", ID_A]),
        ] {
            assert_eq!(argv_session_id(k, &none), None, "{none:?}");
        }
    }

    #[test]
    fn the_claude_pid_file_parses_the_shape_observed_in_the_wild() {
        // Trimmed from a real `~/.claude/sessions/53676.json`.
        let json = r#"{"pid":53676,"sessionId":"20aebce7-f8e2-4582-b480-bf1ae90d7a0b","cwd":"D:\\Work\\proj","startedAt":1790212419413,"procStart":"134346860185931977","version":"2.1.281","kind":"interactive","entrypoint":"cli","status":"idle"}"#;
        let f = parse_claude_pid_file(json).expect("parses");
        assert_eq!(f.pid, 53676);
        assert_eq!(f.session_id, "20aebce7-f8e2-4582-b480-bf1ae90d7a0b");
        assert_eq!(f.started_at_ms, 1_790_212_419_413);
        assert_eq!(f.entrypoint.as_deref(), Some("cli"));
        assert_eq!(parse_claude_pid_file("{}"), None);
        assert_eq!(parse_claude_pid_file("not json"), None);
    }

    #[test]
    fn the_pid_file_is_believed_only_for_this_very_process() {
        let p = proc(53676, 1_790_212_418);
        let file = ClaudePidFile {
            pid: 53676,
            session_id: ID_A.to_string(),
            started_at_ms: 1_790_212_419_413,
            entrypoint: Some("cli".into()),
        };
        assert_eq!(registry_session_id(&p, &file), Some(ID_A.to_string()));
        // A stale file whose pid the OS has since reused.
        let reused = proc(53676, 1_790_300_000);
        assert_eq!(registry_session_id(&reused, &file), None);
        // Some other process's file.
        assert_eq!(registry_session_id(&proc(1, 1_790_212_418), &file), None);
        // The desktop app, or an unsafe id.
        let mut desk = file.clone();
        desk.entrypoint = Some("claude-desktop".into());
        assert_eq!(registry_session_id(&p, &desk), None);
        let mut evil = file;
        evil.session_id = "$(rm -rf /)".into();
        assert_eq!(registry_session_id(&p, &evil), None);
    }
}
