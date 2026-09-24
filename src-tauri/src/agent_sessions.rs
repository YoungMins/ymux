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

/// Strip any conversation selector already present in a saved `startup_cmd`,
/// returning the command with only its non-selector arguments left.
///
/// A pane whose startup command is `claude -c --model opus` plus our
/// `--resume <id>` is two selectors fighting (spec §3). Returns `None` when
/// `startup_cmd` does not start the agent at all — then the caller uses the
/// bare [`resume_argv`] and leaves the unrelated command alone.
///
/// Token-aware rather than a regex over the raw string, so `claude --resume
/// "my session"` loses both tokens and `echo --resume` is left untouched.
///
/// Every flag in the drop lists was read off `--help` on this machine, not
/// guessed: Claude has `-c/--continue`, `-r/--resume`, `--fork-session`,
/// `--teleport` and `--from-pr`; Codex has the `resume` and `fork`
/// subcommands and `--last`. (`--fork` is *not* a Claude flag — the real
/// spelling is `--fork-session`, and it means "when resuming, create a new
/// session ID", which is exactly the opposite of continuing.)
///
/// It also drops any flag [`resume_argv`] supplies itself, so a user whose
/// startup command already carries `--dangerously-skip-permissions` gets it
/// once, not twice.
pub fn strip_selector(startup_cmd: &str, agent: AgentKind) -> Option<Vec<String>> {
    let tokens = shell_split(startup_cmd);
    let first = tokens.first()?;
    if !program_is(first, agent.as_str()) {
        return None;
    }
    // Codex's selector is a subcommand plus an optional positional id, so the
    // whole `resume …` tail goes; Claude's is a flag.
    let (mut out, rest) = match agent {
        AgentKind::Codex => {
            let mut out = vec![tokens[0].clone()];
            let mut rest = &tokens[1..];
            // `fork` is the same shape as `resume` (`codex fork [SESSION_ID]`)
            // and forks rather than continues, so it goes the same way.
            if matches!(
                rest.first().map(String::as_str),
                Some("resume") | Some("fork")
            ) {
                rest = &rest[1..];
                // `resume` may be followed by a bare positional session id.
                if rest
                    .first()
                    .is_some_and(|t| !t.starts_with('-') && is_valid_session_id(t))
                {
                    rest = &rest[1..];
                }
            }
            (std::mem::take(&mut out), rest.to_vec())
        }
        AgentKind::Claude => (vec![tokens[0].clone()], tokens[1..].to_vec()),
    };

    let mut i = 0;
    while i < rest.len() {
        let t = rest[i].as_str();
        let drop_with_value = matches!(t, "--resume" | "-r" | "--teleport" | "--from-pr");
        let drop_alone = matches!(
            t,
            "-c" | "--continue" | "--last" | "--fork-session" | CLAUDE_SKIP_PERMISSIONS
        );
        if drop_with_value {
            i += 1;
            // Its value is optional for every one of these flags, so only eat
            // a following token when it is not itself a flag.
            if rest.get(i).is_some_and(|v| !v.starts_with('-')) {
                i += 1;
            }
            continue;
        }
        if drop_alone {
            i += 1;
            continue;
        }
        // `--resume=<id>` / `-r=<id>` spellings.
        if t.starts_with("--resume=") || t.starts_with("-r=") {
            i += 1;
            continue;
        }
        out.push(rest[i].clone());
        i += 1;
    }
    Some(out)
}

/// The full command to type into the pane's shell: the saved `startup_cmd`'s
/// surviving arguments (if it started this agent) plus our explicit selector.
///
/// Quoting stays the shell's problem because the string is *typed*, not
/// spawned (spec §3); every token we add is either a literal flag or an id
/// that passed [`is_valid_session_id`], so neither can carry a space.
pub fn resume_command(agent: AgentKind, session_id: &str, startup_cmd: &str) -> Option<String> {
    let argv = resume_argv(agent, session_id)?;
    let Some(kept) = strip_selector(startup_cmd, agent) else {
        return Some(argv.join(" "));
    };
    // `kept[0]` is the program as the user spelled it (possibly a full path);
    // keep that spelling and append our selector plus their other flags.
    let mut out: Vec<String> = vec![kept[0].clone()];
    out.extend(argv[1..].iter().cloned());
    out.extend(kept[1..].iter().cloned());
    Some(out.join(" "))
}

/// Whether `token` invokes the program `name`, allowing for a path prefix and
/// a Windows `.exe`/`.cmd`/`.bat` suffix, and for the token being quoted.
fn program_is(token: &str, name: &str) -> bool {
    let t = token.trim_matches(['"', '\'']);
    let stem = t
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(t)
        .to_ascii_lowercase();
    let stem = stem
        .strip_suffix(".exe")
        .or_else(|| stem.strip_suffix(".cmd"))
        .or_else(|| stem.strip_suffix(".bat"))
        .unwrap_or(&stem);
    stem == name
}

/// Minimal POSIX-ish tokenizer: splits on unquoted whitespace and keeps
/// quoted runs together (dropping the quotes). Enough to recognize the
/// selector flags in a saved startup command; it is never used to *build* a
/// command line, only to decide which tokens survive.
fn shell_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                has = true;
            }
            None if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            None => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
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
    pub status: AgentStatus,
    /// The session id a Claude Code hook relayed for this pane, when agent
    /// tracking is on. Exact, so it outranks anything found on disk.
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
    /// A record changed since the last `take_dirty`.
    dirty: bool,
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

    /// Fold one observation into the store.
    ///
    /// The record for a pane is always about the agent *process* running in
    /// it (see `agent_binding` for the sources and their order):
    ///
    /// * A new process in the pane ends the previous one's claim: its record
    ///   is declined until the new process is tied to a conversation — which
    ///   may well be the same one, when it was started with `--resume <id>`.
    /// * An exact id (hook, Claude's pid file, argv) is taken as given and may
    ///   move the pane to a new conversation (`/clear`, `/resume`).
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

        // Exact ids: the hook, Claude's pid file, the process's own argv.
        let exact = obs
            .hook_session_id
            .clone()
            .filter(|s| is_valid_session_id(s))
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
        let candidates = lookup(agent, cwd);
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
        let changed = self.store.put(AgentSession {
            pane_id: obs.pane_id,
            agent,
            session_id: session_id.to_string(),
            cwd,
            state: obs.status,
            interrupted: obs.status != AgentStatus::Done,
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

    /// The user closed the pane for good. Mirrors `delete_scrollback`.
    pub fn forget(&mut self, pane_id: Uuid) {
        self.dirty |= self.store.remove(pane_id);
        self.last_disk_scan.remove(&pane_id);
        self.bindings.remove(&pane_id);
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
    let Some(command) = resume_command(s.agent, &s.session_id, startup_cmd) else {
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
    crate::agent_scan_disk::transcript_exists(session.agent, &session.session_id)
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

    #[test]
    fn strip_selector_removes_continue_and_resume() {
        let c = AgentKind::Claude;
        assert_eq!(strip_selector("claude -c", c), Some(vec!["claude".into()]));
        assert_eq!(
            strip_selector("claude --continue", c),
            Some(vec!["claude".into()])
        );
        assert_eq!(
            strip_selector("claude --resume old-id-1234", c),
            Some(vec!["claude".into()])
        );
        assert_eq!(
            strip_selector("claude -r old-id-1234", c),
            Some(vec!["claude".into()])
        );
        assert_eq!(
            strip_selector("claude --resume=old-id-1234", c),
            Some(vec!["claude".into()])
        );
        // `claude --help`: "--fork-session  When resuming, create a new
        // session ID" — i.e. it forks instead of continuing, so it goes too.
        assert_eq!(
            strip_selector("claude --resume old-id-1234 --fork-session", c),
            Some(vec!["claude".into()])
        );
    }

    #[test]
    fn strip_selector_keeps_unrelated_flags() {
        assert_eq!(
            strip_selector("claude -c --model opus --verbose", AgentKind::Claude),
            Some(vec![
                "claude".into(),
                "--model".into(),
                "opus".into(),
                "--verbose".into()
            ])
        );
    }

    #[test]
    fn strip_selector_handles_codex_subcommand_and_last() {
        let k = AgentKind::Codex;
        assert_eq!(
            strip_selector("codex resume 01a07644-42b3-7183-a7fd-70379b88af1f", k),
            Some(vec!["codex".into()])
        );
        assert_eq!(
            strip_selector("codex resume --last", k),
            Some(vec!["codex".into()])
        );
        // `codex fork` has the same shape and forks instead of continuing.
        assert_eq!(
            strip_selector("codex fork 01a07644-42b3-7183-a7fd-70379b88af1f", k),
            Some(vec!["codex".into()])
        );
        assert_eq!(
            strip_selector("codex resume --last --model gpt-5", k),
            Some(vec!["codex".into(), "--model".into(), "gpt-5".into()])
        );
    }

    #[test]
    fn strip_selector_is_quote_aware() {
        // A quoted value must be eaten with its flag, not left behind as a
        // stray positional argument.
        assert_eq!(
            strip_selector("claude --resume \"old id\" --model opus", AgentKind::Claude),
            Some(vec!["claude".into(), "--model".into(), "opus".into()])
        );
    }

    #[test]
    fn strip_selector_leaves_nothing_to_strip_alone() {
        assert_eq!(
            strip_selector("claude", AgentKind::Claude),
            Some(vec!["claude".into()])
        );
        assert_eq!(
            strip_selector("claude --model opus", AgentKind::Claude),
            Some(vec!["claude".into(), "--model".into(), "opus".into()])
        );
    }

    #[test]
    fn strip_selector_ignores_a_different_program() {
        // `echo --resume x` is not a Claude invocation; leave it be and let
        // the caller fall back to the bare resume command.
        assert_eq!(strip_selector("echo --resume x", AgentKind::Claude), None);
        assert_eq!(strip_selector("", AgentKind::Claude), None);
        assert_eq!(strip_selector("codex", AgentKind::Claude), None);
    }

    #[test]
    fn strip_selector_matches_a_pathed_or_exe_program() {
        assert_eq!(
            strip_selector("C:\\bin\\claude.exe -c", AgentKind::Claude),
            Some(vec!["C:\\bin\\claude.exe".into()])
        );
        assert_eq!(
            strip_selector("/usr/local/bin/claude --continue", AgentKind::Claude),
            Some(vec!["/usr/local/bin/claude".into()])
        );
    }

    #[test]
    fn resume_command_merges_with_startup_cmd() {
        assert_eq!(
            resume_command(AgentKind::Claude, "abc-123", "claude -c --model opus"),
            Some("claude --resume abc-123 --dangerously-skip-permissions --model opus".to_string())
        );
        // Unrelated startup command: use the bare resume, don't mangle theirs.
        assert_eq!(
            resume_command(AgentKind::Claude, "abc-123", "npm run dev"),
            Some("claude --resume abc-123 --dangerously-skip-permissions".to_string())
        );
        assert_eq!(
            resume_command(AgentKind::Codex, "abc-123", "codex resume --last"),
            Some("codex resume abc-123".to_string())
        );
        // The user's own spelling of the program survives.
        assert_eq!(
            resume_command(AgentKind::Claude, "abc-123", "C:\\bin\\claude.exe -c"),
            Some("C:\\bin\\claude.exe --resume abc-123 --dangerously-skip-permissions".to_string())
        );
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
            status: AgentStatus::Working,
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
        // And `strip_selector` is where that happens, so it is visible there.
        assert_eq!(
            strip_selector("claude --dangerously-skip-permissions", AgentKind::Claude),
            Some(vec!["claude".to_string()])
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
            outcome_for(Some(rec), "", 1_000, |_| true),
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
            outcome_for(Some(rec), "", 1_010, |_| true),
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
            outcome_for(Some(rec), "", 100_010, |_| true),
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
            outcome_for(Some(&s), "", s.updated_at, |_| true),
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
        let ResumeOutcome::Resume { plan } = outcome_for(Some(&s), "", now, |_| true) else {
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
                s.updated_at + FRESH_WINDOW.as_secs() + 1,
                |_| true
            ),
            ResumeOutcome::None
        );
        // Transcript pruned by the agent itself. This one the user does hear
        // about (spec §4.3): they were expecting a conversation back.
        assert_eq!(
            outcome_for(Some(&s), "", now, |_| false),
            ResumeOutcome::Missing {
                agent: "claude".to_string()
            }
        );
        // No record at all.
        assert_eq!(outcome_for(None, "", now, |_| true), ResumeOutcome::None);
    }

    #[test]
    fn resume_command_merges_the_panes_own_startup_cmd() {
        let mut s = session(Uuid::from_u128(1), ID_A, IdSource::Disk);
        s.agent = AgentKind::Codex;
        let ResumeOutcome::Resume { plan } = outcome_for(
            Some(&s),
            "codex resume --last --model gpt-5",
            s.updated_at,
            |_| true,
        ) else {
            panic!("a fresh record with a live transcript must resume");
        };
        assert_eq!(plan.command, format!("codex resume {ID_A} --model gpt-5"));
    }
}
