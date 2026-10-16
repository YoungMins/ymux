//! Per-(pane, session) turn state machine.
//!
//! A turn opens on a prompt (Claude `UserPromptSubmit`, Codex `task_started`)
//! and closes on `Stop`/`StopFailure`/`task_complete`. A prompt that arrives
//! while a turn is still open closes the old one as interrupted. Tool records
//! seen in between are folded into the [`TurnFacts`] the close produces.
//!
//! Git baseline/delta is *not* computed here (a worker does that off the hot
//! path); [`TurnFacts::with_stats`] accepts the result afterwards. Pure: the
//! caller supplies the clock, so tests are deterministic.

use super::codex::CodexItem;
use super::extract::{ActivityRaw, RawKind, MAX_CMD_CHARS};
use super::redact::clip;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use uuid::Uuid;

/// Distinct files kept per turn.
pub const MAX_TURN_FILES: usize = 50;
/// Commands kept per turn (the test heuristic still sees every command).
pub const MAX_TURN_COMMANDS: usize = 20;
/// Open turns tracked at once; beyond this the oldest is dropped unreported.
const MAX_OPEN_TURNS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRecord {
    pub name: String,
    pub paths: Vec<String>,
    pub command: Option<String>,
    pub exit_code: Option<i64>,
    pub interrupted: bool,
    pub output_tail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEvent {
    Prompt {
        /// Codex supplies its own turn id; Claude does not.
        turn_id: Option<String>,
        prompt: Option<String>,
        started_ms: Option<u64>,
    },
    Tool(ToolRecord),
    Permission,
    Stop {
        message: Option<String>,
    },
    StopFailure {
        error: String,
        message: Option<String>,
    },
}

impl TurnEvent {
    /// `PreToolUse` carries no result, so only `PostToolUse` becomes a tool
    /// record. `None` for events that don't move the machine.
    pub fn from_raw(raw: &ActivityRaw) -> Option<Self> {
        Some(match raw.kind {
            RawKind::Prompt => TurnEvent::Prompt {
                turn_id: None,
                prompt: raw.prompt.clone(),
                started_ms: None,
            },
            RawKind::ToolStart => return None,
            RawKind::ToolEnd => TurnEvent::Tool(ToolRecord {
                name: raw.tool_name.clone().unwrap_or_default(),
                paths: raw.paths.clone(),
                command: raw.command.clone(),
                exit_code: raw.exit_code,
                interrupted: raw.interrupted,
                output_tail: raw.output_tail.clone(),
            }),
            RawKind::Permission => TurnEvent::Permission,
            RawKind::Stop => TurnEvent::Stop {
                message: raw.message.clone(),
            },
            RawKind::StopFailure => TurnEvent::StopFailure {
                error: raw.error.clone().unwrap_or_else(|| "unknown".into()),
                message: raw.message.clone(),
            },
        })
    }

    pub fn from_codex(item: &CodexItem) -> Self {
        match item {
            CodexItem::TaskStarted {
                turn_id,
                started_at,
            } => TurnEvent::Prompt {
                turn_id: (!turn_id.is_empty()).then(|| turn_id.clone()),
                prompt: None,
                started_ms: started_at
                    .and_then(|s| u64::try_from(s).ok())
                    .and_then(|s| s.checked_mul(1000)),
            },
            CodexItem::TaskComplete {
                last_agent_message, ..
            } => TurnEvent::Stop {
                message: last_agent_message.clone(),
            },
            CodexItem::Command {
                command,
                exit_code,
                output_tail,
                ..
            } => TurnEvent::Tool(ToolRecord {
                name: "shell".into(),
                paths: Vec::new(),
                command: Some(command.clone()),
                exit_code: *exit_code,
                interrupted: false,
                output_tail: output_tail.clone(),
            }),
            CodexItem::FileChange { paths, .. } => TurnEvent::Tool(ToolRecord {
                name: "apply_patch".into(),
                paths: paths.clone(),
                command: None,
                exit_code: None,
                interrupted: false,
                output_tail: None,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "error", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRecord {
    pub command: String,
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestRun {
    pub runner: String,
    pub command: String,
    /// From the exit code, else from output patterns, else unknown.
    pub passed: Option<bool>,
}

/// Injected by the worker after the turn closes (`git diff --stat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DiffStat {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnFacts {
    pub pane: Uuid,
    pub session: String,
    pub turn_id: String,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub outcome: TurnOutcome,
    pub prompt: Option<String>,
    pub tool_count: u32,
    pub tools: BTreeMap<String, u32>,
    pub files: Vec<String>,
    pub commands: Vec<CommandRecord>,
    pub test: Option<TestRun>,
    pub permission_requests: u32,
    pub last_message: Option<String>,
    pub stats: Option<DiffStat>,
}

impl TurnFacts {
    pub fn with_stats(mut self, stats: DiffStat) -> Self {
        self.stats = Some(stats);
        self
    }

    pub fn duration_ms(&self) -> u64 {
        self.ended_ms.saturating_sub(self.started_ms)
    }
}

#[derive(Debug)]
struct OpenTurn {
    turn_id: String,
    started_ms: u64,
    /// For the oldest-first eviction.
    seq: u64,
    prompt: Option<String>,
    tool_count: u32,
    tools: BTreeMap<String, u32>,
    files: Vec<String>,
    commands: Vec<CommandRecord>,
    test: Option<TestRun>,
    permission_requests: u32,
}

#[derive(Debug, Default)]
pub struct TurnTracker {
    open: HashMap<(Uuid, String), OpenTurn>,
    seq: u64,
}

impl TurnTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of turns currently open.
    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    /// Id of the turn open for `(pane, session)`, if any.
    pub fn open_turn_id(&self, pane: Uuid, session: &str) -> Option<&str> {
        self.open
            .get(&(pane, session.to_owned()))
            .map(|t| t.turn_id.as_str())
    }

    /// Whether a turn is open for `(pane, session)`.
    pub fn is_open(&self, pane: Uuid, session: &str) -> bool {
        self.open.contains_key(&(pane, session.to_owned()))
    }

    /// Drop everything a pane had open without reporting (pane killed).
    pub fn forget_pane(&mut self, pane: Uuid) {
        self.open.retain(|(p, _), _| *p != pane);
    }

    /// Feed one event; returns the turn it closed, if any (a new prompt over
    /// an open turn closes that one as interrupted).
    pub fn on_event(
        &mut self,
        pane: Uuid,
        session: &str,
        now_ms: u64,
        ev: TurnEvent,
    ) -> Option<TurnFacts> {
        let key = (pane, session.to_owned());
        match ev {
            TurnEvent::Prompt {
                turn_id,
                prompt,
                started_ms,
            } => {
                let closed = self
                    .open
                    .remove(&key)
                    .map(|t| finish(pane, session, t, now_ms, TurnOutcome::Interrupted, None));
                // A claimed start in the future is clamped to now.
                self.open_turn(
                    key,
                    turn_id,
                    prompt,
                    started_ms.unwrap_or(now_ms).min(now_ms),
                );
                closed
            }
            TurnEvent::Tool(rec) => {
                if !self.open.contains_key(&key) {
                    // Mid-turn start (ymux launched after the prompt, or the
                    // prompt hook was missed): open an anonymous turn.
                    self.open_turn(key.clone(), None, None, now_ms);
                }
                if let Some(t) = self.open.get_mut(&key) {
                    t.record(rec);
                }
                None
            }
            TurnEvent::Permission => {
                if let Some(t) = self.open.get_mut(&key) {
                    t.permission_requests = t.permission_requests.saturating_add(1);
                }
                None
            }
            // The end is when the worker saw it, never a duration the rollout
            // claims: a rollout is not a trusted clock.
            TurnEvent::Stop { message } => self
                .open
                .remove(&key)
                .map(|t| finish(pane, session, t, now_ms, TurnOutcome::Completed, message)),
            TurnEvent::StopFailure { error, message } => self.open.remove(&key).map(|t| {
                finish(
                    pane,
                    session,
                    t,
                    now_ms,
                    TurnOutcome::Failed(error),
                    message,
                )
            }),
        }
    }

    fn open_turn(
        &mut self,
        key: (Uuid, String),
        turn_id: Option<String>,
        prompt: Option<String>,
        started_ms: u64,
    ) {
        if self.open.len() >= MAX_OPEN_TURNS {
            if let Some(oldest) = self
                .open
                .iter()
                .min_by_key(|(_, t)| t.seq)
                .map(|(k, _)| k.clone())
            {
                self.open.remove(&oldest);
            }
        }
        self.seq += 1;
        let turn_id = turn_id.unwrap_or_else(|| format!("t{}", self.seq));
        self.open.insert(
            key,
            OpenTurn {
                turn_id,
                started_ms,
                seq: self.seq,
                prompt,
                tool_count: 0,
                tools: BTreeMap::new(),
                files: Vec::new(),
                commands: Vec::new(),
                test: None,
                permission_requests: 0,
            },
        );
    }
}

impl OpenTurn {
    fn record(&mut self, rec: ToolRecord) {
        self.tool_count = self.tool_count.saturating_add(1);
        if !rec.name.is_empty() {
            let n = self.tools.entry(rec.name.clone()).or_insert(0);
            *n = n.saturating_add(1);
        }
        for p in rec.paths {
            if self.files.len() < MAX_TURN_FILES && !self.files.contains(&p) {
                self.files.push(p);
            }
        }
        if let Some(cmd) = rec.command.as_deref().filter(|c| !c.trim().is_empty()) {
            if let Some(runner) = test_runner(cmd) {
                let passed = if rec.interrupted {
                    None
                } else {
                    test_result(rec.exit_code, rec.output_tail.as_deref())
                };
                self.test = Some(TestRun {
                    runner: runner.to_owned(),
                    command: clip(cmd, MAX_CMD_CHARS),
                    passed,
                });
            }
            if self.commands.len() < MAX_TURN_COMMANDS {
                self.commands.push(CommandRecord {
                    command: clip(cmd, MAX_CMD_CHARS),
                    exit_code: rec.exit_code,
                });
            }
        }
    }
}

fn finish(
    pane: Uuid,
    session: &str,
    t: OpenTurn,
    ended_ms: u64,
    outcome: TurnOutcome,
    last_message: Option<String>,
) -> TurnFacts {
    TurnFacts {
        pane,
        session: session.to_owned(),
        turn_id: t.turn_id,
        started_ms: t.started_ms,
        ended_ms: ended_ms.max(t.started_ms),
        outcome,
        prompt: t.prompt,
        tool_count: t.tool_count,
        tools: t.tools,
        files: t.files,
        commands: t.commands,
        test: t.test,
        permission_requests: t.permission_requests,
        last_message,
        stats: None,
    }
}

/// Runner phrases as consecutive tokens, matched anywhere in the command (so
/// `cd x && cargo test -p y` and `npx vitest run` both count).
const TEST_PHRASES: &[(&[&str], &str)] = &[
    (&["cargo", "test"], "cargo"),
    (&["pnpm", "test"], "pnpm"),
    (&["pnpm", "run", "test"], "pnpm"),
    (&["npm", "test"], "npm"),
    (&["npm", "run", "test"], "npm"),
    (&["yarn", "test"], "yarn"),
    (&["yarn", "run", "test"], "yarn"),
    (&["go", "test"], "go"),
    (&["dotnet", "test"], "dotnet"),
];
const TEST_BINARIES: &[&str] = &["vitest", "jest", "pytest"];

/// Which test runner a shell command invokes, if any.
pub fn test_runner(command: &str) -> Option<&'static str> {
    let tokens: Vec<String> = command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect();
    for (phrase, name) in TEST_PHRASES {
        if tokens
            .windows(phrase.len())
            .any(|w| w.iter().map(String::as_str).eq(phrase.iter().copied()))
        {
            return Some(name);
        }
    }
    for t in &tokens {
        let base = t.rsplit(['/', '\\']).next().unwrap_or("");
        let base = base
            .strip_suffix(".cmd")
            .or_else(|| base.strip_suffix(".exe"))
            .unwrap_or(base);
        if let Some(b) = TEST_BINARIES.iter().find(|b| **b == base) {
            return Some(b);
        }
    }
    // `python -m pytest`
    if tokens.windows(3).any(|w| w[1] == "-m" && w[2] == "pytest") {
        return Some("pytest");
    }
    None
}

fn patterns() -> &'static (Option<Regex>, Option<Regex>) {
    static CELL: OnceLock<(Option<Regex>, Option<Regex>)> = OnceLock::new();
    CELL.get_or_init(|| {
        (
            // Failure: a non-zero "N failed", "test result: FAILED", a line
            // starting FAIL / --- FAIL (jest, go), "Failed!" (dotnet).
            Regex::new(
                r"(?im)\b[1-9]\d*\s+(?:failed|failing)\b|test result:\s*failed|^\s*(?:---\s*)?fail\b|\bfailed!",
            )
            .ok(),
            // Success: "test result: ok", "N passed", "Passed!" (dotnet), go's "ok  pkg".
            Regex::new(r"(?im)test result:\s*ok|\b[1-9]\d*\s+passed\b|\bpassed!|^ok\s+\S+")
                .ok(),
        )
    })
}

/// Pass/fail: the exit code wins; without one, output patterns (failure
/// beats success); otherwise unknown.
pub fn test_result(exit_code: Option<i64>, output: Option<&str>) -> Option<bool> {
    if let Some(code) = exit_code {
        return Some(code == 0);
    }
    let out = output?;
    let (fail, pass) = patterns();
    if fail.as_ref().is_some_and(|r| r.is_match(out)) {
        return Some(false);
    }
    if pass.as_ref().is_some_and(|r| r.is_match(out)) {
        return Some(true);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Uuid {
        Uuid::from_u128(1)
    }

    fn prompt(text: &str) -> TurnEvent {
        TurnEvent::Prompt {
            turn_id: None,
            prompt: Some(text.into()),
            started_ms: None,
        }
    }

    fn tool(name: &str, paths: &[&str], cmd: Option<&str>, exit: Option<i64>) -> TurnEvent {
        TurnEvent::Tool(ToolRecord {
            name: name.into(),
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
            command: cmd.map(str::to_owned),
            exit_code: exit,
            interrupted: false,
            output_tail: None,
        })
    }

    fn stop() -> TurnEvent {
        TurnEvent::Stop {
            message: Some("done".into()),
        }
    }

    #[test]
    fn prompt_tools_stop_produces_facts() {
        let mut t = TurnTracker::new();
        assert!(t.on_event(pane(), "s", 1000, prompt("fix it")).is_none());
        assert_eq!(t.open_count(), 1);
        assert!(t
            .on_event(pane(), "s", 1100, tool("Edit", &["/r/a.rs"], None, None))
            .is_none());
        t.on_event(
            pane(),
            "s",
            1200,
            tool("Edit", &["/r/a.rs", "/r/b.rs"], None, None),
        );
        t.on_event(pane(), "s", 1300, tool("Bash", &[], Some("ls"), Some(0)));
        t.on_event(pane(), "s", 1350, TurnEvent::Permission);
        let f = t.on_event(pane(), "s", 2000, stop()).unwrap();
        assert_eq!(t.open_count(), 0);
        assert_eq!(f.outcome, TurnOutcome::Completed);
        assert_eq!(f.prompt.as_deref(), Some("fix it"));
        assert_eq!(f.tool_count, 3);
        assert_eq!(f.tools["Edit"], 2);
        assert_eq!(f.files, vec!["/r/a.rs", "/r/b.rs"]);
        assert_eq!(f.commands.len(), 1);
        assert_eq!(f.permission_requests, 1);
        assert_eq!(f.last_message.as_deref(), Some("done"));
        assert_eq!(f.duration_ms(), 1000);
        assert!(f.test.is_none());
        assert!(f.stats.is_none());
        let f = f.with_stats(DiffStat {
            files: 2,
            insertions: 5,
            deletions: 1,
        });
        assert_eq!(f.stats.unwrap().files, 2);
    }

    #[test]
    fn new_prompt_interrupts_open_turn() {
        let mut t = TurnTracker::new();
        t.on_event(pane(), "s", 10, prompt("one"));
        let closed = t.on_event(pane(), "s", 50, prompt("two")).unwrap();
        assert_eq!(closed.outcome, TurnOutcome::Interrupted);
        assert_eq!(closed.prompt.as_deref(), Some("one"));
        assert_eq!(closed.ended_ms, 50);
        let second = t.on_event(pane(), "s", 90, stop()).unwrap();
        assert_eq!(second.prompt.as_deref(), Some("two"));
        assert_ne!(second.turn_id, closed.turn_id);
    }

    #[test]
    fn stop_failure_and_stray_stop() {
        let mut t = TurnTracker::new();
        assert!(t.on_event(pane(), "s", 1, stop()).is_none());
        t.on_event(pane(), "s", 2, prompt("p"));
        let f = t
            .on_event(
                pane(),
                "s",
                9,
                TurnEvent::StopFailure {
                    error: "rate_limit".into(),
                    message: None,
                },
            )
            .unwrap();
        assert_eq!(f.outcome, TurnOutcome::Failed("rate_limit".into()));
    }

    #[test]
    fn sessions_and_panes_are_independent() {
        let mut t = TurnTracker::new();
        t.on_event(pane(), "a", 1, prompt("a"));
        t.on_event(pane(), "b", 1, prompt("b"));
        t.on_event(Uuid::from_u128(2), "a", 1, prompt("c"));
        assert_eq!(t.open_count(), 3);
        assert_eq!(
            t.on_event(pane(), "a", 5, stop())
                .unwrap()
                .prompt
                .as_deref(),
            Some("a")
        );
        t.forget_pane(pane());
        assert_eq!(t.open_count(), 1);
    }

    #[test]
    fn tool_without_prompt_opens_anonymous_turn() {
        let mut t = TurnTracker::new();
        t.on_event(pane(), "s", 5, tool("Write", &["/r/x"], None, None));
        let f = t.on_event(pane(), "s", 8, stop()).unwrap();
        assert!(f.prompt.is_none());
        assert_eq!(f.files, vec!["/r/x"]);
    }

    #[test]
    fn caps_hold() {
        let mut t = TurnTracker::new();
        t.on_event(pane(), "s", 0, prompt("p"));
        for i in 0..100 {
            let path = format!("/r/{i}");
            t.on_event(
                pane(),
                "s",
                1,
                tool("Edit", &[path.as_str()], Some(&format!("c{i}")), None),
            );
        }
        let f = t.on_event(pane(), "s", 2, stop()).unwrap();
        assert_eq!(f.files.len(), MAX_TURN_FILES);
        assert_eq!(f.commands.len(), MAX_TURN_COMMANDS);
        assert_eq!(f.tool_count, 100);

        let mut t = TurnTracker::new();
        for i in 0..(MAX_OPEN_TURNS as u128 + 10) {
            t.on_event(Uuid::from_u128(i), "s", 0, prompt("p"));
        }
        assert_eq!(t.open_count(), MAX_OPEN_TURNS);
    }

    #[test]
    fn codex_turn_uses_its_own_id_and_the_workers_clock() {
        let mut t = TurnTracker::new();
        let started = CodexItem::TaskStarted {
            turn_id: "ct1".into(),
            started_at: Some(100),
        };
        t.on_event(pane(), "s", 999_999, TurnEvent::from_codex(&started));
        let cmd = CodexItem::Command {
            turn_id: "ct1".into(),
            command: "cargo test".into(),
            cwd: "/r".into(),
            exit_code: Some(0),
            duration_ms: Some(10),
            output_tail: None,
        };
        t.on_event(pane(), "s", 999_999, TurnEvent::from_codex(&cmd));
        let done = CodexItem::TaskComplete {
            turn_id: "ct1".into(),
            duration_ms: Some(2500),
            last_agent_message: Some("ok".into()),
        };
        let f = t
            .on_event(pane(), "s", 999_999, TurnEvent::from_codex(&done))
            .unwrap();
        assert_eq!(f.turn_id, "ct1");
        assert_eq!(f.started_ms, 100_000);
        // The end is the worker's clock, not the rollout's claimed duration.
        assert_eq!(f.ended_ms, 999_999);
        assert_eq!(f.test.unwrap().passed, Some(true));
    }

    #[test]
    fn from_raw_maps_kinds() {
        let raw = |kind| ActivityRaw {
            kind,
            session_id: "s".into(),
            cwd: String::new(),
            transcript_path: String::new(),
            agent_id: None,
            prompt: Some("p".into()),
            tool_name: Some("Bash".into()),
            paths: vec![],
            command: Some("cargo test".into()),
            exit_code: Some(1),
            interrupted: false,
            output_tail: None,
            message: None,
            error: None,
        };
        assert!(TurnEvent::from_raw(&raw(RawKind::ToolStart)).is_none());
        assert!(matches!(
            TurnEvent::from_raw(&raw(RawKind::StopFailure)),
            Some(TurnEvent::StopFailure { error, .. }) if error == "unknown"
        ));
        let mut t = TurnTracker::new();
        t.on_event(
            pane(),
            "s",
            0,
            TurnEvent::from_raw(&raw(RawKind::Prompt)).unwrap(),
        );
        t.on_event(
            pane(),
            "s",
            1,
            TurnEvent::from_raw(&raw(RawKind::ToolEnd)).unwrap(),
        );
        let f = t
            .on_event(
                pane(),
                "s",
                2,
                TurnEvent::from_raw(&raw(RawKind::Stop)).unwrap(),
            )
            .unwrap();
        assert_eq!(f.test.unwrap().passed, Some(false));
    }

    #[test]
    fn runner_detection() {
        for (cmd, want) in [
            ("cargo test -p ymux", "cargo"),
            ("cd x && pnpm test", "pnpm"),
            ("pnpm run test", "pnpm"),
            ("npm test", "npm"),
            ("npm run test -- --watch=false", "npm"),
            ("yarn test", "yarn"),
            ("npx vitest run", "vitest"),
            ("./node_modules/.bin/jest --ci", "jest"),
            (".\\node_modules\\.bin\\vitest.cmd", "vitest"),
            ("pytest -q", "pytest"),
            ("python -m pytest tests", "pytest"),
            ("go test ./...", "go"),
            ("dotnet test", "dotnet"),
            ("CARGO_TERM_COLOR=never cargo test", "cargo"),
        ] {
            assert_eq!(test_runner(cmd), Some(want), "{cmd}");
        }
        for cmd in [
            "cargo build",
            "pnpm install",
            "echo test",
            "git commit -m 'cargo'",
            "",
        ] {
            assert_eq!(test_runner(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn result_from_exit_then_output_else_none() {
        assert_eq!(test_result(Some(0), Some("1 failed")), Some(true));
        assert_eq!(test_result(Some(2), None), Some(false));
        assert_eq!(
            test_result(None, Some("test result: ok. 5 passed; 0 failed")),
            Some(true)
        );
        assert_eq!(
            test_result(None, Some("test result: FAILED. 4 passed; 1 failed")),
            Some(false)
        );
        assert_eq!(
            test_result(None, Some("Tests: 2 failed, 8 passed")),
            Some(false)
        );
        assert_eq!(test_result(None, Some("FAIL src/a.test.ts")), Some(false));
        assert_eq!(
            test_result(None, Some("=== 12 passed in 0.5s ===")),
            Some(true)
        );
        assert_eq!(
            test_result(None, Some("ok  \texample.com/pkg\t0.01s")),
            Some(true)
        );
        assert_eq!(
            test_result(None, Some("Passed!  - Failed: 0, Passed: 3")),
            Some(true)
        );
        assert_eq!(test_result(None, Some("compiling...")), None);
        assert_eq!(test_result(None, None), None);
    }

    #[test]
    fn interrupted_test_is_unknown() {
        let mut t = TurnTracker::new();
        t.on_event(pane(), "s", 0, prompt("p"));
        t.on_event(
            pane(),
            "s",
            1,
            TurnEvent::Tool(ToolRecord {
                name: "Bash".into(),
                paths: vec![],
                command: Some("pytest".into()),
                exit_code: Some(130),
                interrupted: true,
                output_tail: None,
            }),
        );
        let f = t.on_event(pane(), "s", 2, stop()).unwrap();
        assert_eq!(f.test.unwrap().passed, None);
    }

    #[test]
    fn a_claimed_start_in_the_future_is_clamped_to_now() {
        let mut t = TurnTracker::new();
        let started = CodexItem::TaskStarted {
            turn_id: "ct2".into(),
            started_at: Some(4_000_000_000),
        };
        t.on_event(pane(), "s", 5_000, TurnEvent::from_codex(&started));
        let done = CodexItem::TaskComplete {
            turn_id: "ct2".into(),
            duration_ms: Some(u64::MAX),
            last_agent_message: None,
        };
        let f = t
            .on_event(pane(), "s", 6_000, TurnEvent::from_codex(&done))
            .unwrap();
        assert_eq!((f.started_ms, f.ended_ms), (5_000, 6_000));
        assert_eq!(f.duration_ms(), 1_000);
    }
}
