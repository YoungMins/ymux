//! Resumable agent sessions: which coding agent was mid-conversation in which
//! pane, and the agent's own session id, so ymux can *resume* the agent on the
//! next launch instead of replaying a screenshot of it.
//!
//! Deliberately Tauri-free — every function here is covered by
//! `cargo test --no-default-features --lib -p ymux` on Linux CI (rule 1).
//!
//! **Why this is not a `PaneSpec` field.** Three of the project's rules point
//! the same way. Rule 2: a `PaneSpec` field has to be mirrored in four places
//! or it silently vanishes. Rule 3: an `Option<T>` inside the `#[serde(tag =
//! "kind")]` layout enum does not round-trip through TOML at all. Rule 11: the
//! frontend rewrites the whole config on every layout save, so a
//! backend-owned value living there gets clobbered by a stale snapshot — the
//! same reason `agent_tracking` is excluded from `merge_layouts_from`. A
//! session id is written by the hook listener and the process scan, never by
//! the frontend, so it belongs in a backend-owned store beside
//! [`crate::scrollback`].

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent_binding::{
    argv_session_id, guess_transcript, OtherAgent, PaneProcess, ProcessKey,
};
use crate::agents::AgentStatus;

/// A coding-agent CLI ymux knows how to resume.
///
/// Serialized lowercase so it matches the `kind` strings the agent registry
/// and the process scan already use (`"claude"`, `"codex"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    /// Parse the `kind` string the registry/scan use. `None` for an agent
    /// ymux has no resume story for yet (Gemini, Aider, …) — spec §8.
    pub fn from_kind(kind: &str) -> Option<Self> {
        match kind.to_ascii_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    /// The `kind` string, as the registry spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// Where the session id came from. A hook-borne id is the agent telling us its
/// own id, so it outranks anything inferred from a transcript on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdSource {
    Hook,
    /// Named by the agent process itself: its argv (`--resume <id>`) or
    /// Claude's `~/.claude/sessions/<pid>.json`. Exact, like a hook.
    Process,
    /// Inferred from a transcript on disk (`agent_binding::guess_transcript`).
    Disk,
}

/// How long a record stays eligible for an automatic resume (spec §4).
pub const FRESH_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// One pane's resumable session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    pub pane_id: Uuid,
    pub agent: AgentKind,
    /// The agent's own session id, exactly as it must be typed back to the
    /// CLI. Always passes [`is_valid_session_id`] before it is stored.
    pub session_id: String,
    /// The cwd the session belongs to, in the raw spelling its producer used.
    /// Compared with `ypath` (rule 15), never `==`; kept raw because it is
    /// also the directory the resumed pane must be spawned in.
    pub cwd: String,
    /// Last known agent status.
    pub state: AgentStatus,
    /// The last known state was not `done` — the conversation was cut off
    /// mid-turn rather than finished.
    pub interrupted: bool,
    /// The agent is still (as far as ymux knows) the thing running in that
    /// pane. Cleared when the process scan sees the agent exit while the pane
    /// lives on, so quitting Claude and then closing ymux an hour later does
    /// not silently resurrect the conversation.
    pub active: bool,
    pub source: IdSource,
    /// Seconds since the Unix epoch. Stored as a plain integer rather than a
    /// `SystemTime` so the JSON stays readable and version-stable.
    pub updated_at: u64,
}

impl AgentSession {
    /// Whether this record is still eligible for an automatic resume, ignoring
    /// whether the transcript file still exists (the caller checks that).
    ///
    /// Requires `active`, and an `updated_at` inside [`FRESH_WINDOW`] of
    /// `now`. A record stamped in the future (clock skew, a restored backup)
    /// counts as fresh rather than being thrown away.
    pub fn is_fresh_at(&self, now: u64) -> bool {
        self.active && now.saturating_sub(self.updated_at) <= FRESH_WINDOW.as_secs()
    }
}

/// Seconds since the Unix epoch, saturating at 0 for a pre-epoch clock.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether `id` is safe to splice into a shell command line.
///
/// The id reaches us from a filename and from JSON written by another program,
/// and it is *typed into the user's shell* (spec §3), so it gets the same
/// paranoia `scrollback::scrollback_file_under` applies to a pane id. Both
/// CLIs use UUID-shaped ids, and `codex resume` additionally accepts free-form
/// session *names* — which is exactly why this is an allowlist: hex digits and
/// `-` only, bounded length, no empty string.
pub fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && id.chars().any(|c| c.is_ascii_hexdigit())
}

/// The argv that resumes `agent` at `session_id`, as a user would type it.
///
/// Verified against the CLIs installed on the development machine:
/// `claude --help` lists `-r, --resume [value]  Resume a conversation by
/// session ID`, and `codex resume --help` lists
/// `Usage: codex resume [OPTIONS] [SESSION_ID] [PROMPT]`.
///
/// Never the "most recent" forms (`claude -c`, `codex resume --last`): an
/// explicit id is the only selector that cannot resume the wrong conversation
/// (spec §3).
///
/// A resumed Claude session also comes back in permission-bypass mode, so the
/// user picks the conversation up where they left it instead of re-approving
/// everything they had already approved. `claude --help` on this machine
/// documents the flag as `--dangerously-skip-permissions  Bypass all
/// permission checks.`, which is the mode those transcripts record as
/// `"permissionMode":"bypassPermissions"`.
///
/// Claude only. `codex --help` does document an equivalent
/// (`--dangerously-bypass-approvals-and-sandbox`), but it *also* turns off the
/// sandbox, which is a materially different promise from skipping approval
/// prompts — so it stays out until someone asks for it.
pub fn resume_argv(agent: AgentKind, session_id: &str) -> Option<Vec<String>> {
    if !is_valid_session_id(session_id) {
        return None;
    }
    Some(match agent {
        AgentKind::Claude => vec![
            "claude".into(),
            "--resume".into(),
            session_id.to_string(),
            CLAUDE_SKIP_PERMISSIONS.into(),
        ],
        AgentKind::Codex => {
            vec!["codex".into(), "resume".into(), session_id.to_string()]
        }
    })
}

/// The flag that brings a resumed Claude session back without permission
/// prompts. Verbatim from `claude --help` on this machine.
pub const CLAUDE_SKIP_PERMISSIONS: &str = "--dangerously-skip-permissions";

/// The quoting rules of the shell a resume command is typed into.
///
/// The command is typed into the pane's shell (spec §3), so each argument the
/// user's own startup command carried has to come back out quoted for *that*
/// shell. Anything this cannot quote with certainty makes the caller fall
/// back to the bare resume command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFamily {
    /// bash / zsh / sh (Git Bash included): `'…'`, `'\''` for a quote.
    Posix,
    /// Windows PowerShell and pwsh: `'…'`, `''` for a quote.
    PowerShell,
    /// cmd.exe: `"…"`, with no way to escape `"`, `%` or `!` inside.
    Cmd,
    /// Anything else — fish, nu, `wsl.exe` (whose inner shell is unknown), an
    /// unresolved profile: only arguments that need no quoting at all.
    Unknown,
}

impl ShellFamily {
    /// Classify a shell profile by its executable's file stem.
    pub fn from_executable(executable: &str) -> Self {
        let stem = executable
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(executable)
            .to_ascii_lowercase();
        let stem = stem.strip_suffix(".exe").unwrap_or(&stem);
        match stem {
            "cmd" => Self::Cmd,
            "powershell" | "pwsh" => Self::PowerShell,
            "bash" | "zsh" | "sh" | "dash" | "ksh" => Self::Posix,
            _ => Self::Unknown,
        }
    }

    fn bare_ok(self, c: char) -> bool {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '.' | '/' | ':' | '=' | '+' | '-')
            || (c == '\\' && matches!(self, Self::Cmd | Self::PowerShell))
    }

    /// `token` as it must be typed into this shell to arrive as exactly
    /// `token`, or `None` when that cannot be done with certainty.
    pub fn quote(self, token: &str) -> Option<String> {
        if token.is_empty() || token.chars().any(char::is_control) {
            return None;
        }
        if token.chars().all(|c| self.bare_ok(c)) {
            return Some(token.to_string());
        }
        match self {
            Self::Posix => Some(format!("'{}'", token.replace('\'', r"'\''"))),
            Self::PowerShell => {
                // PowerShell also reads U+2018–U+201B as single quotes; and a
                // `"` or a trailing `\` is mangled when Windows PowerShell
                // re-quotes the argument for a native program.
                if token.contains('"')
                    || token.ends_with('\\')
                    || token
                        .chars()
                        .any(|c| ('\u{2018}'..='\u{201B}').contains(&c))
                {
                    return None;
                }
                Some(format!("'{}'", token.replace('\'', "''")))
            }
            Self::Cmd => {
                // `%VAR%` and `!VAR!` expand even inside quotes, `"` cannot be
                // escaped, and a trailing `\` escapes the closing quote for
                // the program's own argv parser.
                if token.contains(['"', '%', '!', '^']) || token.ends_with('\\') {
                    return None;
                }
                Some(format!("\"{token}\""))
            }
            Self::Unknown => None,
        }
    }
}

/// Split a saved `startup_cmd` into the arguments `shell` would pass, or
/// `None` when that cannot be said with certainty.
///
/// Deliberately a strict *subset* of each shell's grammar: plain words, and
/// quoted runs with nothing inside them the shell would still interpret.
/// Operators (`;`, `&`, `|`, `<`, `>`, parentheses), expansions (`$`, `` ` ``,
/// `%`, `!`), globs, escapes and anything unbalanced all return `None` — the
/// caller then types the bare resume command, which is always correct.
fn split_for(shell: ShellFamily, s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_control() && !c.is_whitespace() {
            return None;
        }
        if c.is_whitespace() {
            if c != ' ' && c != '\t' {
                return None;
            }
            if in_word {
                out.push(std::mem::take(&mut cur));
                in_word = false;
            }
            continue;
        }
        let quote = match c {
            '"' if shell != ShellFamily::Unknown => Some('"'),
            '\'' if matches!(shell, ShellFamily::Posix | ShellFamily::PowerShell) => Some('\''),
            _ => None,
        };
        if let Some(q) = quote {
            in_word = true;
            let mut closed = false;
            for d in chars.by_ref() {
                if d == q {
                    closed = true;
                    break;
                }
                if d.is_control() || !quoted_char_is_literal(shell, q, d) {
                    return None;
                }
                cur.push(d);
            }
            // `""`/`''` right after a closing quote is an escape in cmd and
            // PowerShell and a concatenation in POSIX: not worth telling apart.
            if !closed || matches!(chars.peek(), Some('"') | Some('\'')) {
                return None;
            }
            // On Windows the *program* splits its own command line, and to
            // its parser a `\` run before a `"` escapes the quote.
            if matches!(shell, ShellFamily::Cmd | ShellFamily::PowerShell) && cur.ends_with('\\') {
                return None;
            }
            continue;
        }
        if !shell.bare_ok(c) {
            return None;
        }
        cur.push(c);
        in_word = true;
    }
    if in_word {
        out.push(cur);
    }
    Some(out)
}

/// Whether `d`, inside a `q`-quoted run, is taken literally by `shell`.
fn quoted_char_is_literal(shell: ShellFamily, q: char, d: char) -> bool {
    match (shell, q) {
        // POSIX single quotes take everything literally.
        (ShellFamily::Posix, '\'') => true,
        (ShellFamily::Posix, _) => !matches!(d, '$' | '`' | '\\' | '!'),
        (ShellFamily::PowerShell, '\'') => !('\u{2018}'..='\u{201B}').contains(&d),
        (ShellFamily::PowerShell, _) => {
            !matches!(d, '$' | '`') && !('\u{201C}'..='\u{201E}').contains(&d)
        }
        (ShellFamily::Cmd, _) => !matches!(d, '%' | '!' | '^'),
        (ShellFamily::Unknown, _) => false,
    }
}

/// What to do with one flag of the user's startup command on resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlagRule {
    /// Re-emit, with exactly one value.
    KeepValue,
    /// Re-emit, alone.
    KeepBool,
    /// A conversation selector (or something the resume replaces): drop it,
    /// along with an optional value.
    DropOptionalValue,
    /// Drop it and its (required) value.
    DropValue,
    /// Drop it alone.
    Drop,
}

/// Claude flags read off `claude --help` (2.1.281). Only flags that take
/// exactly one value or none are kept; variadic flags (`<x...>`: `--add-dir`,
/// `--allowed-tools`, `--mcp-config`, `--tools`, …), `-p/--print`, `--bg`,
/// `--worktree` and everything unlisted make the caller fall back to the bare
/// resume command, because where their values end — or what they do to a
/// resumed conversation — cannot be read off the command line.
fn claude_flag(flag: &str) -> Option<FlagRule> {
    use FlagRule::*;
    Some(match flag {
        "--model"
        | "--agent"
        | "--append-system-prompt"
        | "--system-prompt"
        | "--settings"
        | "--effort"
        | "--fallback-model"
        | "--setting-sources"
        | "--plugin-dir"
        | "--debug-file"
        | "--autocompact" => KeepValue,
        "--verbose"
        | "--ide"
        | "--chrome"
        | "--no-chrome"
        | "--strict-mcp-config"
        | "--disable-slash-commands"
        | "--brief"
        | "--ax-screen-reader"
        | "--exclude-dynamic-system-prompt-sections" => KeepBool,
        // Selectors, all with an optional value (`[value]` in --help).
        "-r" | "--resume" | "--from-pr" | "--teleport" => DropOptionalValue,
        "--session-id" => DropValue,
        "-c" | "--continue" | "--fork-session" => Drop,
        // Ours already; once is enough.
        CLAUDE_SKIP_PERMISSIONS | "--allow-dangerously-skip-permissions" => Drop,
        _ => return None,
    })
}

/// Codex flags read off `codex resume --help` (codex-cli 0.155): the ones it
/// lists are the only ones that can follow `codex resume <id>`. `-i/--image`
/// (variadic) and anything unlisted make the caller fall back to bare.
fn codex_flag(flag: &str) -> Option<FlagRule> {
    use FlagRule::*;
    Some(match flag {
        "-c"
        | "--config"
        | "--enable"
        | "--disable"
        | "-m"
        | "--model"
        | "--local-provider"
        | "-p"
        | "--profile"
        | "-s"
        | "--sandbox"
        | "-a"
        | "--ask-for-approval"
        | "--add-dir"
        | "--remote"
        | "--remote-auth-token-env" => KeepValue,
        "--oss"
        | "--search"
        | "--no-alt-screen"
        | "--strict-config"
        | "--approve-for-me"
        | "--dangerously-bypass-approvals-and-sandbox" => KeepBool,
        // The pane is spawned in the session's own directory already.
        "-C" | "--cd" => DropValue,
        "--last" | "--all" | "--include-non-interactive" | "--worktree" => Drop,
        _ => return None,
    })
}

/// Codex subcommands other than the selectors: a startup command running one
/// of these is not an interactive session whose flags could carry over.
const CODEX_SUBCOMMANDS: &[&str] = &[
    "exec",
    "e",
    "review",
    "login",
    "logout",
    "mcp",
    "plugin",
    "app-server",
    "remote-control",
    "app",
    "completion",
    "update",
    "doctor",
    "sandbox",
    "debug",
    "apply",
    "a",
    "queue",
    "archive",
    "delete",
    "migrate-rollouts",
    "unarchive",
    "cloud",
    "exec-server",
    "features",
    "help",
    "agents",
];

/// The user's own flags from `startup_cmd` that are safe to carry into the
/// resumed command, as raw (unquoted) arguments, plus the program token.
///
/// `None` means "type the bare resume command": the startup command does not
/// start this agent, or it cannot be split with certainty for `shell`, or it
/// carries a flag this does not know to be safe to re-emit. Positional
/// arguments are prompts and are always dropped — resending `claude "review
/// the diff"` on every resume would be a new turn each launch.
pub fn carried_args(
    startup_cmd: &str,
    agent: AgentKind,
    shell: ShellFamily,
) -> Option<(String, Vec<String>)> {
    let tokens = split_for(shell, startup_cmd)?;
    let (program, rest) = tokens.split_first()?;
    if !program_is(program, agent.as_str()) {
        return None;
    }
    let rules: fn(&str) -> Option<FlagRule> = match agent {
        AgentKind::Claude => claude_flag,
        AgentKind::Codex => codex_flag,
    };
    let mut kept = Vec::new();
    let mut seen_subcommand = false;
    let mut i = 0;
    while i < rest.len() {
        let t = rest[i].as_str();
        i += 1;
        if !t.starts_with('-') || t == "-" {
            // A positional: a prompt, a session id/name after a selector, or
            // (Codex) a subcommand.
            if agent == AgentKind::Codex && !seen_subcommand {
                seen_subcommand = true;
                if CODEX_SUBCOMMANDS.contains(&t) {
                    return None;
                }
            }
            continue;
        }
        let (flag, inline) = match t.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v)),
            _ => (t, None),
        };
        match rules(flag)? {
            FlagRule::KeepValue => {
                let value = match inline {
                    Some(v) => v.to_string(),
                    None => {
                        let v = rest.get(i)?;
                        i += 1;
                        v.clone()
                    }
                };
                kept.push(flag.to_string());
                kept.push(value);
            }
            FlagRule::KeepBool if inline.is_none() => kept.push(flag.to_string()),
            FlagRule::KeepBool => return None,
            FlagRule::DropOptionalValue => {
                if inline.is_none() && rest.get(i).is_some_and(|v| !v.starts_with('-')) {
                    i += 1;
                }
            }
            FlagRule::DropValue => {
                if inline.is_none() {
                    rest.get(i)?;
                    i += 1;
                }
            }
            FlagRule::Drop if inline.is_none() => {}
            FlagRule::Drop => return None,
        }
    }
    Some((program.clone(), kept))
}

/// The full command to type into the pane's shell: our explicit selector plus
/// whatever of the saved `startup_cmd` is provably safe to carry over, each
/// argument quoted for `shell`.
///
/// Falls back to the bare [`resume_argv`] — never to a best guess — whenever
/// the startup command is not this agent, cannot be split with certainty, has
/// a flag not known to be safe, or has an argument `shell` cannot quote with
/// certainty.
pub fn resume_command(
    agent: AgentKind,
    session_id: &str,
    startup_cmd: &str,
    shell: ShellFamily,
) -> Option<String> {
    let argv = resume_argv(agent, session_id)?;
    let bare = argv.join(" ");
    let Some((program, kept)) = carried_args(startup_cmd, agent, shell) else {
        return Some(bare);
    };
    // The user's own spelling of the program (a full path, say) survives only
    // when it needs no quoting: a quoted program needs `& ` in PowerShell and
    // is a different thing again in cmd.
    let program = if program.chars().all(|c| shell.bare_ok(c)) {
        program
    } else {
        argv[0].clone()
    };
    let mut out = vec![program];
    out.extend(argv[1..].iter().cloned());
    for arg in &kept {
        match shell.quote(arg) {
            Some(q) => out.push(q),
            None => return Some(bare),
        }
    }
    Some(out.join(" "))
}

/// Whether `token` invokes the program `name`, allowing for a path prefix and
/// a Windows `.exe`/`.cmd`/`.bat` suffix.
fn program_is(token: &str, name: &str) -> bool {
    let stem = token
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(token)
        .to_ascii_lowercase();
    let stem = stem
        .strip_suffix(".exe")
        .or_else(|| stem.strip_suffix(".cmd"))
        .or_else(|| stem.strip_suffix(".bat"))
        .unwrap_or(&stem);
    stem == name
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Pane id → session, as persisted. A `BTreeMap` so the JSON has a stable key
/// order and a save that changed nothing produces an identical file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentSessionStore {
    pub sessions: BTreeMap<Uuid, AgentSession>,
}

impl AgentSessionStore {
    pub fn get(&self, pane_id: Uuid) -> Option<&AgentSession> {
        self.sessions.get(&pane_id)
    }

    pub fn remove(&mut self, pane_id: Uuid) -> bool {
        self.sessions.remove(&pane_id).is_some()
    }

    /// Record (or refresh) the session for one pane. Returns whether anything
    /// changed, so the caller can skip a disk write.
    ///
    /// Two invariants, both about not resuming the same conversation twice:
    ///
    /// * **A session id belongs to at most one pane.** Two panes opened in the
    ///   same directory scan up the same newest transcript; resuming one id in
    ///   both forks the conversation. A later claim on an id another pane
    ///   holds is refused — unless it is exact (a hook, or the process's own
    ///   argv / pid file), which is the agent itself saying "this id is
    ///   mine", and then the other pane's record loses the id.
    /// * **A disk-scanned id never overwrites a live exact one** for the same
    ///   pane, because the exact id is the agent's word and the scan is a
    ///   guess. An inactive record belongs to an agent that has exited, and
    ///   the pane's next agent is free to replace it.
    pub fn put(&mut self, session: AgentSession) -> bool {
        if !is_valid_session_id(&session.session_id) {
            return false;
        }
        if let Some(existing) = self.sessions.get(&session.pane_id) {
            if existing.active
                && existing.source != IdSource::Disk
                && session.source == IdSource::Disk
                && existing.session_id != session.session_id
            {
                return false;
            }
        }
        let clash: Vec<Uuid> = self
            .sessions
            .iter()
            .filter(|(id, s)| **id != session.pane_id && s.session_id == session.session_id)
            .map(|(id, _)| *id)
            .collect();
        if !clash.is_empty() {
            if session.source == IdSource::Disk {
                return false;
            }
            for id in clash {
                self.sessions.remove(&id);
            }
        }
        if self.sessions.get(&session.pane_id) == Some(&session) {
            return false;
        }
        self.sessions.insert(session.pane_id, session);
        true
    }

    /// Mark a pane's session as no longer running, keeping the record.
    ///
    /// "Decline, don't delete" (spec §4): the id is still the best thing we
    /// know about that pane, so a later fix — or a `get` that only wants to
    /// display it — can still see it. It simply stops being eligible for an
    /// automatic resume.
    pub fn deactivate(&mut self, pane_id: Uuid) -> bool {
        match self.sessions.get_mut(&pane_id) {
            Some(s) if s.active => {
                s.active = false;
                true
            }
            _ => false,
        }
    }
}

/// `<config_dir>/ymux/agent-sessions.json`, with the same relative-directory
/// fallback `scrollback::scrollback_dir` uses.
pub fn store_path() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join("ymux"))
        .unwrap_or_else(|| PathBuf::from("./ymux-config"))
        .join("agent-sessions.json")
}

/// Load the store from `path`. A missing or unparseable file is an empty
/// store, never an error: a corrupt sessions file must not stop ymux starting,
/// and the worst case is that panes fall back to their normal startup command.
pub fn load_from(path: &Path) -> AgentSessionStore {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write the store to `path` via temp file + rename, mirroring
/// `scrollback::save_blob_under` and `config::store::write_atomic` so a crash
/// mid-write cannot leave a half-written JSON file behind.
pub fn save_to(path: &Path, store: &AgentSessionStore) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, path)
}

/// Load the store from the real, OS-resolved location.
pub fn load() -> AgentSessionStore {
    load_from(&store_path())
}

/// Save the store to the real, OS-resolved location.
pub fn save(store: &AgentSessionStore) -> std::io::Result<()> {
    save_to(&store_path(), store)
}

// ---------------------------------------------------------------------------
// Tracker: turning scan ticks and hook events into store records
// ---------------------------------------------------------------------------

/// How often an agent pane's transcripts are re-read while its process is not
/// yet tied to a conversation (typically: Claude is open but nothing has been
/// typed, so no transcript exists yet).
///
/// The process scan runs every 2 s; a disk scan on every tick would open files
/// for every such pane for as long as it sat there. Once a process is bound
/// its pane is never scanned again — the only thing a rescan could add is
/// somebody else's conversation.
pub const DISK_RESCAN_INTERVAL: u64 = 10;

/// Granularity `updated_at` is rounded down to.
///
/// The scan ticks every 2 s, and a record whose only change is a new
/// `updated_at` would otherwise rewrite the store file every two seconds for
/// as long as an agent is running. Quantizing the timestamp makes those ticks
/// compare equal, so `AgentSessionStore::put` reports no change and the file
/// is written about once a minute instead. The cost is up to a minute of
/// under-reporting against a 24 h freshness window, which is nothing, and it
/// errs toward calling a record stale rather than fresh.
pub const PERSIST_GRANULARITY: u64 = 60;

/// What one scan tick (or one hook event) knows about a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneObservation {
    pub pane_id: Uuid,
    /// The registry's `kind` string for the agent seen in this pane.
    pub kind: String,
    /// The pane's live cwd, as OSC 7 last reported it. Without one there is
    /// nothing to match a transcript against.
    pub cwd: Option<String>,
    /// What the agent is doing, when the caller knows (a hook does). The
    /// process scan only knows the agent is *there* and passes `None`, which
    /// keeps whatever status the record already has.
    pub status: Option<AgentStatus>,
    /// The session id a Claude Code hook relayed for this pane, when agent
    /// tracking is on. Exact once the pane's own agent process vouches for it
    /// (see [`SessionTracker::observe`]), and then it outranks anything found
    /// on disk; until then it is ignored here (the agent tree still shows
    /// it live).
    pub hook_session_id: Option<String>,
    /// The agent process the scan found in the pane. `None` from the hook
    /// listener, which cannot see processes.
    pub process: Option<PaneProcess>,
    /// The id Claude's own `~/.claude/sessions/<pid>.json` names for that
    /// process, already vetted by `agent_binding::registry_session_id`.
    pub pid_file_session_id: Option<String>,
}

/// Which process a pane's record was established by, and for which id.
///
/// In memory only: a process does not outlive the app run that saw it, so a
/// binding from a previous launch has nothing to point at.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    process: ProcessKey,
    /// `None` while the process has been seen but not yet tied to a
    /// conversation.
    session_id: Option<String>,
}

/// The store plus the bookkeeping that ties each record to a process.
///
/// Holds no Tauri types and opens no files itself — the transcript lookup is
/// passed in, so its tests drive it with a stub instead of the developer's
/// real `~/.claude`.
#[derive(Debug, Default)]
pub struct SessionTracker {
    store: AgentSessionStore,
    /// Pane id -> when its transcripts were last read (epoch seconds).
    last_disk_scan: BTreeMap<Uuid, u64>,
    /// Pane id -> the agent process running in it.
    bindings: BTreeMap<Uuid, Binding>,
    /// Pane id -> a resume handed to the frontend that no running agent has
    /// confirmed yet.
    pending: BTreeMap<Uuid, PendingResume>,
    /// Panes whose resume was just confirmed, for the caller to act on.
    confirmed: Vec<Uuid>,
    /// Panes whose resume failed this run. Their old scrollback is what the
    /// next launch restores, so nothing may overwrite it until then.
    failed: HashSet<Uuid>,
    /// A record changed since the last `take_dirty`.
    dirty: bool,
    /// Pane id -> when a hook-borne id was last checked against the pane's
    /// transcripts (epoch seconds), so an id nothing vouches for costs at
    /// most one disk read per [`DISK_RESCAN_INTERVAL`].
    last_hook_check: BTreeMap<Uuid, u64>,
}

/// A resume plan the frontend is carrying out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingResume {
    session_id: String,
    /// Epoch seconds after which an unconfirmed resume counts as failed.
    deadline: u64,
}

/// How long a resumed agent has to show up running before the resume is
/// declared failed and its record declined, so the same failing command is
/// not typed again on every launch. Generous: the shell has to start, run
/// its profile, and then start the agent, and the scan only looks every 2 s.
pub const RESUME_CONFIRM_WINDOW: u64 = 90;

/// How long a resumed agent must have been running before a sighting of it
/// confirms the resume. `claude --resume <id>` for a conversation it cannot
/// load prints an error and exits within a second or two — long enough for
/// one 2 s scan tick to see it.
pub const RESUME_CONFIRM_MIN_UPTIME: u64 = 10;

/// What `save_scrollback` should do with a pane's blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbackAction {
    /// A plain shell pane (or one whose agent has gone): save as always.
    Save,
    /// A resume is in flight: keep whatever is on disk, write nothing. If the
    /// resume fails, the old blob is still there for the next launch.
    Skip,
    /// A live agent pane: its blob would be a picture of a conversation that
    /// resumes for real, so none is kept (spec §5).
    Delete,
}

impl SessionTracker {
    pub fn from_store(store: AgentSessionStore) -> Self {
        Self {
            store,
            ..Default::default()
        }
    }

    pub fn store(&self) -> &AgentSessionStore {
        &self.store
    }

    pub fn get(&self, pane_id: Uuid) -> Option<&AgentSession> {
        self.store.get(pane_id)
    }

    /// Whether anything changed since this was last called, clearing the flag.
    /// The caller uses it to skip a disk write on an idle tick.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Whether `pane_id`'s transcripts should be read on this tick.
    ///
    /// Only for a pane whose agent is not yet tied to a conversation, and then
    /// at most once every [`DISK_RESCAN_INTERVAL`]. A pane whose live agent's
    /// id came from a hook is not scanned: the agent already told us, exactly.
    /// Once that agent has exited (the record is inactive) the pane is
    /// scanned again, so the next agent in it gets a binding of its own.
    pub fn wants_disk_scan(&self, pane_id: Uuid, now: u64) -> bool {
        if self
            .store
            .get(pane_id)
            .is_some_and(|s| s.source == IdSource::Hook && s.active)
        {
            return false;
        }
        if self
            .bindings
            .get(&pane_id)
            .is_some_and(|b| b.session_id.is_some())
        {
            return false;
        }
        match self.last_disk_scan.get(&pane_id) {
            None => true,
            Some(last) => now.saturating_sub(*last) >= DISK_RESCAN_INTERVAL,
        }
    }

    /// Whether the pane's own agent process vouches for the hook-borne `id`
    /// (see [`Self::observe`]). `transcript_has_it` reads the pane's
    /// transcripts; it is called at most once per [`DISK_RESCAN_INTERVAL`]
    /// per pane, and only when nothing cheaper settled it.
    fn hook_id_vouched(
        &mut self,
        agent: AgentKind,
        obs: &PaneObservation,
        id: &str,
        now: u64,
        transcript_has_it: &mut dyn FnMut() -> bool,
    ) -> bool {
        let pane = obs.pane_id;
        // Already tied to this id by an earlier vouched-for observation of
        // the same process (`observe` has released a binding whose process
        // is gone before this runs).
        if self
            .bindings
            .get(&pane)
            .is_some_and(|b| b.session_id.as_deref() == Some(id))
        {
            return true;
        }
        if agent != AgentKind::Claude {
            return false;
        }
        let Some(process) = obs.process.as_ref() else {
            return false;
        };
        if obs.pid_file_session_id.as_deref() == Some(id)
            || argv_session_id(agent, &process.argv).as_deref() == Some(id)
            || process.argv.iter().any(|a| a == id)
        {
            return true;
        }
        if !obs.cwd.as_deref().is_some_and(|c| !c.is_empty()) {
            return false;
        }
        if self
            .last_hook_check
            .get(&pane)
            .is_some_and(|last| now.saturating_sub(*last) < DISK_RESCAN_INTERVAL)
        {
            return false;
        }
        self.last_hook_check.insert(pane, now);
        transcript_has_it()
    }

    /// Fold one observation into the store.
    ///
    /// The record for a pane is always about the agent *process* running in
    /// it (see `agent_binding` for the sources and their order):
    ///
    /// * A new process in the pane ends the previous one's claim: its record
    ///   is declined until the new process is tied to a conversation — which
    ///   may well be the same one, when it was started with `--resume <id>`.
    /// * An exact id (hook, Claude's pid file, argv) is taken as given and may
    ///   move the pane to a new conversation (`/clear`, `/resume`) — but a
    ///   hook's id only once the pane's own Claude process vouches for it:
    ///   the pane is already bound to that id, or the scan sees a Claude
    ///   process there whose pid file or argv names it, or whose pane cwd
    ///   holds that transcript (`ypath::same_path` on its recorded cwd). Any
    ///   process with the hook token can POST any id, and the resume it
    ///   would steer runs with `--dangerously-skip-permissions`.
    /// * Otherwise transcripts are read, through `lookup`, only while the
    ///   process is unbound, and a guess is made only when it is unambiguous
    ///   ([`crate::agent_binding::guess_transcript`]). Once made it is never
    ///   changed while that process lives.
    ///
    /// `others` is every other agent process on the machine; the tracker adds
    /// the ids it has already bound other panes' processes to.
    ///
    /// An agent ymux has no resume story for (Gemini and the rest — spec §8)
    /// is ignored rather than recorded with no way to act on it.
    pub fn observe<F>(&mut self, obs: &PaneObservation, now: u64, others: &[OtherAgent], lookup: F)
    where
        F: FnOnce(AgentKind, &str) -> Vec<crate::agent_scan_disk::DiskSession>,
    {
        let Some(agent) = AgentKind::from_kind(&obs.kind) else {
            return;
        };
        let pane = obs.pane_id;
        // A different process from the one this pane was bound to: that one
        // is gone, and whatever it was running goes with it.
        if let Some(p) = &obs.process {
            if self
                .bindings
                .get(&pane)
                .is_some_and(|b| b.process != p.key())
            {
                self.release(pane);
            }
        }
        let key = obs
            .process
            .as_ref()
            .map(PaneProcess::key)
            .or_else(|| self.bindings.get(&pane).map(|b| b.process));

        // At most one transcript read per observation, shared by the hook
        // check below and the guess further down.
        let mut lookup = Some(lookup);
        let mut looked: Option<Vec<crate::agent_scan_disk::DiskSession>> = None;
        let hook_id = obs
            .hook_session_id
            .as_deref()
            .filter(|s| is_valid_session_id(s))
            .filter(|id| {
                self.hook_id_vouched(agent, obs, id, now, &mut || {
                    let cwd = obs.cwd.as_deref().unwrap_or_default();
                    let found = lookup.take().map(|f| f(agent, cwd)).unwrap_or_default();
                    let hit = found
                        .iter()
                        .any(|d| d.session_id == *id && ypath::same_path(&d.cwd, cwd));
                    looked = Some(found);
                    hit
                })
            })
            .map(str::to_string);

        // Exact ids: the (vouched-for) hook, Claude's pid file, the
        // process's own argv.
        let exact = hook_id
            .map(|s| (s, IdSource::Hook))
            .or_else(|| {
                obs.pid_file_session_id
                    .clone()
                    .filter(|s| is_valid_session_id(s))
                    .map(|s| (s, IdSource::Process))
            })
            .or_else(|| {
                obs.process
                    .as_ref()
                    .and_then(|p| argv_session_id(agent, &p.argv))
                    .map(|s| (s, IdSource::Process))
            });
        if let Some((id, source)) = exact {
            let existing = self.store.get(pane);
            // Keep a cwd the record already had for this very conversation
            // (a transcript's own spelling beats the shell's), else the
            // pane's live one, else whatever the record had.
            let cwd = existing
                .filter(|s| s.session_id == id && !s.cwd.is_empty())
                .map(|s| s.cwd.clone())
                .or_else(|| obs.cwd.clone().filter(|c| !c.is_empty()))
                .or_else(|| existing.map(|s| s.cwd.clone()))
                .unwrap_or_default();
            self.record(agent, obs, &id, cwd, source, now);
            // Only an exact id confirms a resume (the resumed transcript
            // began before the process did, so a guess never could), and
            // only once the process has outlived a resume that fails fast.
            let settled =
                key.is_some_and(|k| now.saturating_sub(k.start_secs) >= RESUME_CONFIRM_MIN_UPTIME);
            if settled && self.pending.get(&pane).is_some_and(|p| p.session_id == id) {
                self.pending.remove(&pane);
                self.confirmed.push(pane);
            }
            if let Some(process) = key {
                self.bindings.insert(
                    pane,
                    Binding {
                        process,
                        session_id: Some(id),
                    },
                );
            }
            return;
        }

        // Nothing exact, and no process to reason about (a hook event that
        // carried no id): nothing to do.
        let Some(process) = obs.process.as_ref() else {
            return;
        };
        match self.bindings.get(&pane) {
            Some(Binding {
                session_id: Some(id),
                ..
            }) => {
                // Already tied to this very process. Refresh `updated_at`
                // ("when ymux last saw this agent alive", which the 24 h
                // window measures) — only while the record is the live one
                // for that id, never reviving one that was declined.
                let id = id.clone();
                if let Some(existing) = self
                    .store
                    .get(pane)
                    .filter(|s| s.active && s.session_id == id)
                {
                    let (cwd, source) = (existing.cwd.clone(), existing.source);
                    self.record(agent, obs, &id, cwd, source, now);
                }
                return;
            }
            Some(_) => {}
            None => {
                // First sight of this process. Whatever the pane's record
                // says was established by an earlier process (the previous
                // launch, or an agent that exited between two ticks), so it
                // is declined until this one is tied to a conversation.
                self.bindings.insert(
                    pane,
                    Binding {
                        process: process.key(),
                        session_id: None,
                    },
                );
                self.dirty |= self.store.deactivate(pane);
            }
        }

        if !self.wants_disk_scan(pane, now) {
            return;
        }
        let Some(cwd) = obs.cwd.as_deref().filter(|c| !c.is_empty()) else {
            return;
        };
        self.last_disk_scan.insert(pane, now);
        let claimed: HashSet<String> = self
            .store
            .sessions
            .iter()
            .filter(|(id, _)| **id != pane)
            .map(|(_, s)| s.session_id.clone())
            .collect();
        // Another pane's process that is already tied to a conversation is
        // not a candidate author of this one's.
        let others: Vec<OtherAgent> = others
            .iter()
            .map(|o| {
                let mut o = o.clone();
                if o.known_id.is_none() {
                    o.known_id = self
                        .bindings
                        .iter()
                        .find(|(p, b)| {
                            **p != pane
                                && b.process
                                    == ProcessKey {
                                        pid: o.pid,
                                        start_secs: o.start_secs,
                                    }
                        })
                        .and_then(|(_, b)| b.session_id.clone());
                }
                o
            })
            .collect();
        let candidates = match looked {
            Some(c) => c,
            None => lookup.take().map(|f| f(agent, cwd)).unwrap_or_default(),
        };
        let Some(found) = guess_transcript(agent, cwd, process, &candidates, &claimed, &others)
        else {
            return;
        };
        self.record(
            agent,
            obs,
            &found.session_id,
            found.cwd,
            IdSource::Disk,
            now,
        );
        if self
            .store
            .get(pane)
            .is_some_and(|s| s.active && s.session_id == found.session_id)
        {
            if let Some(b) = self.bindings.get_mut(&pane) {
                b.session_id = Some(found.session_id);
            }
        }
    }

    fn record(
        &mut self,
        agent: AgentKind,
        obs: &PaneObservation,
        session_id: &str,
        cwd: String,
        source: IdSource,
        now: u64,
    ) {
        // No status from the caller: keep the record's own for this same
        // conversation, else `working` — the conservative answer for
        // `interrupted`.
        let state = obs
            .status
            .or_else(|| {
                self.store
                    .get(obs.pane_id)
                    .filter(|s| s.session_id == session_id)
                    .map(|s| s.state)
            })
            .unwrap_or(AgentStatus::Working);
        let changed = self.store.put(AgentSession {
            pane_id: obs.pane_id,
            agent,
            session_id: session_id.to_string(),
            cwd,
            state,
            interrupted: state != AgentStatus::Done,
            active: true,
            source,
            updated_at: now - (now % PERSIST_GRANULARITY),
        });
        self.dirty |= changed;
    }

    /// Drop the pane's process binding and decline its record.
    fn release(&mut self, pane_id: Uuid) {
        self.bindings.remove(&pane_id);
        self.last_disk_scan.remove(&pane_id);
        self.dirty |= self.store.deactivate(pane_id);
    }

    /// The agent in `pane_id` is gone while the pane itself lives on — the
    /// user quit it. Stop offering to resume that conversation, but keep the
    /// record ("decline, don't delete", spec §4).
    ///
    /// Deliberately *not* called when a pane disappears: at app shutdown every
    /// PTY dies at once, and treating that as "the user quit the agent" would
    /// erase exactly the records the next launch needs.
    pub fn note_agent_exit(&mut self, pane_id: Uuid) {
        self.release(pane_id);
    }

    /// Whether `pane_id` should stop persisting its scrollback (spec §5).
    ///
    /// True for any pane with a fresh, active record — *not* only one that
    /// was resumed at spawn. A pane running Claude for the first time has a
    /// fresh record but was not resumed, and if it kept saving, the blob it
    /// wrote would sit on disk unread (the next launch resumes and skips it)
    /// until the record went stale or the user quit the agent — and would
    /// then be replayed, putting a dead Claude screen back on the display
    /// this feature exists to clear.
    pub fn suppresses_scrollback(&self, pane_id: Uuid, now: u64) -> bool {
        self.store
            .get(pane_id)
            .is_some_and(|s| s.is_fresh_at(now) && !s.cwd.is_empty())
    }

    /// What to do with `pane_id`'s scrollback blob. The backend alone
    /// decides; the frontend just offers the blob.
    pub fn scrollback_action(&self, pane_id: Uuid, now: u64) -> ScrollbackAction {
        if self.pending.contains_key(&pane_id) {
            ScrollbackAction::Skip
        } else if self.suppresses_scrollback(pane_id, now) {
            ScrollbackAction::Delete
        } else if self.failed.contains(&pane_id) {
            ScrollbackAction::Skip
        } else {
            ScrollbackAction::Save
        }
    }

    /// Act on the outcome `get_agent_session` is handing the frontend: a
    /// resume starts its confirmation window, and a session whose transcript
    /// is gone is declined now — left active, it would keep the pane's
    /// scrollback deleted for a day and repeat the banner every launch.
    pub fn note_outcome(&mut self, pane_id: Uuid, outcome: &ResumeOutcome, now: u64) {
        match outcome {
            ResumeOutcome::Resume { .. } => {
                if let Some(id) = self.store.get(pane_id).map(|s| s.session_id.clone()) {
                    self.begin_resume(pane_id, &id, now);
                }
            }
            ResumeOutcome::Missing { .. } => {
                self.dirty |= self.store.deactivate(pane_id);
            }
            ResumeOutcome::None => {}
        }
    }

    /// The frontend is about to type the resume command for `session_id`
    /// into `pane_id`. Until a running agent confirms it (by an exact id —
    /// its argv, pid file or hook), the pane's old scrollback is kept.
    pub fn begin_resume(&mut self, pane_id: Uuid, session_id: &str, now: u64) {
        self.pending.insert(
            pane_id,
            PendingResume {
                session_id: session_id.to_string(),
                deadline: now + RESUME_CONFIRM_WINDOW,
            },
        );
    }

    /// Decline every resume whose deadline passed without an agent showing
    /// up: the command failed (bad id, agent not installed, exited at once),
    /// and typing it again on every launch for a day would not fix it.
    pub fn expire_pending(&mut self, now: u64) {
        let expired: Vec<Uuid> = self
            .pending
            .iter()
            .filter(|(_, p)| now > p.deadline)
            .map(|(id, _)| *id)
            .collect();
        for pane in expired {
            self.pending.remove(&pane);
            self.failed.insert(pane);
            self.dirty |= self.store.deactivate(pane);
        }
    }

    /// Panes whose resume a running agent has confirmed since the last call.
    /// Their saved scrollback is now dead history and can go.
    pub fn take_confirmed(&mut self) -> Vec<Uuid> {
        std::mem::take(&mut self.confirmed)
    }

    /// Drop every record whose pane is not in `panes` — the panes the saved
    /// layout still has. A pane closed while ymux was not running to see it
    /// (a crash, a hand-edited config, a deleted workspace whose frontend
    /// cleanup never ran) otherwise keeps its record forever.
    ///
    /// Only for a layout that was actually read from disk: pruning against a
    /// default or fallback config would wipe every record.
    pub fn retain_panes(&mut self, panes: &HashSet<Uuid>) {
        let before = self.store.sessions.len();
        self.store.sessions.retain(|id, _| panes.contains(id));
        self.dirty |= self.store.sessions.len() != before;
    }

    /// The user closed the pane for good. Mirrors `delete_scrollback`.
    pub fn forget(&mut self, pane_id: Uuid) {
        self.dirty |= self.store.remove(pane_id);
        self.last_disk_scan.remove(&pane_id);
        self.bindings.remove(&pane_id);
        self.pending.remove(&pane_id);
        self.failed.remove(&pane_id);
    }
}

/// Tauri-managed handle: `app.manage(SharedSessions::default())`.
#[derive(Default)]
pub struct SharedSessions(pub parking_lot::Mutex<SessionTracker>);

/// What the frontend needs in order to choose resume over scrollback replay
/// (spec §4). Serialized by `get_agent_session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResumePlan {
    /// The agent's kind string, for the banner.
    pub agent: String,
    /// The full command line to type into the pane's shell.
    pub command: String,
    /// The directory the resumed pane must be spawned in. Claude sessions are
    /// project-scoped, so resuming one from the wrong cwd finds nothing —
    /// and the pane's own saved `cwd` may since have drifted.
    pub cwd: String,
    /// How long ago ymux last saw this session, in seconds — the banner's
    /// "3 hours ago".
    pub age_secs: u64,
}

/// What a pane should do as it comes up.
///
/// Three outcomes, not two, because spec §4.3 asks for a different line when
/// the id was dropped: "이전 세션을 찾지 못해 새로 시작합니다". Without the
/// `Missing` arm the frontend cannot tell "this pane never held an agent"
/// from "it did, and the transcript is gone" — and the second is the one
/// worth saying out loud, because the user is about to lose a conversation
/// they expected back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ResumeOutcome {
    /// Resume this session.
    Resume { plan: ResumePlan },
    /// A session was recorded here and is recent, but its transcript is gone
    /// — pruned by the agent itself, or by a `~/.claude` cleanup. The pane
    /// starts normally and says so.
    Missing { agent: String },
    /// Nothing to say: no record, or one too old to act on.
    None,
}

/// Decide the outcome for one pane.
///
/// Resume needs all of: a fresh, active record, a cwd to resume in, a session
/// id safe to type into a shell, and a transcript still on disk. A
/// conversation the agent itself has pruned cannot be resumed, and
/// `claude --resume <gone>` would only error into the user's face.
///
/// A record that is merely *stale* gets no banner. It is not news that a pane
/// the user has not touched in over a day starts as a plain shell.
pub fn outcome_for(
    session: Option<&AgentSession>,
    startup_cmd: &str,
    shell: ShellFamily,
    now: u64,
    transcript_exists: impl FnOnce(&AgentSession) -> bool,
) -> ResumeOutcome {
    let Some(s) = session else {
        return ResumeOutcome::None;
    };
    // An empty cwd means the record was written from a hook before OSC 7 had
    // reported one. Resuming somewhere arbitrary is worse than not resuming:
    // Claude sessions are project-scoped, so the wrong directory finds
    // nothing, and the user gets an error instead of their conversation.
    if !s.is_fresh_at(now) || s.cwd.is_empty() {
        return ResumeOutcome::None;
    }
    let Some(command) = resume_command(s.agent, &s.session_id, startup_cmd, shell) else {
        return ResumeOutcome::None;
    };
    if !transcript_exists(s) {
        return ResumeOutcome::Missing {
            agent: s.agent.as_str().to_string(),
        };
    }
    ResumeOutcome::Resume {
        plan: ResumePlan {
            agent: s.agent.as_str().to_string(),
            command,
            cwd: s.cwd.clone(),
            age_secs: now.saturating_sub(s.updated_at),
        },
    }
}

/// Whether the transcript naming `session.session_id` is still on disk.
///
/// Delegates to a search *by id* rather than a path rebuilt from the record's
/// `cwd` — see `agent_scan_disk::transcript_exists_under` for why both agents
/// need that.
pub fn transcript_exists(session: &AgentSession) -> bool {
    static CACHE: std::sync::OnceLock<parking_lot::Mutex<ExistsCache>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (session.agent, session.session_id.clone());
    let now = std::time::Instant::now();
    if let Some(hit) = cache.lock().get(&key, now) {
        return hit;
    }
    // The walk runs outside the cache lock: two panes checking at once may
    // both walk, which is cheaper than serialising every spawn on one walk.
    let found =
        crate::agent_scan_disk::transcript_exists(session.agent, &session.session_id, &session.cwd);
    cache.lock().put(key, found, now);
    found
}

/// How long an answer to "is this transcript still on disk?" is reused.
pub const EXISTS_CACHE_TTL: Duration = Duration::from_secs(30);

/// Recent existence answers, so a burst of pane spawns (a workspace restore)
/// or a quick relaunch does not walk the transcript tree once per pane.
#[derive(Debug, Default)]
pub struct ExistsCache {
    entries: BTreeMap<(AgentKind, String), (bool, std::time::Instant)>,
}

impl ExistsCache {
    pub fn get(&self, key: &(AgentKind, String), now: std::time::Instant) -> Option<bool> {
        self.entries
            .get(key)
            .filter(|(_, at)| now.saturating_duration_since(*at) < EXISTS_CACHE_TTL)
            .map(|(found, _)| *found)
    }

    pub fn put(&mut self, key: (AgentKind, String), found: bool, now: std::time::Instant) {
        self.entries
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < EXISTS_CACHE_TTL);
        self.entries.insert(key, (found, now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(pane: Uuid, id: &str, source: IdSource) -> AgentSession {
        AgentSession {
            pane_id: pane,
            agent: AgentKind::Claude,
            session_id: id.to_string(),
            cwd: "D:\\Git\\ymux".to_string(),
            state: AgentStatus::Working,
            interrupted: true,
            active: true,
            source,
            updated_at: 1_000_000,
        }
    }

    #[test]
    fn resume_argv_per_agent() {
        // Both spellings checked against the installed CLIs' own `--help`:
        // `claude --help` -> `-r, --resume [value]`;
        // `codex resume --help` -> `codex resume [OPTIONS] [SESSION_ID]`.
        assert_eq!(
            resume_argv(AgentKind::Claude, "20aebce7-f8e2-4582-b480-bf1ae90d7a0b"),
            Some(vec![
                "claude".to_string(),
                "--resume".to_string(),
                "20aebce7-f8e2-4582-b480-bf1ae90d7a0b".to_string(),
                CLAUDE_SKIP_PERMISSIONS.to_string(),
            ])
        );
        assert_eq!(
            resume_argv(AgentKind::Codex, "01a07644-42b3-7183-a7fd-70379b88af1f"),
            Some(vec![
                "codex".to_string(),
                "resume".to_string(),
                "01a07644-42b3-7183-a7fd-70379b88af1f".to_string(),
            ])
        );
    }

    #[test]
    fn resume_argv_rejects_unsafe_ids() {
        for bad in [
            "",
            "abc; rm -rf /",
            "$(whoami)",
            "my session",
            "../../etc/passwd",
            "------",
            &"a".repeat(65),
        ] {
            assert_eq!(
                resume_argv(AgentKind::Claude, bad),
                None,
                "must refuse to type {bad:?} into a shell"
            );
        }
    }

    const RID: &str = "abcd-1234";

    fn cmd(agent: AgentKind, startup: &str, shell: ShellFamily) -> String {
        resume_command(agent, RID, startup, shell).expect("valid id")
    }

    fn claude_bare() -> String {
        format!("claude --resume {RID} --dangerously-skip-permissions")
    }

    const ALL_SHELLS: [ShellFamily; 4] = [
        ShellFamily::Posix,
        ShellFamily::PowerShell,
        ShellFamily::Cmd,
        ShellFamily::Unknown,
    ];

    #[test]
    fn shell_family_comes_from_the_profile_executable() {
        for (exe, fam) in [
            ("C:\\Windows\\System32\\cmd.exe", ShellFamily::Cmd),
            ("powershell.exe", ShellFamily::PowerShell),
            (
                "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
                ShellFamily::PowerShell,
            ),
            ("C:\\Program Files\\Git\\bin\\bash.exe", ShellFamily::Posix),
            ("/bin/zsh", ShellFamily::Posix),
            ("/opt/homebrew/bin/fish", ShellFamily::Unknown),
            ("C:\\Windows\\System32\\wsl.exe", ShellFamily::Unknown),
            ("", ShellFamily::Unknown),
        ] {
            assert_eq!(ShellFamily::from_executable(exe), fam, "{exe}");
        }
    }

    #[test]
    fn every_selector_in_the_startup_command_is_dropped() {
        for startup in [
            "claude -c",
            "claude --continue",
            "claude --resume old-id-1234",
            "claude -r old-id-1234",
            "claude --resume=old-id-1234",
            "claude --resume old-id-1234 --fork-session",
            "claude --session-id 11111111-2222-3333-4444-555555555555",
            "claude --teleport",
            "claude --from-pr 12",
            "claude --dangerously-skip-permissions",
        ] {
            for shell in [
                ShellFamily::Posix,
                ShellFamily::PowerShell,
                ShellFamily::Cmd,
            ] {
                assert_eq!(
                    cmd(AgentKind::Claude, startup, shell),
                    claude_bare(),
                    "{startup}"
                );
            }
        }
        for startup in [
            "codex resume 01a07644-42b3-7183-a7fd-70379b88af1f",
            "codex resume --last",
            "codex fork 01a07644-42b3-7183-a7fd-70379b88af1f",
            "codex -m o3 resume --last",
            "codex resume my-name",
        ] {
            let got = cmd(AgentKind::Codex, startup, ShellFamily::Posix);
            assert!(
                got == format!("codex resume {RID}") || got == format!("codex resume {RID} -m o3"),
                "{startup} -> {got}"
            );
            assert!(!got.contains("--last") && !got.contains("my-name") && !got.contains("fork"));
            assert_eq!(got.matches("resume").count(), 1, "{got}");
        }
    }

    #[test]
    fn known_flags_are_carried_over() {
        assert_eq!(
            cmd(
                AgentKind::Claude,
                "claude -c --model opus --verbose",
                ShellFamily::Posix
            ),
            format!("{} --model opus --verbose", claude_bare())
        );
        assert_eq!(
            cmd(AgentKind::Claude, "claude --model=opus", ShellFamily::Cmd),
            format!("{} --model opus", claude_bare())
        );
        assert_eq!(
            cmd(
                AgentKind::Codex,
                "codex -m o3 resume --last --search",
                ShellFamily::PowerShell
            ),
            format!("codex resume {RID} -m o3 --search")
        );
    }

    #[test]
    fn a_prompt_in_the_startup_command_is_not_resent_on_every_resume() {
        for shell in [
            ShellFamily::Posix,
            ShellFamily::PowerShell,
            ShellFamily::Cmd,
        ] {
            assert_eq!(
                cmd(AgentKind::Claude, "claude \"review diff\"", shell),
                claude_bare()
            );
            assert_eq!(
                cmd(
                    AgentKind::Claude,
                    "claude --model opus \"review diff\"",
                    shell
                ),
                format!("{} --model opus", claude_bare())
            );
        }
        assert_eq!(
            cmd(
                AgentKind::Codex,
                "codex \"fix the build\"",
                ShellFamily::Posix
            ),
            format!("codex resume {RID}")
        );
    }

    #[test]
    fn a_quoted_value_is_requoted_for_each_shell() {
        // The review's case: `; no emoji` must stay inside the argument.
        let startup_posix = "claude --append-system-prompt 'be terse; no emoji'";
        assert_eq!(
            cmd(AgentKind::Claude, startup_posix, ShellFamily::Posix),
            format!(
                "{} --append-system-prompt 'be terse; no emoji'",
                claude_bare()
            )
        );
        let startup_dq = "claude --append-system-prompt \"be terse; no emoji\"";
        assert_eq!(
            cmd(AgentKind::Claude, startup_dq, ShellFamily::Posix),
            format!(
                "{} --append-system-prompt 'be terse; no emoji'",
                claude_bare()
            )
        );
        assert_eq!(
            cmd(AgentKind::Claude, startup_dq, ShellFamily::PowerShell),
            format!(
                "{} --append-system-prompt 'be terse; no emoji'",
                claude_bare()
            )
        );
        assert_eq!(
            cmd(AgentKind::Claude, startup_dq, ShellFamily::Cmd),
            format!(
                "{} --append-system-prompt \"be terse; no emoji\"",
                claude_bare()
            )
        );
        // A shell we cannot quote for gets the bare command, not a guess.
        assert_eq!(
            cmd(AgentKind::Claude, startup_dq, ShellFamily::Unknown),
            claude_bare()
        );
        // Embedded single quotes.
        assert_eq!(
            cmd(
                AgentKind::Claude,
                "claude --append-system-prompt \"don't\"",
                ShellFamily::Posix
            ),
            format!("{} --append-system-prompt 'don'\\''t'", claude_bare())
        );
        assert_eq!(
            cmd(
                AgentKind::Claude,
                "claude --append-system-prompt \"don't\"",
                ShellFamily::PowerShell
            ),
            format!("{} --append-system-prompt 'don''t'", claude_bare())
        );
    }

    #[test]
    fn anything_not_provably_safe_falls_back_to_the_bare_command() {
        for startup in [
            // Operators and expansions outside quotes.
            "claude --model opus; rm -rf ~",
            "claude --model opus && echo hi",
            "claude --model $MODEL",
            "claude --model `x`",
            "claude --model %MODEL%",
            "claude --model opus | tee log",
            "claude --model (opus)",
            // Unbalanced or doubled quotes.
            "claude --append-system-prompt \"unterminated",
            "claude --append-system-prompt \"a\"\"b\"",
            // A flag whose values cannot be delimited, or that changes what
            // the session is.
            "claude --add-dir a b",
            "claude --allowed-tools Bash Edit",
            "claude -p hello",
            "claude --worktree",
            "claude --some-future-flag x",
            // A value flag with no value.
            "claude --model",
            // Codex: a non-interactive subcommand, a variadic flag.
            "codex exec --model o3",
            "codex -i a.png resume --last",
        ] {
            let agent = if startup.starts_with("codex") {
                AgentKind::Codex
            } else {
                AgentKind::Claude
            };
            let bare = resume_argv(agent, RID).unwrap().join(" ");
            for shell in ALL_SHELLS {
                assert_eq!(cmd(agent, startup, shell), bare, "{startup} in {shell:?}");
            }
        }
        // `$` expands inside double quotes in POSIX shells and PowerShell;
        // cmd takes it literally.
        let dollar = "claude --append-system-prompt \"hi $USER\"";
        for shell in [ShellFamily::Posix, ShellFamily::PowerShell] {
            assert_eq!(cmd(AgentKind::Claude, dollar, shell), claude_bare());
        }
        assert_eq!(
            cmd(AgentKind::Claude, dollar, ShellFamily::Cmd),
            format!("{} --append-system-prompt \"hi $USER\"", claude_bare())
        );
    }

    #[test]
    fn cmd_refuses_what_it_cannot_quote() {
        for value in ["\"50%\"", "\"hi!\"", "\"C:\\dir\\\\\""] {
            let startup = format!("claude --append-system-prompt {value}");
            assert_eq!(
                cmd(AgentKind::Claude, &startup, ShellFamily::Cmd),
                claude_bare(),
                "{startup}"
            );
        }
    }

    #[test]
    fn single_quotes_mean_nothing_to_cmd() {
        // cmd passes `'be` and `terse'` as two arguments; not something to
        // reason about.
        assert_eq!(
            cmd(
                AgentKind::Claude,
                "claude --append-system-prompt 'be terse'",
                ShellFamily::Cmd
            ),
            claude_bare()
        );
    }

    #[test]
    fn a_different_program_is_left_alone() {
        for startup in ["echo --resume x", "", "npm run dev", "codex"] {
            assert_eq!(
                cmd(AgentKind::Claude, startup, ShellFamily::Posix),
                claude_bare(),
                "{startup}"
            );
        }
    }

    #[test]
    fn the_programs_own_spelling_survives_only_when_it_needs_no_quoting() {
        // A Windows path is a plain word in cmd and PowerShell...
        assert_eq!(
            cmd(
                AgentKind::Claude,
                "C:\\bin\\claude.exe -c",
                ShellFamily::PowerShell
            ),
            format!("C:\\bin\\claude.exe --resume {RID} --dangerously-skip-permissions")
        );
        assert_eq!(
            cmd(
                AgentKind::Claude,
                "C:\\bin\\claude.exe -c",
                ShellFamily::Cmd
            ),
            format!("C:\\bin\\claude.exe --resume {RID} --dangerously-skip-permissions")
        );
        // ...but in Git Bash its backslashes are escapes: plain `claude`.
        assert_eq!(
            cmd(
                AgentKind::Claude,
                "C:\\bin\\claude.exe -c",
                ShellFamily::Posix
            ),
            claude_bare()
        );
        assert_eq!(
            cmd(
                AgentKind::Claude,
                "/usr/local/bin/claude --continue",
                ShellFamily::Posix
            ),
            format!("/usr/local/bin/claude --resume {RID} --dangerously-skip-permissions")
        );
        // A path with a space would need `& '…'` in PowerShell.
        assert_eq!(
            cmd(
                AgentKind::Claude,
                "\"C:\\Program Files\\claude\\claude.exe\" --model opus",
                ShellFamily::PowerShell
            ),
            format!("{} --model opus", claude_bare())
        );
    }

    #[test]
    fn quoting_round_trips_per_shell() {
        assert_eq!(ShellFamily::Posix.quote("a b"), Some("'a b'".into()));
        assert_eq!(ShellFamily::Posix.quote("it's"), Some("'it'\\''s'".into()));
        assert_eq!(ShellFamily::Posix.quote("C:\\x"), Some("'C:\\x'".into()));
        assert_eq!(ShellFamily::Posix.quote("a\nb"), None);
        assert_eq!(
            ShellFamily::PowerShell.quote("it's"),
            Some("'it''s'".into())
        );
        assert_eq!(ShellFamily::PowerShell.quote("C:\\x"), Some("C:\\x".into()));
        assert_eq!(ShellFamily::PowerShell.quote("a \"b\""), None);
        assert_eq!(ShellFamily::PowerShell.quote("a\u{2019}b c"), None);
        assert_eq!(ShellFamily::PowerShell.quote("dir\\ x\\"), None);
        assert_eq!(ShellFamily::Cmd.quote("a & b"), Some("\"a & b\"".into()));
        assert_eq!(ShellFamily::Cmd.quote("50% off"), None);
        assert_eq!(ShellFamily::Unknown.quote("a b"), None);
        assert_eq!(ShellFamily::Unknown.quote("opus"), Some("opus".into()));
    }

    #[test]
    fn freshness_boundary_is_24h() {
        let s = session(Uuid::nil(), "abc-123", IdSource::Disk);
        let t = s.updated_at;
        let day = FRESH_WINDOW.as_secs();
        assert!(s.is_fresh_at(t), "same instant is fresh");
        assert!(s.is_fresh_at(t + day - 1));
        assert!(s.is_fresh_at(t + day), "exactly 24h still counts");
        assert!(!s.is_fresh_at(t + day + 1), "one second past 24h is stale");
        // Clock skew: a record stamped in the future is not thrown away.
        assert!(s.is_fresh_at(t - 10_000));
    }

    #[test]
    fn deactivated_record_is_never_fresh() {
        let mut s = session(Uuid::nil(), "abc-123", IdSource::Disk);
        s.active = false;
        assert!(!s.is_fresh_at(s.updated_at));
    }

    #[test]
    fn put_refuses_a_session_id_another_pane_already_holds() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let mut store = AgentSessionStore::default();
        assert!(store.put(session(a, "aaaa-0001", IdSource::Disk)));
        // Two panes in the same directory scan up the same newest transcript.
        assert!(!store.put(session(b, "aaaa-0001", IdSource::Disk)));
        assert!(store.get(b).is_none());
        assert_eq!(
            store.get(a).map(|s| s.session_id.as_str()),
            Some("aaaa-0001")
        );
    }

    #[test]
    fn a_hook_id_takes_the_session_from_a_scanned_pane() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let mut store = AgentSessionStore::default();
        assert!(store.put(session(a, "aaaa-0001", IdSource::Disk)));
        assert!(store.put(session(b, "aaaa-0001", IdSource::Hook)));
        assert!(
            store.get(a).is_none(),
            "the guess loses to the agent itself"
        );
        assert_eq!(store.get(b).map(|s| s.source), Some(IdSource::Hook));
    }

    #[test]
    fn a_disk_scan_never_overwrites_a_hook_id_for_the_same_pane() {
        let a = Uuid::from_u128(1);
        let mut store = AgentSessionStore::default();
        assert!(store.put(session(a, "bbbb-0001", IdSource::Hook)));
        assert!(!store.put(session(a, "cccc-0002", IdSource::Disk)));
        assert_eq!(
            store.get(a).map(|s| s.session_id.as_str()),
            Some("bbbb-0001")
        );
        // The same id from the scan is fine — it just refreshes the record.
        let mut refresh = session(a, "bbbb-0001", IdSource::Disk);
        refresh.updated_at += 60;
        assert!(store.put(refresh));
    }

    #[test]
    fn put_rejects_an_unsafe_session_id() {
        let mut store = AgentSessionStore::default();
        assert!(!store.put(session(Uuid::from_u128(1), "rm -rf /", IdSource::Hook)));
        assert!(store.sessions.is_empty());
    }

    #[test]
    fn deactivate_keeps_the_record() {
        let a = Uuid::from_u128(1);
        let mut store = AgentSessionStore::default();
        store.put(session(a, "abc-123", IdSource::Disk));
        assert!(store.deactivate(a));
        assert!(!store.deactivate(a), "already inactive: no change");
        let rec = store.get(a).expect("record must survive deactivation");
        assert!(!rec.active);
        assert_eq!(rec.session_id, "abc-123");
    }

    #[test]
    fn store_json_round_trip() {
        let mut store = AgentSessionStore::default();
        store.put(session(Uuid::from_u128(7), "abc-123", IdSource::Hook));
        let dir = std::env::temp_dir().join(format!(
            "ymux-agent-sessions-test-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let path = dir.join("agent-sessions.json");
        save_to(&path, &store).expect("save");
        let loaded = load_from(&path);
        assert_eq!(loaded, store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_corrupt_store_loads_empty() {
        let dir = std::env::temp_dir().join(format!(
            "ymux-agent-sessions-bad-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let missing = dir.join("nope.json");
        assert_eq!(load_from(&missing), AgentSessionStore::default());
        let corrupt = dir.join("corrupt.json");
        std::fs::write(&corrupt, b"{not json at all").expect("write");
        assert_eq!(load_from(&corrupt), AgentSessionStore::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_kind_round_trips_the_registry_kind_strings() {
        assert_eq!(AgentKind::from_kind("claude"), Some(AgentKind::Claude));
        assert_eq!(AgentKind::from_kind("Codex"), Some(AgentKind::Codex));
        assert_eq!(AgentKind::from_kind("gemini"), None);
        assert_eq!(AgentKind::Claude.as_str(), "claude");
        assert_eq!(AgentKind::Codex.as_str(), "codex");
    }

    fn obs(pane: Uuid, kind: &str, cwd: Option<&str>) -> PaneObservation {
        PaneObservation {
            pane_id: pane,
            kind: kind.to_string(),
            cwd: cwd.map(str::to_string),
            status: Some(AgentStatus::Working),
            hook_session_id: None,
            process: Some(process(100, 0, &["claude"])),
            pid_file_session_id: None,
        }
    }

    fn process(pid: u32, start_secs: u64, argv: &[&str]) -> PaneProcess {
        PaneProcess {
            pid,
            start_secs,
            argv: argv.iter().map(|s| s.to_string()).collect(),
            subtree: [pid].into_iter().collect(),
        }
    }

    /// A transcript that began at `created` (epoch seconds).
    fn disk_at(id: &str, cwd: &str, created: u64) -> crate::agent_scan_disk::DiskSession {
        crate::agent_scan_disk::DiskSession {
            session_id: id.to_string(),
            cwd: cwd.to_string(),
            modified: SystemTime::UNIX_EPOCH,
            created: Some(created),
        }
    }

    /// A transcript begun after the default test process (start 0) started.
    fn disk(id: &str, cwd: &str) -> crate::agent_scan_disk::DiskSession {
        disk_at(id, cwd, 1)
    }

    fn other(pid: u32, start_secs: u64, cwd: &str) -> OtherAgent {
        OtherAgent {
            kind: AgentKind::Claude,
            pid,
            start_secs,
            cwd: Some(cwd.to_string()),
            known_id: None,
        }
    }

    const ID_A: &str = "aaaaaaaa-0000-0000-0000-00000000000a";
    const ID_B: &str = "bbbbbbbb-0000-0000-0000-00000000000b";

    #[test]
    fn a_resumed_claude_session_skips_permission_prompts() {
        // `claude --help` on this machine: `--dangerously-skip-permissions
        // Bypass all permission checks.` The resumed conversation comes back
        // in the mode the user left it in rather than re-asking for approvals
        // they already gave.
        let argv = resume_argv(AgentKind::Claude, ID_A).expect("valid id");
        assert_eq!(
            argv,
            vec![
                "claude".to_string(),
                "--resume".to_string(),
                ID_A.to_string(),
                "--dangerously-skip-permissions".to_string(),
            ]
        );
        assert_eq!(
            argv.iter()
                .filter(|a| *a == CLAUDE_SKIP_PERMISSIONS)
                .count(),
            1,
            "exactly once"
        );
        // Codex documents `--dangerously-bypass-approvals-and-sandbox`, but
        // that also disables the sandbox — a different promise. Left out.
        let codex = resume_argv(AgentKind::Codex, ID_A).expect("valid id");
        assert!(!codex.iter().any(|a| a.starts_with("--dangerously")));
    }

    #[test]
    fn the_permission_flag_is_not_duplicated_from_startup_cmd() {
        // The user's own startup command already carries it.
        let cmd = resume_command(
            AgentKind::Claude,
            ID_A,
            "claude -c --dangerously-skip-permissions --model opus",
            ShellFamily::Posix,
        )
        .expect("valid id");
        assert_eq!(
            cmd.matches(CLAUDE_SKIP_PERMISSIONS).count(),
            1,
            "{cmd} must carry the flag exactly once"
        );
        assert_eq!(
            cmd,
            format!("claude --resume {ID_A} --dangerously-skip-permissions --model opus")
        );
    }

    #[test]
    fn tracker_records_a_disk_scanned_session() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            1_000,
            &[],
            |_, cwd| {
                assert_eq!(cwd, "D:/Git/ymux");
                vec![disk(ID_A, "D:\\Git\\ymux")]
            },
        );
        let rec = t.get(pane).expect("recorded");
        assert_eq!(rec.session_id, ID_A);
        // The transcript's own spelling is kept, not the pane's (rule 15: the
        // key is for comparison, the raw string is what names the directory).
        assert_eq!(rec.cwd, "D:\\Git\\ymux");
        assert_eq!(rec.source, IdSource::Disk);
        assert!(rec.interrupted, "status was `working`, not `done`");
        assert!(t.take_dirty());
        assert!(!t.take_dirty(), "the flag clears");
    }

    #[test]
    fn tracker_skips_an_agent_with_no_resume_story() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        t.observe(
            &obs(pane, "gemini", Some("D:/Git/ymux")),
            1_000,
            &[],
            |_, _| panic!("must not even look on disk for an agent we cannot resume"),
        );
        assert!(t.get(pane).is_none());
    }

    #[test]
    fn tracker_throttles_the_disk_scan() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let mut calls = 0;
        // Nothing found: the pane stays unknown, but the scan is still
        // throttled or every 2 s tick would re-read the transcript tree.
        for tick in 0..5u64 {
            let now = 1_000 + tick * 2;
            if t.wants_disk_scan(pane, now) {
                calls += 1;
            }
            t.observe(
                &obs(pane, "claude", Some("D:/Git/ymux")),
                now,
                &[],
                |_, _| vec![],
            );
        }
        assert_eq!(calls, 1, "only the first tick may look");
        assert!(
            t.wants_disk_scan(pane, 1_000 + DISK_RESCAN_INTERVAL),
            "and again after the interval"
        );
    }

    #[test]
    fn tracker_never_rescans_a_hook_sourced_pane() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let mut o = obs(pane, "claude", Some("D:/Git/ymux"));
        o.hook_session_id = Some(ID_A.to_string());
        o.pid_file_session_id = Some(ID_A.to_string()); // vouches for it
        t.observe(&o, 1_000, &[], |_, _| {
            panic!("a hook id needs no disk scan")
        });
        assert_eq!(t.get(pane).map(|s| s.source), Some(IdSource::Hook));
        assert!(!t.wants_disk_scan(pane, 1_000 + DISK_RESCAN_INTERVAL * 10));
    }

    #[test]
    fn tracker_refreshes_updated_at_while_the_agent_lives() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            1_000,
            &[],
            |_, _| vec![disk(ID_A, "D:\\Git\\ymux")],
        );
        // Much later, still the same agent and no new disk scan result.
        let later = 1_000 + 5 * PERSIST_GRANULARITY;
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            later,
            &[],
            |_, _| vec![],
        );
        let rec = t.get(pane).expect("still recorded");
        assert_eq!(
            rec.updated_at,
            later - (later % PERSIST_GRANULARITY),
            "the 24h window measures liveness, quantized to the persist grid"
        );
        assert_eq!(rec.session_id, ID_A);
    }

    #[test]
    fn an_unchanged_record_does_not_dirty_the_store_every_tick() {
        // The scan ticks every 2 s. Without the quantized timestamp each tick
        // would count as a change and rewrite the store file forever.
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let start = 10 * PERSIST_GRANULARITY;
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            start,
            &[],
            |_, _| vec![disk(ID_A, "D:\\Git\\ymux")],
        );
        assert!(t.take_dirty(), "the first record is a real change");
        for tick in 1..(PERSIST_GRANULARITY / 2) {
            t.observe(
                &obs(pane, "claude", Some("D:/Git/ymux")),
                start + tick * 2,
                &[],
                |_, _| vec![],
            );
        }
        assert!(
            !t.take_dirty(),
            "ticks inside one grid step must not rewrite the file"
        );
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            start + PERSIST_GRANULARITY,
            &[],
            |_, _| vec![],
        );
        assert!(t.take_dirty(), "and the next grid step does");
    }

    #[test]
    fn note_agent_exit_declines_without_deleting() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            1_000,
            &[],
            |_, _| vec![disk(ID_A, "D:\\Git\\ymux")],
        );
        t.note_agent_exit(pane);
        let rec = t.get(pane).expect("record survives");
        assert!(!rec.active);
        assert_eq!(
            outcome_for(Some(rec), "", ShellFamily::Posix, 1_000, |_| true),
            ResumeOutcome::None
        );
        // And the pane can start a fresh conversation afterwards.
        assert!(t.wants_disk_scan(pane, 1_000));
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            1_001,
            &[],
            |_, _| vec![disk(ID_B, "D:\\Git\\ymux")],
        );
        assert_eq!(t.get(pane).map(|s| s.session_id.as_str()), Some(ID_B));
        assert!(t.get(pane).is_some_and(|s| s.active));
    }

    #[test]
    fn an_exited_session_is_not_revived_by_the_next_agent_in_the_pane() {
        // The user quit Claude (the scan saw it go), then started a new one in
        // the same pane. Seeing *an* agent of the same kind again says nothing
        // about which conversation it is running; the old record must stay
        // declined, or the next launch resumes a conversation the user ended.
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            1_000,
            &[],
            |_, _| vec![disk(ID_A, "D:\\Git\\ymux")],
        );
        t.note_agent_exit(pane);
        // The new agent has not written a transcript yet.
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            1_010,
            &[],
            |_, _| vec![],
        );
        let rec = t.get(pane).expect("record kept");
        assert!(!rec.active, "an ended conversation must not come back");
        assert_eq!(
            outcome_for(Some(rec), "", ShellFamily::Posix, 1_010, |_| true),
            ResumeOutcome::None
        );
    }

    #[test]
    fn a_hook_sourced_pane_gets_a_fresh_binding_after_its_agent_exits() {
        // A hook-borne id stops the disk scan while that agent lives. Once it
        // has exited, the next agent in the pane (tracking since turned off,
        // say) must be looked up afresh rather than never again.
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let mut o = obs(pane, "claude", Some("D:/Git/ymux"));
        o.hook_session_id = Some(ID_A.to_string());
        o.pid_file_session_id = Some(ID_A.to_string()); // vouches for it
        t.observe(&o, 1_000, &[], |_, _| {
            panic!("a hook id needs no disk scan")
        });
        t.note_agent_exit(pane);
        assert!(t.wants_disk_scan(pane, 1_002));
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            1_002,
            &[],
            |_, _| vec![disk(ID_B, "D:\\Git\\ymux")],
        );
        let rec = t.get(pane).expect("recorded");
        assert_eq!(rec.session_id, ID_B);
        assert!(rec.active);
        assert_eq!(rec.source, IdSource::Disk);
    }

    const CWD: &str = "D:\\Work\\proj";
    const ID_C: &str = "cccccccc-0000-0000-0000-00000000000c";
    const ID_Y: &str = "99999999-0000-0000-0000-000000000099";

    /// One scan tick over `panes`: each pane sees the others' agent
    /// processes, plus any `outside` ones, and the same transcripts on disk.
    fn tick(
        t: &mut SessionTracker,
        now: u64,
        panes: &[(Uuid, PaneProcess)],
        on_disk: &[crate::agent_scan_disk::DiskSession],
        outside: &[OtherAgent],
    ) {
        for (pane, proc_) in panes {
            let mut others: Vec<OtherAgent> = panes
                .iter()
                .filter(|(p, _)| p != pane)
                .map(|(_, q)| other(q.pid, q.start_secs, CWD))
                .collect();
            others.extend(outside.iter().cloned());
            let mut o = obs(*pane, "claude", Some(CWD));
            o.process = Some(proc_.clone());
            t.observe(&o, now, &others, |_, _| on_disk.to_vec());
        }
    }

    fn id_of(t: &SessionTracker, pane: Uuid) -> Option<String> {
        t.get(pane)
            .filter(|s| s.active)
            .map(|s| s.session_id.clone())
    }

    #[test]
    fn two_panes_in_one_folder_never_take_each_others_session() {
        // Hooks off. A starts at 1000, B at 1100; yesterday's conversation is
        // still on disk. The user prompts in B first (1200), then A (1300).
        // Every step of the old failure is here: A's first scan runs before
        // its transcript exists (and must not take yesterday's), and B's is
        // the newest unclaimed transcript on every later rescan.
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let panes = [
            (a, process(10, 1_000, &["claude"])),
            (b, process(20, 1_100, &["claude"])),
        ];
        let yesterday = disk_at(ID_Y, CWD, 1_000 - 900);
        let tb = disk_at(ID_B, CWD, 1_200);
        let ta = disk_at(ID_A, CWD, 1_300);
        let mut t = SessionTracker::default();
        tick(&mut t, 1_002, &panes, std::slice::from_ref(&yesterday), &[]);
        assert_eq!(id_of(&t, a), None, "yesterday's session is not A's");
        let mut now = 1_250;
        tick(&mut t, now, &panes, &[yesterday.clone(), tb.clone()], &[]);
        for _ in 0..6 {
            now += DISK_RESCAN_INTERVAL;
            tick(
                &mut t,
                now,
                &panes,
                &[yesterday.clone(), tb.clone(), ta.clone()],
                &[],
            );
            assert_ne!(id_of(&t, a).as_deref(), Some(ID_B), "A must never hold B's");
            assert_ne!(id_of(&t, b).as_deref(), Some(ID_A), "B must never hold A's");
            assert_ne!(id_of(&t, a).as_deref(), Some(ID_Y));
        }
        // Both transcripts began after both processes did, so from timestamps
        // alone either could be either: no binding beats a wrong one.
        assert_eq!(id_of(&t, a), None);
        assert_eq!(id_of(&t, b), None);
    }

    #[test]
    fn two_panes_in_one_folder_each_get_their_own_when_it_is_decidable() {
        // A prompts (1050) before B even starts (1100): A's transcript can
        // only be A's, and once A is bound, B's is the only one left for B.
        // B is observed first each tick, so order does not decide it.
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let panes = [
            (b, process(20, 1_100, &["claude"])),
            (a, process(10, 1_000, &["claude"])),
        ];
        let on_disk = [disk_at(ID_B, CWD, 1_200), disk_at(ID_A, CWD, 1_050)];
        let mut t = SessionTracker::default();
        let mut now = 1_300;
        for _ in 0..3 {
            tick(&mut t, now, &panes, &on_disk, &[]);
            now += DISK_RESCAN_INTERVAL;
        }
        assert_eq!(id_of(&t, a).as_deref(), Some(ID_A));
        assert_eq!(id_of(&t, b).as_deref(), Some(ID_B));
    }

    #[test]
    fn a_claude_in_an_outside_terminal_is_never_adopted() {
        let pane = Uuid::from_u128(1);
        let panes = [(pane, process(10, 1_000, &["claude"]))];
        for outside_start in [900, 1_100] {
            let outside = other(77, outside_start, CWD);
            let mut t = SessionTracker::default();
            // Only the outside Claude has written anything.
            tick(
                &mut t,
                1_300,
                &panes,
                &[disk_at(ID_B, CWD, 1_200)],
                std::slice::from_ref(&outside),
            );
            assert_eq!(id_of(&t, pane), None, "outside started at {outside_start}");
            // Once the outside process is known to hold ID_B (its own pid
            // file), the pane's own later transcript is unambiguous.
            let mut known = outside;
            known.known_id = Some(ID_B.to_string());
            tick(
                &mut t,
                1_300 + DISK_RESCAN_INTERVAL,
                &panes,
                &[disk_at(ID_B, CWD, 1_200), disk_at(ID_A, CWD, 1_250)],
                &[known],
            );
            assert_eq!(id_of(&t, pane).as_deref(), Some(ID_A));
        }
    }

    #[test]
    fn last_launchs_record_is_declined_for_a_process_that_is_not_running_it() {
        // The pane's record says ID_Y (from the previous launch); what is
        // running now is a plain `claude` whose own transcript does not exist
        // yet. ID_Y began long before this process, so it is not its.
        let pane = Uuid::from_u128(1);
        let mut store = AgentSessionStore::default();
        let mut old = session(pane, ID_Y, IdSource::Disk);
        old.cwd = CWD.to_string();
        store.put(old);
        let mut t = SessionTracker::from_store(store);
        let panes = [(pane, process(10, 100_000, &["claude"]))];
        tick(&mut t, 100_010, &panes, &[disk_at(ID_Y, CWD, 50_000)], &[]);
        let rec = t.get(pane).expect("kept");
        assert!(!rec.active);
        assert_eq!(
            outcome_for(Some(rec), "", ShellFamily::Posix, 100_010, |_| true),
            ResumeOutcome::None
        );
    }

    #[test]
    fn a_pane_ymux_resumed_is_bound_by_the_processes_own_argv() {
        // The resumed conversation's transcript began days ago — older than
        // the process — so only argv can say this process is running it.
        let pane = Uuid::from_u128(1);
        let mut store = AgentSessionStore::default();
        let mut old = session(pane, ID_A, IdSource::Disk);
        old.cwd = "D:\\Work\\proj".to_string();
        store.put(old);
        let mut t = SessionTracker::from_store(store);
        let panes = [(
            pane,
            process(
                10,
                100_000,
                &["claude", "--resume", ID_A, "--dangerously-skip-permissions"],
            ),
        )];
        tick(&mut t, 100_010, &panes, &[disk_at(ID_A, CWD, 10)], &[]);
        let rec = t.get(pane).expect("kept");
        assert!(rec.active);
        assert_eq!(rec.session_id, ID_A);
        assert_eq!(rec.source, IdSource::Process);
        assert_eq!(rec.cwd, "D:\\Work\\proj", "the recorded spelling is kept");
    }

    #[test]
    fn claudes_pid_file_is_exact_and_may_move_the_pane() {
        // `/clear` or `/resume` inside Claude switches conversation; the pid
        // file follows it, and so does the pane.
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let panes = [(pane, process(10, 1_000, &["claude"]))];
        tick(&mut t, 1_100, &panes, &[disk_at(ID_A, CWD, 1_050)], &[]);
        assert_eq!(id_of(&t, pane).as_deref(), Some(ID_A));
        let mut o = obs(pane, "claude", Some(CWD));
        o.process = Some(panes[0].1.clone());
        o.pid_file_session_id = Some(ID_Y.to_string());
        t.observe(&o, 1_200, &[], |_, _| panic!("exact ids need no disk"));
        assert_eq!(id_of(&t, pane).as_deref(), Some(ID_Y));
        assert_eq!(t.get(pane).map(|s| s.source), Some(IdSource::Process));
    }

    #[test]
    fn a_guess_is_never_switched_while_the_same_process_lives() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let panes = [(pane, process(10, 1_000, &["claude"]))];
        tick(&mut t, 1_100, &panes, &[disk_at(ID_A, CWD, 1_050)], &[]);
        assert_eq!(id_of(&t, pane).as_deref(), Some(ID_A));
        assert!(!t.wants_disk_scan(pane, 1_100 + 10 * DISK_RESCAN_INTERVAL));
        for i in 1..5 {
            tick(
                &mut t,
                1_100 + i * DISK_RESCAN_INTERVAL,
                &panes,
                &[disk_at(ID_C, CWD, 1_060), disk_at(ID_A, CWD, 1_050)],
                &[],
            );
        }
        assert_eq!(id_of(&t, pane).as_deref(), Some(ID_A));
    }

    #[test]
    fn a_relaunch_between_two_ticks_ends_the_old_binding() {
        // The user quit Claude and started a new one within one scan
        // interval, so the scan never saw the pane without an agent.
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        tick(
            &mut t,
            1_100,
            &[(pane, process(10, 1_000, &["claude"]))],
            &[disk_at(ID_A, CWD, 1_050)],
            &[],
        );
        assert_eq!(id_of(&t, pane).as_deref(), Some(ID_A));
        tick(
            &mut t,
            1_502,
            &[(pane, process(11, 1_500, &["claude"]))],
            &[disk_at(ID_A, CWD, 1_050)],
            &[],
        );
        assert_eq!(id_of(&t, pane), None, "the new process has not said yet");
        tick(
            &mut t,
            1_502 + DISK_RESCAN_INTERVAL,
            &[(pane, process(11, 1_500, &["claude"]))],
            &[disk_at(ID_A, CWD, 1_050), disk_at(ID_C, CWD, 1_510)],
            &[],
        );
        assert_eq!(id_of(&t, pane).as_deref(), Some(ID_C));
    }

    #[test]
    fn a_record_with_no_cwd_is_not_resumable() {
        // A hook can land before OSC 7 has reported a cwd. Resuming from an
        // arbitrary directory is worse than not resuming: Claude sessions are
        // project-scoped, so the user gets an error instead of their
        // conversation.
        let mut s = session(Uuid::from_u128(1), ID_A, IdSource::Hook);
        s.cwd = String::new();
        assert_eq!(
            outcome_for(Some(&s), "", ShellFamily::Posix, s.updated_at, |_| true),
            ResumeOutcome::None
        );
    }

    #[test]
    fn a_pane_with_a_live_agent_stops_persisting_scrollback() {
        // Spec §5, and the half that is easy to miss: the condition is "has a
        // fresh record", not "was resumed at spawn". The very first Claude
        // session in a pane is not resumed and must still stop saving.
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let now = 10 * PERSIST_GRANULARITY;
        assert!(
            !t.suppresses_scrollback(pane, now),
            "a plain shell pane saves as it always did"
        );
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            now,
            &[],
            |_, _| vec![disk(ID_A, r"D:\Git\ymux")],
        );
        assert!(t.suppresses_scrollback(pane, now));
        // Quitting the agent hands the pane back to the shell, and the shell
        // gets its scrollback persistence back with it.
        t.note_agent_exit(pane);
        assert!(!t.suppresses_scrollback(pane, now));
    }

    #[test]
    fn a_stale_record_does_not_suppress_scrollback() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let now = 10 * PERSIST_GRANULARITY;
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            now,
            &[],
            |_, _| vec![disk(ID_A, r"D:\Git\ymux")],
        );
        assert!(!t.suppresses_scrollback(pane, now + FRESH_WINDOW.as_secs() + 1));
    }

    #[test]
    fn forget_removes_the_record_entirely() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        t.observe(
            &obs(pane, "claude", Some("D:/Git/ymux")),
            1_000,
            &[],
            |_, _| vec![disk(ID_A, "D:\\Git\\ymux")],
        );
        t.forget(pane);
        assert!(t.get(pane).is_none());
    }

    #[test]
    fn outcome_is_resume_only_when_fresh_active_and_present() {
        let mut s = session(Uuid::from_u128(1), ID_A, IdSource::Disk);
        s.agent = AgentKind::Claude;
        let now = s.updated_at + 3 * 3600;
        let ResumeOutcome::Resume { plan } =
            outcome_for(Some(&s), "", ShellFamily::Posix, now, |_| true)
        else {
            panic!("a fresh record with a live transcript must resume");
        };
        assert_eq!(plan.agent, "claude");
        assert_eq!(plan.age_secs, 3 * 3600);
        assert_eq!(plan.cwd, "D:\\Git\\ymux");
        assert_eq!(
            plan.command,
            format!("claude --resume {ID_A} --dangerously-skip-permissions")
        );

        // Stale: no resume, and deliberately no banner either — a pane the
        // user has not touched in over a day coming up as a plain shell is
        // not news worth a line of screen.
        assert_eq!(
            outcome_for(
                Some(&s),
                "",
                ShellFamily::Posix,
                s.updated_at + FRESH_WINDOW.as_secs() + 1,
                |_| true
            ),
            ResumeOutcome::None
        );
        // Transcript pruned by the agent itself. This one the user does hear
        // about (spec §4.3): they were expecting a conversation back.
        assert_eq!(
            outcome_for(Some(&s), "", ShellFamily::Posix, now, |_| false),
            ResumeOutcome::Missing {
                agent: "claude".to_string()
            }
        );
        // No record at all.
        assert_eq!(
            outcome_for(None, "", ShellFamily::Posix, now, |_| true),
            ResumeOutcome::None
        );
    }

    #[test]
    fn resume_command_merges_the_panes_own_startup_cmd() {
        let mut s = session(Uuid::from_u128(1), ID_A, IdSource::Disk);
        s.agent = AgentKind::Codex;
        let ResumeOutcome::Resume { plan } = outcome_for(
            Some(&s),
            "codex resume --last --model gpt-5",
            ShellFamily::Posix,
            s.updated_at,
            |_| true,
        ) else {
            panic!("a fresh record with a live transcript must resume");
        };
        assert_eq!(plan.command, format!("codex resume {ID_A} --model gpt-5"));
    }

    #[test]
    fn process_presence_never_overwrites_a_hook_status() {
        // The scan only knows the agent is there. Reporting `working` every
        // 2 s flipped a hook's `done` back and forth — and rewrote the store
        // on every hook event.
        let pane = Uuid::from_u128(1);
        let now = 10 * PERSIST_GRANULARITY;
        let mut t = SessionTracker::default();
        let mut hook = obs(pane, "claude", Some(CWD));
        hook.hook_session_id = Some(ID_A.to_string());
        hook.pid_file_session_id = Some(ID_A.to_string()); // vouches for it
        hook.status = Some(AgentStatus::Done);
        t.observe(&hook, now, &[], |_, _| vec![]);
        assert!(t.take_dirty());
        let mut scan = obs(pane, "claude", Some(CWD));
        scan.hook_session_id = Some(ID_A.to_string());
        scan.status = None;
        for i in 1..10 {
            t.observe(&scan, now + i * 2, &[], |_, _| vec![]);
        }
        let rec = t.get(pane).expect("recorded");
        assert_eq!(rec.state, AgentStatus::Done);
        assert!(
            !rec.interrupted,
            "a finished turn is not an interrupted one"
        );
        assert!(!t.take_dirty(), "presence alone changes nothing");
        // With no status ever reported, the conservative answer stands.
        let other_pane = Uuid::from_u128(2);
        let mut fresh = obs(other_pane, "claude", Some(CWD));
        fresh.hook_session_id = Some(ID_B.to_string());
        fresh.pid_file_session_id = Some(ID_B.to_string());
        fresh.status = None;
        t.observe(&fresh, now, &[], |_, _| vec![]);
        assert_eq!(
            t.get(other_pane).map(|s| s.state),
            Some(AgentStatus::Working)
        );
        assert!(t.get(other_pane).is_some_and(|s| s.interrupted));
    }

    #[test]
    fn records_for_panes_the_layout_no_longer_has_are_dropped() {
        let (kept, gone) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let mut store = AgentSessionStore::default();
        store.put(session(kept, ID_A, IdSource::Disk));
        store.put(session(gone, ID_B, IdSource::Disk));
        let mut t = SessionTracker::from_store(store);
        t.retain_panes(&[kept].into_iter().collect());
        assert!(t.get(kept).is_some());
        assert!(t.get(gone).is_none());
        assert!(t.take_dirty(), "the pruned store is written back");
        t.retain_panes(&[kept].into_iter().collect());
        assert!(!t.take_dirty(), "nothing to prune: no write");
    }

    #[test]
    fn existence_answers_are_reused_for_a_while_then_rechecked() {
        let mut c = ExistsCache::default();
        let t0 = std::time::Instant::now();
        let key = (AgentKind::Claude, ID_A.to_string());
        assert_eq!(c.get(&key, t0), None);
        c.put(key.clone(), true, t0);
        assert_eq!(c.get(&key, t0 + Duration::from_secs(5)), Some(true));
        assert_eq!(c.get(&key, t0 + EXISTS_CACHE_TTL), None, "expired");
        assert_eq!(
            c.get(&(AgentKind::Codex, ID_A.to_string()), t0),
            None,
            "keyed by agent too"
        );
    }

    /// A tracker holding last launch's live record for `pane` (id `ID_A`).
    fn tracker_with_last_launch(pane: Uuid, now: u64) -> SessionTracker {
        let mut store = AgentSessionStore::default();
        let mut old = session(pane, ID_A, IdSource::Disk);
        old.cwd = CWD.to_string();
        old.updated_at = now - 3600;
        store.put(old);
        SessionTracker::from_store(store)
    }

    #[test]
    fn a_resume_keeps_the_old_scrollback_until_the_agent_is_seen_running() {
        let pane = Uuid::from_u128(1);
        let now = 100_000;
        let mut t = tracker_with_last_launch(pane, now);
        assert_eq!(t.scrollback_action(pane, now), ScrollbackAction::Delete);
        t.begin_resume(pane, ID_A, now);
        // In flight: nothing is written, and nothing is deleted.
        assert_eq!(t.scrollback_action(pane, now + 5), ScrollbackAction::Skip);
        assert!(t.take_confirmed().is_empty());
        // The scan sees the resumed agent, by its own argv — but a `claude
        // --resume` that errors out lives a second or two, so one early
        // sighting does not confirm yet.
        let resumed = [(
            pane,
            process(
                10,
                now + 3,
                &["claude", "--resume", ID_A, CLAUDE_SKIP_PERMISSIONS],
            ),
        )];
        tick(&mut t, now + 6, &resumed, &[], &[]);
        assert!(t.take_confirmed().is_empty(), "too early to tell");
        assert_eq!(t.scrollback_action(pane, now + 6), ScrollbackAction::Skip);
        tick(
            &mut t,
            now + 3 + RESUME_CONFIRM_MIN_UPTIME,
            &resumed,
            &[],
            &[],
        );
        assert_eq!(t.take_confirmed(), vec![pane], "now the old blob can go");
        assert!(t.take_confirmed().is_empty(), "once");
        assert_eq!(
            t.scrollback_action(pane, now + 20),
            ScrollbackAction::Delete
        );
        t.expire_pending(now + 10 * RESUME_CONFIRM_WINDOW);
        assert!(
            t.get(pane).is_some_and(|s| s.active),
            "a confirmed resume never expires"
        );
    }

    #[test]
    fn a_resume_that_never_produced_an_agent_is_not_retried() {
        // The agent exited (or never started) before any scan saw it, so
        // `exited_panes` cannot report it; the deadline does.
        let pane = Uuid::from_u128(1);
        let now = 100_000;
        let mut t = tracker_with_last_launch(pane, now);
        t.begin_resume(pane, ID_A, now);
        t.expire_pending(now + RESUME_CONFIRM_WINDOW);
        assert_eq!(
            t.scrollback_action(pane, now + RESUME_CONFIRM_WINDOW),
            ScrollbackAction::Skip,
            "not yet"
        );
        t.expire_pending(now + RESUME_CONFIRM_WINDOW + 1);
        assert!(t.take_confirmed().is_empty());
        let rec = t.get(pane).expect("kept");
        assert!(
            !rec.active,
            "declined, so the next launch does not retry it"
        );
        assert_eq!(
            outcome_for(Some(rec), "", ShellFamily::Posix, now + 200, |_| true),
            ResumeOutcome::None
        );
        // Its old blob was never deleted, and nothing may overwrite it for
        // the rest of this run either: a save now would replace the history
        // the next launch is meant to restore with just this run's screen.
        assert_eq!(t.scrollback_action(pane, now + 200), ScrollbackAction::Skip);
        assert!(t.take_dirty());
        // A new agent in the pane that does get bound is a live agent pane.
        tick(
            &mut t,
            now + 300,
            &[(pane, process(11, now + 250, &["claude", "--resume", ID_B]))],
            &[],
            &[],
        );
        assert_eq!(
            t.scrollback_action(pane, now + 300),
            ScrollbackAction::Delete
        );
    }

    #[test]
    fn a_session_whose_transcript_is_gone_is_declined_at_once() {
        // Otherwise the record stays fresh, `save_scrollback` keeps deleting
        // the pane's blob for a day, and the banner repeats every launch.
        let pane = Uuid::from_u128(1);
        let now = 100_000;
        let mut t = tracker_with_last_launch(pane, now);
        let missing = ResumeOutcome::Missing {
            agent: "claude".into(),
        };
        t.note_outcome(pane, &missing, now);
        assert!(t.get(pane).is_some_and(|s| !s.active));
        assert_eq!(t.scrollback_action(pane, now), ScrollbackAction::Save);
        // And a resume outcome starts the pending window.
        let mut t = tracker_with_last_launch(pane, now);
        let rec = t.get(pane).cloned();
        let resume = outcome_for(rec.as_ref(), "", ShellFamily::Posix, now, |_| true);
        t.note_outcome(pane, &resume, now);
        assert_eq!(t.scrollback_action(pane, now), ScrollbackAction::Skip);
    }

    #[test]
    fn a_guessed_transcript_never_confirms_a_resume() {
        // Only the process naming the id counts; a pane whose process is a
        // plain `claude` has not resumed anything, whatever is on disk.
        let pane = Uuid::from_u128(1);
        let now = 100_000;
        let mut t = tracker_with_last_launch(pane, now);
        t.begin_resume(pane, ID_A, now);
        tick(
            &mut t,
            now + 6,
            &[(pane, process(10, now + 3, &["claude"]))],
            &[disk_at(ID_A, CWD, now + 4)],
            &[],
        );
        assert!(t.take_confirmed().is_empty());
    }

    #[test]
    fn a_hook_or_the_pid_file_confirms_a_resume_too() {
        let pane = Uuid::from_u128(1);
        let now = 100_000;
        for via_hook in [true, false] {
            let mut t = tracker_with_last_launch(pane, now);
            t.begin_resume(pane, ID_A, now);
            let mut o = obs(pane, "claude", Some(CWD));
            o.process = Some(process(10, now + 3, &["claude"]));
            if via_hook {
                // Vouched for by the pane's transcript (no pid file yet).
                o.hook_session_id = Some(ID_A.to_string());
            } else {
                o.pid_file_session_id = Some(ID_A.to_string());
            }
            t.observe(&o, now + 3 + RESUME_CONFIRM_MIN_UPTIME, &[], |_, _| {
                vec![disk_at(ID_A, CWD, now - 50_000)]
            });
            assert_eq!(t.take_confirmed(), vec![pane], "via_hook={via_hook}");
        }
    }

    // ---- A hook-borne session id must be corroborated by the pane's own
    // agent process before it can steer a resume (security review M3). Any
    // process holding the token can POST any id; the resume that follows
    // runs with --dangerously-skip-permissions.

    const HOOK_CWD: &str = "D:\\Work\\proj";

    fn hook_obs(pane: Uuid, id: &str) -> PaneObservation {
        let mut o = obs(pane, "claude", Some(HOOK_CWD));
        o.hook_session_id = Some(id.to_string());
        o
    }

    #[test]
    fn a_forged_hook_id_is_not_recorded() {
        // The id names a transcript somewhere else entirely; the pane's
        // process has no pid file entry or argv naming it.
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        t.observe(&hook_obs(pane, ID_B), 1_000, &[], |_, _| {
            vec![disk(ID_A, HOOK_CWD)]
        });
        assert_ne!(t.get(pane).map(|s| s.session_id.as_str()), Some(ID_B));
    }

    #[test]
    fn a_hook_id_from_another_cwd_is_not_recorded() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        t.observe(&hook_obs(pane, ID_B), 1_000, &[], |_, _| {
            vec![disk(ID_B, "D:\\Elsewhere")]
        });
        // (The stub's other-cwd candidate may still feed the *guess*, which
        // the real lookup never would; what matters is the hook didn't win.)
        assert_ne!(t.get(pane).map(|s| s.source), Some(IdSource::Hook));
    }

    #[test]
    fn a_hook_id_without_a_process_to_vouch_for_it_is_not_recorded() {
        // The hook listener's own observation: no process, no binding yet.
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let mut o = hook_obs(pane, ID_A);
        o.process = None;
        t.observe(&o, 1_000, &[], |_, _| vec![disk(ID_A, HOOK_CWD)]);
        assert!(t.get(pane).is_none());
    }

    #[test]
    fn a_hook_id_is_accepted_when_the_process_vouches_for_it() {
        type Vouch = fn(&mut PaneObservation);
        let cases: [(&str, Vouch); 3] = [
            ("pid file", |o| {
                o.pid_file_session_id = Some(ID_A.to_string())
            }),
            ("argv", |o| {
                o.process = Some(process(100, 0, &["claude", "--resume", ID_A]))
            }),
            ("transcript cwd", |_| {}),
        ];
        for (name, vouch) in cases {
            let pane = Uuid::from_u128(1);
            let mut t = SessionTracker::default();
            let mut o = hook_obs(pane, ID_A);
            vouch(&mut o);
            t.observe(&o, 1_000, &[], |_, _| vec![disk(ID_A, HOOK_CWD)]);
            let rec = t.get(pane).unwrap_or_else(|| panic!("{name}: recorded"));
            assert_eq!(rec.session_id, ID_A, "{name}");
            assert_eq!(rec.source, IdSource::Hook, "{name}");
        }
    }

    #[test]
    fn once_vouched_for_the_hook_id_keeps_flowing_without_a_process() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let mut scan = hook_obs(pane, ID_A);
        scan.pid_file_session_id = Some(ID_A.to_string());
        t.observe(&scan, 1_000, &[], |_, _| vec![]);
        let mut hook = hook_obs(pane, ID_A);
        hook.process = None;
        hook.status = Some(AgentStatus::Done);
        t.observe(&hook, 1_002, &[], |_, _| panic!("already vouched for"));
        assert_eq!(t.get(pane).map(|s| s.state), Some(AgentStatus::Done));
        // A different id from the hook alone is not taken over it.
        let mut switch = hook_obs(pane, ID_B);
        switch.process = None;
        t.observe(&switch, 1_004, &[], |_, _| vec![disk(ID_B, HOOK_CWD)]);
        assert_eq!(t.get(pane).map(|s| s.session_id.as_str()), Some(ID_A));
    }

    #[test]
    fn an_unvouched_hook_id_reads_the_disk_at_most_once_per_interval() {
        let pane = Uuid::from_u128(1);
        let mut t = SessionTracker::default();
        let mut calls = 0;
        for i in 0..5 {
            t.observe(&hook_obs(pane, ID_B), 1_000 + i, &[], |_, _| {
                calls += 1;
                vec![]
            });
        }
        assert_eq!(calls, 1);
    }
}
