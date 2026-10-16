//! The activity worker: everything that must not run on the hook path.
//!
//! One thread drains the bounded channel the hook tap and the Codex tail feed
//! (`ActivityMsg`). For each message it, in this order:
//!
//! 1. **redacts** every free-text string (secret patterns), masks sensitive
//!    paths and drops the hook's own `cwd`/`transcript_path` claims. The
//!    hot path hands over a window twice the final size; the final clip comes
//!    *after* redaction, so a token is never cut before it is recognised. The
//!    pane's cwd and the touched file paths stay raw in worker memory (git and
//!    attachments need the real ones) and are redacted only when a record is
//!    written;
//! 2. resolves workspace, pane title, account, cwd and branch from backend
//!    state through [`Env`] (never from the hook body);
//! 3. feeds the [`TurnTracker`]; a turn opening snapshots a git baseline, a
//!    turn closing computes the agent's `DiffStat` and the rule summary;
//! 4. appends records to the [`ActivityStore`] and, for `turn_end` and
//!    `permission` (attention) records, publishes them on the [`ActivityHub`]
//!    for the webhook dispatcher.
//!
//! Bounded work: when the queue is backed up (`backed_up`), git baselines and
//! stats are skipped so the worker catches up. The thread is the only owner of
//! the turn state, so there is no lock on it. [`Env`] is the seam to the app
//! (config, PTY cwd, git), faked in tests.

use super::codex::CodexItem;
use super::extract::{
    ActivityRaw, RawKind, MAX_CMD_CHARS, MAX_MESSAGE_CHARS, MAX_OUTPUT_TAIL_CHARS,
    MAX_PROMPT_CHARS, WINDOW,
};
use super::gitstat::GitBaseline;
use super::hub::ActivityHub;
use super::redact::{clip, is_sensitive_path, redact, redact_json, tail, REDACTED};
use super::store::{kind, ActivityStore, Record};
use super::turn::{DiffStat, TurnEvent, TurnFacts, TurnOutcome, TurnTracker};
use super::ActivityMsg;
use crate::agent_sessions::IdSource;
use crate::hook_http::TapStats;
use crate::summarizer::rule::{self, RuleInput};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Queue depth above which git work is skipped.
pub const BACKLOG: usize = 64;
const DAY_MS: u64 = 86_400_000;
const TICK: Duration = Duration::from_secs(60);

/// Live settings, shared with the config glue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Capture switch. While off the worker drops everything but `Forget`,
    /// including messages that were queued before the switch flipped.
    pub enabled: bool,
    pub record_prompts: bool,
    /// App language code for the rule summary.
    pub lang: String,
    pub retention_days: u32,
    pub max_total_mb: u32,
    /// Publish records whose session id was only inferred from a transcript
    /// on disk. Off: they are recorded locally and never leave the machine.
    pub publish_disk_sourced: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            record_prompts: false,
            lang: "en".into(),
            retention_days: super::store::DEFAULT_RETENTION_DAYS,
            max_total_mb: 200,
            publish_disk_sourced: false,
        }
    }
}

/// What backend state knows about a pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneCtx {
    pub ws: u32,
    pub title: String,
    /// Effective account id, `""` for none.
    pub account: String,
    /// The pane's live (OSC 7) directory.
    pub cwd: String,
}

pub trait Env {
    fn now_ms(&self) -> u64;
    fn pane_ctx(&mut self, pane: Uuid) -> PaneCtx;
    fn is_live(&self, pane: Uuid) -> bool;
    fn branch(&mut self, cwd: &str) -> String;
    fn baseline(&mut self, cwd: &str) -> Option<GitBaseline>;
    fn stat(&mut self, baseline: &GitBaseline) -> Option<DiffStat>;
}

/// Which agent produced an event and how its session id was learned.
#[derive(Debug, Clone, Copy)]
struct Origin {
    agent: &'static str,
    /// `hook` | `process` | `disk`, `""` for a Claude hook (the agent itself
    /// said it, through an authenticated request).
    id_source: &'static str,
}

fn origin_of(agent: &'static str, source: IdSource) -> Origin {
    Origin {
        agent,
        id_source: match source {
            IdSource::Hook => "hook",
            IdSource::Process => "process",
            IdSource::Disk => "disk",
        },
    }
}

/// The cheap, cloneable part of a turn's context. `cwd` is the raw live
/// directory (git needs it); it is redacted when a record is written.
#[derive(Debug, Clone)]
struct Meta {
    origin: Origin,
    ws: u32,
    title: String,
    account: String,
    cwd: String,
    branch: String,
}

#[derive(Default)]
struct Hint {
    /// Prompt length in chars (clipped by the extractor), kept even when the
    /// text itself is not.
    prompt_len: Option<usize>,
    /// `(tool, command)` of a permission request.
    perm: Option<(Option<String>, Option<String>)>,
}

pub struct Worker<E: Env> {
    env: E,
    tracker: TurnTracker,
    metas: HashMap<(Uuid, String), Meta>,
    baselines: HashMap<(Uuid, String), GitBaseline>,
    dir: PathBuf,
    settings: Arc<Mutex<Settings>>,
    hub: Arc<ActivityHub>,
    last_prune_ms: u64,
    last_tick_ms: u64,
}

/// A sensitive path never enters worker memory by name; any other path stays
/// raw (a long directory name must not break `git` or an attachment lookup)
/// and is redacted with the rest of the record when it is written.
fn mask_path(p: &str) -> String {
    if is_sensitive_path(p) {
        REDACTED.to_owned()
    } else {
        p.to_owned()
    }
}

/// A window the extractor truncated ends in `…` at full window length. Its
/// last word may be a secret cut below a pattern's minimum length, so that
/// partial word (everything after the last whitespace) is dropped before
/// redaction; a window with no whitespace at all is dropped whole.
fn drop_cut_word(v: &str, window: usize) -> &str {
    if v.chars().count() < window {
        return v;
    }
    let Some(body) = v.strip_suffix('…') else {
        return v;
    };
    body.rfind(char::is_whitespace).map_or("", |i| &body[..i])
}

/// Redact, then clip to the final size (the extractor left a window).
fn red_opt(s: &mut Option<String>, limit: usize) {
    if let Some(v) = s {
        *v = clip(
            &redact(drop_cut_word(v, limit.saturating_mul(WINDOW))),
            limit,
        );
    }
}

fn red_tail(s: &mut Option<String>, limit: usize) {
    if let Some(v) = s {
        *v = tail(&redact(v), limit);
    }
}

/// Redact in place, clip to the final size and forget the fields the hook is
/// not trusted for.
pub fn redact_raw(raw: &mut ActivityRaw) {
    raw.cwd.clear();
    raw.transcript_path.clear();
    red_opt(&mut raw.prompt, MAX_PROMPT_CHARS);
    red_opt(&mut raw.command, MAX_CMD_CHARS);
    red_tail(&mut raw.output_tail, MAX_OUTPUT_TAIL_CHARS);
    red_opt(&mut raw.message, MAX_MESSAGE_CHARS);
    red_opt(&mut raw.error, 80);
    for p in &mut raw.paths {
        *p = mask_path(p);
    }
    raw.paths.dedup();
}

pub fn redact_codex(item: &mut CodexItem) {
    match item {
        CodexItem::TaskStarted { .. } => {}
        CodexItem::TaskComplete {
            last_agent_message, ..
        } => red_opt(last_agent_message, MAX_MESSAGE_CHARS),
        CodexItem::Command {
            command,
            cwd,
            output_tail,
            ..
        } => {
            *command = clip(&redact(command), MAX_CMD_CHARS);
            cwd.clear();
            red_tail(output_tail, MAX_OUTPUT_TAIL_CHARS);
        }
        CodexItem::FileChange { paths, .. } => {
            for p in paths.iter_mut() {
                *p = mask_path(p);
            }
        }
    }
}

/// The per-event record (`prompt`, `tool`, `permission`); `Stop` and
/// `StopFailure` produce none (the `turn_end` record covers them).
fn event_data(ev: &TurnEvent, hint: &Hint, record_prompts: bool) -> Option<(&'static str, Value)> {
    Some(match ev {
        TurnEvent::Prompt { prompt, .. } => {
            let len = hint
                .prompt_len
                .or_else(|| prompt.as_ref().map(|p| p.chars().count()))
                .unwrap_or(0);
            let mut d = json!({ "prompt_len": len });
            if record_prompts {
                if let Some(p) = prompt {
                    d["text"] = json!(p);
                }
            }
            (kind::PROMPT, d)
        }
        TurnEvent::Tool(t) => (
            kind::TOOL,
            json!({
                "name": t.name,
                "paths": t.paths,
                "command": t.command,
                "exit_code": t.exit_code,
                "interrupted": t.interrupted,
            }),
        ),
        TurnEvent::Permission => {
            let (tool, command) = hint.perm.clone().unwrap_or_default();
            (
                kind::PERMISSION,
                json!({ "tool": tool, "command": command }),
            )
        }
        TurnEvent::Stop { .. } | TurnEvent::StopFailure { .. } => return None,
    })
}

/// Most-touched directories, relative to `cwd` when under it.
fn top_dirs(files: &[String], cwd: &str) -> Vec<String> {
    let cwd = cwd.replace('\\', "/");
    let cwd = cwd.trim_end_matches('/');
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for f in files.iter().filter(|f| f.as_str() != REDACTED) {
        let f = f.replace('\\', "/");
        let rel = match f.strip_prefix(cwd) {
            Some(r) if !cwd.is_empty() && r.starts_with('/') => r.trim_start_matches('/'),
            _ => f.as_str(),
        };
        let dir = rel.rsplit_once('/').map_or(".", |(d, _)| d);
        *counts.entry(dir.to_owned()).or_insert(0) += 1;
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.into_iter().take(3).map(|(d, _)| d).collect()
}

impl<E: Env> Worker<E> {
    pub fn new(
        env: E,
        dir: impl Into<PathBuf>,
        settings: Arc<Mutex<Settings>>,
        hub: Arc<ActivityHub>,
    ) -> Self {
        Self {
            env,
            tracker: TurnTracker::new(),
            metas: HashMap::new(),
            baselines: HashMap::new(),
            dir: dir.into(),
            settings,
            hub,
            last_prune_ms: 0,
            last_tick_ms: 0,
        }
    }

    pub fn env(&self) -> &E {
        &self.env
    }

    fn store(&self) -> ActivityStore {
        let s = self.settings.lock();
        ActivityStore::new(self.dir.clone(), s.retention_days, s.max_total_mb)
    }

    /// Process one message, then run housekeeping if a minute has passed on
    /// the worker's clock (a busy channel never reaches the idle timeout).
    pub fn handle(&mut self, pane: Uuid, msg: ActivityMsg, backed_up: bool) {
        self.handle_msg(pane, msg, backed_up);
        if self.env.now_ms().saturating_sub(self.last_tick_ms) >= TICK.as_millis() as u64 {
            self.tick();
        }
    }

    fn handle_msg(&mut self, pane: Uuid, msg: ActivityMsg, backed_up: bool) {
        if !self.settings.lock().enabled && !matches!(msg, ActivityMsg::Forget) {
            return;
        }
        match msg {
            ActivityMsg::Hook(mut raw) => {
                let record_prompts = self.settings.lock().record_prompts;
                // Measured before redaction so the length is the real one.
                let prompt_len = raw.prompt.as_ref().map(|p| p.chars().count());
                redact_raw(&mut raw);
                if !record_prompts {
                    // Prompt and assistant text are content, not facts.
                    raw.prompt = None;
                    raw.message = None;
                }
                let Some(ev) = TurnEvent::from_raw(&raw) else {
                    return;
                };
                let hint = Hint {
                    prompt_len,
                    perm: (raw.kind == RawKind::Permission)
                        .then(|| (raw.tool_name.clone(), raw.command.clone())),
                };
                let origin = Origin {
                    agent: "claude",
                    id_source: "",
                };
                self.apply(pane, &raw.session_id, origin, ev, hint, backed_up);
            }
            ActivityMsg::Codex {
                session,
                source,
                mut item,
            } => {
                redact_codex(&mut item);
                if !self.settings.lock().record_prompts {
                    if let CodexItem::TaskComplete {
                        last_agent_message, ..
                    } = &mut item
                    {
                        *last_agent_message = None;
                    }
                }
                let ev = TurnEvent::from_codex(&item);
                let origin = origin_of("codex", source);
                self.apply(pane, &session, origin, ev, Hint::default(), backed_up);
            }
            ActivityMsg::Forget => self.forget(pane),
        }
    }

    /// Drop everything a pane had open, unreported.
    pub fn forget(&mut self, pane: Uuid) {
        self.tracker.forget_pane(pane);
        self.metas.retain(|k, _| k.0 != pane);
        self.baselines.retain(|k, _| k.0 != pane);
    }

    /// Housekeeping, about once a minute: prune the log once a day (and at
    /// start), forget panes that no longer exist.
    pub fn tick(&mut self) {
        let now = self.env.now_ms();
        self.last_tick_ms = now;
        if now.saturating_sub(self.last_prune_ms) >= DAY_MS {
            self.last_prune_ms = now;
            if let Err(e) = self.store().prune(now) {
                tracing::warn!(error = %e, "pruning the activity log failed");
            }
        }
        let mut dead: Vec<Uuid> = self
            .metas
            .keys()
            .map(|k| k.0)
            .filter(|p| !self.env.is_live(*p))
            .collect();
        dead.sort();
        dead.dedup();
        for p in dead {
            self.forget(p);
        }
    }

    fn fields(&mut self, pane: Uuid, origin: Origin, with_git: bool) -> Meta {
        let c = self.env.pane_ctx(pane);
        let branch = if with_git && !c.cwd.is_empty() {
            self.env.branch(&c.cwd)
        } else {
            String::new()
        };
        Meta {
            origin,
            ws: c.ws,
            title: redact(&c.title),
            account: c.account,
            cwd: c.cwd,
            branch: redact(&branch),
        }
    }

    fn apply(
        &mut self,
        pane: Uuid,
        session: &str,
        origin: Origin,
        ev: TurnEvent,
        hint: Hint,
        backed_up: bool,
    ) {
        let now = self.env.now_ms();
        let key = (pane, session.to_owned());
        let record_prompts = self.settings.lock().record_prompts;
        let is_prompt = matches!(ev, TurnEvent::Prompt { .. });
        let is_tool = matches!(ev, TurnEvent::Tool(_));
        let closed = self.tracker.on_event(pane, session, now, ev.clone());
        if let Some(facts) = closed {
            self.finish(facts, backed_up);
        }
        if is_prompt {
            let meta = self.fields(pane, origin, !backed_up);
            self.baselines.remove(&key);
            if !backed_up {
                if let Some(b) = self.env.baseline(&meta.cwd) {
                    self.baselines.insert(key.clone(), b);
                }
            }
            self.metas.insert(key.clone(), meta);
        } else if is_tool && !self.metas.contains_key(&key) {
            // A turn the tracker opened mid-way: no baseline, so no stats.
            let meta = self.fields(pane, origin, false);
            self.metas.insert(key.clone(), meta);
        }
        // The tracker drops its oldest open turn when it is full; whatever
        // this side kept for an evicted turn goes with it.
        let open = self.tracker.open_count();
        if self.metas.len() > open || self.baselines.len() > open {
            let tracker = &self.tracker;
            self.metas.retain(|k, _| tracker.is_open(k.0, &k.1));
            self.baselines.retain(|k, _| tracker.is_open(k.0, &k.1));
        }
        let Some((rec_kind, data)) = event_data(&ev, &hint, record_prompts) else {
            return;
        };
        let meta = match self.metas.get(&key) {
            Some(m) => m.clone(),
            None => self.fields(pane, origin, false),
        };
        let turn = self
            .tracker
            .open_turn_id(pane, session)
            .unwrap_or("")
            .to_owned();
        self.write(now, rec_kind, &meta, pane, session, &turn, data);
    }

    fn finish(&mut self, mut facts: TurnFacts, backed_up: bool) {
        let key = (facts.pane, facts.session.clone());
        let baseline = self.baselines.remove(&key);
        let meta = match self.metas.remove(&key) {
            Some(m) => m,
            None => self.fields(
                facts.pane,
                Origin {
                    agent: "claude",
                    id_source: "",
                },
                false,
            ),
        };
        if !backed_up {
            if let Some(b) = baseline {
                if let Some(stat) = self.env.stat(&b) {
                    facts = facts.with_stats(stat);
                }
            }
        }
        let (files, add, del) = match facts.stats {
            Some(s) => (s.files, s.insertions, s.deletions),
            None => (u32::try_from(facts.files.len()).unwrap_or(u32::MAX), 0, 0),
        };
        let stop_reason = match &facts.outcome {
            TurnOutcome::Completed => "end_turn".to_owned(),
            TurnOutcome::Interrupted => "interrupted".to_owned(),
            TurnOutcome::Failed(e) => format!("error: {e}"),
        };
        let lang = self.settings.lock().lang.clone();
        let summary = rule::summarize(
            &RuleInput {
                files,
                add,
                del,
                tests: facts.test.as_ref().map(|t| (t.runner.clone(), t.passed)),
                duration_ms: Some(facts.duration_ms()),
                stop_reason: Some(stop_reason),
                top_dirs: top_dirs(&facts.files, &meta.cwd),
                commands: u32::try_from(facts.commands.len()).unwrap_or(u32::MAX),
            },
            &lang,
        );
        let mut data = serde_json::to_value(&facts).unwrap_or(Value::Null);
        if let Value::Object(m) = &mut data {
            m.remove("pane");
            m.insert("duration_ms".into(), json!(facts.duration_ms()));
            m.insert(
                "summary".into(),
                json!({ "headline": summary.headline, "bullets": summary.bullets }),
            );
        }
        self.write(
            facts.ended_ms,
            kind::TURN_END,
            &meta,
            facts.pane,
            &facts.session,
            &facts.turn_id,
            data,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        ts: u64,
        record_kind: &str,
        meta: &Meta,
        pane: Uuid,
        session: &str,
        turn: &str,
        mut data: Value,
    ) {
        // Raw cwd and file paths were kept for git; everything leaving worker
        // memory for a record goes through redaction here.
        redact_json(&mut data);
        if let (Value::Object(m), false) = (&mut data, meta.title.is_empty()) {
            m.insert("title".into(), json!(meta.title));
        }
        let mut rec = Record::new(ts, record_kind, data);
        rec.ws = meta.ws;
        rec.pane = pane.to_string();
        rec.agent = meta.origin.agent.to_owned();
        rec.id_source = meta.origin.id_source.to_owned();
        rec.account = meta.account.clone();
        rec.session = session.to_owned();
        rec.turn = turn.to_owned();
        rec.cwd = redact(&meta.cwd);
        rec.branch = meta.branch.clone();
        if let Err(e) = self.store().append(&rec) {
            tracing::warn!(error = %e, "writing the activity log failed");
        }
        let publishable = rec.id_source != "disk" || self.settings.lock().publish_disk_sourced;
        if publishable && (record_kind == kind::TURN_END || record_kind == kind::PERMISSION) {
            self.hub.publish(&rec);
        }
    }
}

/// The worker thread body: drain until every sender is gone.
pub fn run<E: Env>(mut w: Worker<E>, rx: Receiver<(Uuid, ActivityMsg)>, stats: Arc<TapStats>) {
    w.tick();
    loop {
        match rx.recv_timeout(TICK) {
            Ok((pane, msg)) => {
                let before = stats.pending.fetch_sub(1, Ordering::SeqCst);
                w.handle(pane, msg, before > BACKLOG);
            }
            Err(RecvTimeoutError::Timeout) => w.tick(),
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::codex::CodexItem;
    use crate::activity::extract::from_hook;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Calls {
        baseline: AtomicUsize,
        stat: AtomicUsize,
        branch: AtomicUsize,
        baseline_cwds: Mutex<Vec<String>>,
    }

    struct FakeEnv {
        now: u64,
        ctx: PaneCtx,
        live: bool,
        calls: Arc<Calls>,
    }

    impl Env for FakeEnv {
        fn now_ms(&self) -> u64 {
            self.now
        }
        fn pane_ctx(&mut self, _: Uuid) -> PaneCtx {
            self.ctx.clone()
        }
        fn is_live(&self, _: Uuid) -> bool {
            self.live
        }
        fn branch(&mut self, _: &str) -> String {
            self.calls.branch.fetch_add(1, Ordering::SeqCst);
            "feat/x".into()
        }
        fn baseline(&mut self, cwd: &str) -> Option<GitBaseline> {
            self.calls.baseline.fetch_add(1, Ordering::SeqCst);
            self.calls.baseline_cwds.lock().push(cwd.to_owned());
            Some(GitBaseline {
                root: "/repo".into(),
                head: "abc".into(),
                files: Default::default(),
            })
        }
        fn stat(&mut self, _: &GitBaseline) -> Option<DiffStat> {
            self.calls.stat.fetch_add(1, Ordering::SeqCst);
            Some(DiffStat {
                files: 2,
                insertions: 10,
                deletions: 3,
            })
        }
    }

    struct Rig {
        w: Worker<FakeEnv>,
        dir: tempfile::TempDir,
        calls: Arc<Calls>,
        seen: Arc<Mutex<Vec<Record>>>,
        pane: Uuid,
    }

    fn rig(record_prompts: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(Calls::default());
        let env = FakeEnv {
            now: 1_792_108_800_000 + 1000,
            ctx: PaneCtx {
                ws: 3,
                title: "api".into(),
                account: "work".into(),
                cwd: "/repo".into(),
            },
            live: true,
            calls: calls.clone(),
        };
        let hub = Arc::new(ActivityHub::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        hub.subscribe(Arc::new(move |r| s2.lock().push(r.clone())));
        let settings = Arc::new(Mutex::new(Settings {
            record_prompts,
            ..Settings::default()
        }));
        let w = Worker::new(env, dir.path().join("activity"), settings, hub);
        Rig {
            w,
            dir,
            calls,
            seen,
            pane: Uuid::from_u128(7),
        }
    }

    fn hook(v: Value) -> ActivityMsg {
        ActivityMsg::Hook(from_hook(&v).expect("recorded event"))
    }

    fn prompt(text: &str) -> ActivityMsg {
        hook(
            json!({"hook_event_name": "UserPromptSubmit", "session_id": "s1",
                    "cwd": "/HOOK/CLAIM", "prompt": text}),
        )
    }

    fn stop() -> ActivityMsg {
        hook(json!({"hook_event_name": "Stop", "session_id": "s1",
                    "last_assistant_message": "all done"}))
    }

    fn tool(name: &str, input: Value) -> ActivityMsg {
        hook(json!({"hook_event_name": "PostToolUse", "session_id": "s1",
                    "tool_name": name, "tool_input": input,
                    "tool_response": {"stdout": "ok", "exit_code": 0}}))
    }

    fn records(r: &Rig) -> Vec<Record> {
        ActivityStore::new(r.dir.path().join("activity"), 14, 200).read_recent(100)
    }

    fn disk_text(r: &Rig) -> String {
        let mut out = String::new();
        for e in std::fs::read_dir(r.dir.path().join("activity"))
            .unwrap()
            .flatten()
        {
            out.push_str(&std::fs::read_to_string(e.path()).unwrap());
        }
        out
    }

    #[test]
    fn a_turn_becomes_prompt_tool_and_turn_end_records() {
        let mut r = rig(false);
        let p = r.pane;
        r.w.handle(p, prompt("fix the bug"), false);
        r.w.handle(
            p,
            tool("Write", json!({"file_path": "/repo/src/a.rs"})),
            false,
        );
        r.w.handle(p, tool("Write", json!({"file_path": "/repo/.env"})), false);
        r.w.handle(
            p,
            tool("Bash", json!({"command": "cargo test -p x"})),
            false,
        );
        r.w.handle(p, stop(), false);

        let recs = records(&r);
        let kinds: Vec<&str> = recs.iter().map(|x| x.kind.as_str()).collect();
        assert_eq!(kinds, ["prompt", "tool", "tool", "tool", "turn_end"]);
        let end = recs.last().unwrap();
        // Context from backend state, never from the hook claim.
        assert_eq!(
            (end.ws, end.agent.as_str(), end.account.as_str()),
            (3, "claude", "work")
        );
        assert_eq!(end.cwd, "/repo");
        assert_eq!(end.branch, "feat/x");
        assert_eq!(end.pane, p.to_string());
        assert!(!disk_text(&r).contains("HOOK/CLAIM"));
        // Stats from the git delta, summary from the rule.
        assert_eq!(end.data["stats"]["insertions"], 10);
        assert_eq!(end.data["outcome"]["status"], "completed");
        assert_eq!(end.data["summary"]["headline"], "Changed 2 files (+10/-3)");
        assert_eq!(end.data["title"], "api");
        assert_eq!(end.data["test"]["runner"], "cargo");
        // The sensitive path never reaches disk by name.
        assert!(!disk_text(&r).contains(".env"));
        assert!(end.data["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == REDACTED));
        // The turn id is shared by the in-turn records.
        assert_eq!(recs[1].turn, end.turn);
        assert!(!end.turn.is_empty());
        // Only the turn_end was broadcast.
        let seen = r.seen.lock();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].kind, "turn_end");
        assert_eq!(r.calls.baseline.load(Ordering::SeqCst), 1);
        assert_eq!(r.calls.stat.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn disabled_worker_drops_everything_but_forget() {
        let mut r = rig(false);
        let p = r.pane;
        r.w.settings.lock().enabled = false;
        r.w.handle(p, prompt("go"), false);
        r.w.handle(p, stop(), false);
        assert!(records(&r).is_empty());
        assert!(r.seen.lock().is_empty());
        // Re-enabled: the dropped turn left no half-open state behind.
        r.w.settings.lock().enabled = true;
        r.w.handle(p, stop(), false);
        r.w.handle(p, ActivityMsg::Forget, false);
    }

    #[test]
    fn prompt_text_is_stored_only_when_enabled_and_redacted() {
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let text = format!("use token {secret} please");
        let mut off = rig(false);
        let p = off.pane;
        off.w.handle(p, prompt(&text), false);
        off.w.handle(p, stop(), false);
        let disk = disk_text(&off);
        assert!(!disk.contains("please") && !disk.contains(secret));
        let recs = records(&off);
        assert_eq!(recs[0].data["prompt_len"], text.chars().count());
        assert!(recs[0].data.get("text").is_none());
        assert!(recs.last().unwrap().data["prompt"].is_null());

        let mut on = rig(true);
        let p = on.pane;
        on.w.handle(p, prompt(&text), false);
        on.w.handle(p, stop(), false);
        let disk = disk_text(&on);
        assert!(!disk.contains(secret), "secret must be redacted: {disk}");
        let recs = records(&on);
        let t = recs[0].data["text"].as_str().unwrap();
        assert!(t.contains("please") && t.contains(REDACTED));
    }

    #[test]
    fn secrets_in_commands_and_output_never_reach_disk() {
        let mut r = rig(false);
        let p = r.pane;
        r.w.handle(p, prompt("go"), false);
        r.w.handle(
            p,
            tool(
                "Bash",
                json!({"command": "curl -H 'Authorization: Bearer abcdefghijklmnop' x"}),
            ),
            false,
        );
        r.w.handle(p, stop(), false);
        let disk = disk_text(&r);
        assert!(!disk.contains("abcdefghijklmnop"), "{disk}");
    }

    #[test]
    fn a_backed_up_queue_skips_git_work() {
        let mut r = rig(false);
        let p = r.pane;
        r.w.handle(p, prompt("go"), true);
        r.w.handle(p, tool("Write", json!({"file_path": "/repo/a.rs"})), true);
        r.w.handle(p, stop(), true);
        assert_eq!(r.calls.baseline.load(Ordering::SeqCst), 0);
        assert_eq!(r.calls.stat.load(Ordering::SeqCst), 0);
        assert_eq!(r.calls.branch.load(Ordering::SeqCst), 0);
        let end = records(&r).pop().unwrap();
        assert_eq!(end.kind, "turn_end");
        assert!(end.data["stats"].is_null());
        // Falls back to the files the tools named.
        assert_eq!(end.data["summary"]["headline"], "Changed 1 file (+0/-0)");
    }

    #[test]
    fn a_new_prompt_closes_the_old_turn_as_interrupted() {
        let mut r = rig(false);
        let p = r.pane;
        r.w.handle(p, prompt("one"), false);
        r.w.handle(p, prompt("two"), false);
        let recs = records(&r);
        let kinds: Vec<&str> = recs.iter().map(|x| x.kind.as_str()).collect();
        assert_eq!(kinds, ["prompt", "turn_end", "prompt"]);
        assert_eq!(recs[1].data["outcome"]["status"], "interrupted");
        assert_ne!(recs[0].turn, recs[2].turn);
    }

    #[test]
    fn a_permission_request_is_recorded_and_broadcast() {
        let mut r = rig(false);
        let p = r.pane;
        r.w.handle(
            p,
            hook(
                json!({"hook_event_name": "PermissionRequest", "session_id": "s1",
                        "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}}),
            ),
            false,
        );
        let recs = records(&r);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].kind, "permission");
        assert_eq!(recs[0].data["command"], "rm -rf build");
        assert_eq!(recs[0].ws, 3);
        assert_eq!(r.seen.lock().len(), 1);
    }

    #[test]
    fn a_failed_turn_is_recorded_with_its_error() {
        let mut r = rig(false);
        let p = r.pane;
        r.w.handle(p, prompt("go"), false);
        r.w.handle(
            p,
            hook(json!({"hook_event_name": "StopFailure", "session_id": "s1",
                        "error": "rate_limit"})),
            false,
        );
        let end = records(&r).pop().unwrap();
        assert_eq!(end.data["outcome"]["status"], "failed");
        assert_eq!(end.data["outcome"]["error"], "rate_limit");
        assert_eq!(end.data["summary"]["headline"], "Stopped early");
    }

    #[test]
    fn codex_items_flow_through_the_same_path() {
        let mut r = rig(false);
        let p = r.pane;
        let m = |item| ActivityMsg::Codex {
            session: "cx1".into(),
            source: IdSource::Process,
            item,
        };
        r.w.handle(
            p,
            m(CodexItem::TaskStarted {
                turn_id: "t9".into(),
                started_at: None,
            }),
            false,
        );
        r.w.handle(
            p,
            m(CodexItem::Command {
                turn_id: "t9".into(),
                command: "npm test".into(),
                cwd: "/elsewhere".into(),
                exit_code: Some(0),
                duration_ms: None,
                output_tail: None,
            }),
            false,
        );
        r.w.env.now += 5000;
        r.w.handle(
            p,
            m(CodexItem::TaskComplete {
                turn_id: "t9".into(),
                duration_ms: Some(5000),
                last_agent_message: Some("ok".into()),
            }),
            false,
        );
        let recs = records(&r);
        let end = recs.last().unwrap();
        assert_eq!(end.kind, "turn_end");
        assert_eq!(end.agent, "codex");
        assert_eq!(end.id_source, "process");
        assert_eq!(end.session, "cx1");
        assert_eq!(end.turn, "t9");
        assert_eq!(end.data["duration_ms"], 5000);
        assert!(!disk_text(&r).contains("/elsewhere"));
    }

    #[test]
    fn forget_and_dead_panes_drop_open_turns_unreported() {
        let mut r = rig(false);
        let p = r.pane;
        r.w.handle(p, prompt("go"), false);
        r.w.handle(p, ActivityMsg::Forget, false);
        r.w.handle(p, stop(), false);
        assert_eq!(records(&r).len(), 1, "only the prompt record");

        let mut r = rig(false);
        let p = r.pane;
        r.w.handle(p, prompt("go"), false);
        r.w.env.live = false;
        r.w.tick();
        r.w.handle(p, stop(), false);
        assert_eq!(records(&r).len(), 1);
    }

    #[test]
    fn the_first_tick_prunes_old_days() {
        let mut r = rig(false);
        let dir = r.dir.path().join("activity");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2020-01-01.jsonl"), "{}\n").unwrap();
        r.w.tick();
        assert!(!dir.join("2020-01-01.jsonl").exists());
    }

    #[test]
    fn top_dirs_are_relative_and_ranked() {
        let files: Vec<String> = ["/r/src/a.rs", "/r/src/b.rs", "/r/docs/x.md", REDACTED]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(top_dirs(&files, "/r"), ["src", "docs"]);
        assert_eq!(top_dirs(&["C:\\r\\a.rs".into()], "C:\\r"), ["."]);
    }

    fn codex_msg(source: IdSource, item: CodexItem) -> ActivityMsg {
        ActivityMsg::Codex {
            session: "cx2".into(),
            source,
            item,
        }
    }

    fn codex_turn(r: &mut Rig, source: IdSource, started_at: Option<i64>) {
        let p = r.pane;
        r.w.handle(
            p,
            codex_msg(
                source,
                CodexItem::TaskStarted {
                    turn_id: "t1".into(),
                    started_at,
                },
            ),
            false,
        );
        r.w.env.now += 1000;
        r.w.handle(
            p,
            codex_msg(
                source,
                CodexItem::TaskComplete {
                    turn_id: "t1".into(),
                    duration_ms: Some(u64::MAX),
                    last_agent_message: Some("the answer".into()),
                },
            ),
            false,
        );
    }

    #[test]
    fn disk_sourced_sessions_are_recorded_but_not_published() {
        let mut r = rig(false);
        codex_turn(&mut r, IdSource::Disk, None);
        let end = records(&r).pop().unwrap();
        assert_eq!(
            (end.kind.as_str(), end.id_source.as_str()),
            ("turn_end", "disk")
        );
        assert!(r.seen.lock().is_empty(), "inferred ids stay local");

        let mut r = rig(false);
        codex_turn(&mut r, IdSource::Process, None);
        assert_eq!(r.seen.lock().len(), 1);
        assert_eq!(r.seen.lock()[0].id_source, "process");

        // The opt-in publishes them.
        let mut r = rig(false);
        r.w.settings.lock().publish_disk_sourced = true;
        codex_turn(&mut r, IdSource::Disk, None);
        assert_eq!(r.seen.lock().len(), 1);
    }

    #[test]
    fn codex_turn_end_uses_the_workers_clock() {
        let mut r = rig(false);
        let now = r.w.env.now;
        // A rollout claiming a start far in the future and a huge duration.
        codex_turn(&mut r, IdSource::Process, Some(4_000_000_000));
        let end = records(&r).pop().unwrap();
        assert_eq!(end.ts, now + 1000);
        assert_eq!(end.data["duration_ms"], 1000);
    }

    #[test]
    fn assistant_text_is_kept_only_when_prompts_are_recorded() {
        let mut off = rig(false);
        let p = off.pane;
        off.w.handle(p, prompt("go"), false);
        off.w.handle(p, stop(), false);
        assert!(records(&off).pop().unwrap().data["last_message"].is_null());
        assert!(!disk_text(&off).contains("all done"));

        let mut on = rig(true);
        let p = on.pane;
        on.w.handle(p, prompt("go"), false);
        on.w.handle(p, stop(), false);
        assert_eq!(records(&on).pop().unwrap().data["last_message"], "all done");

        let mut cx = rig(false);
        codex_turn(&mut cx, IdSource::Process, None);
        assert!(records(&cx).pop().unwrap().data["last_message"].is_null());
        assert!(!disk_text(&cx).contains("the answer"));
    }

    #[test]
    fn a_secret_across_the_final_clip_is_redacted_whole() {
        let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        // The token starts just before the 300-char final limit.
        let text = format!("{}{token} tail", "ab ".repeat(95));
        assert!(text.chars().count() > MAX_PROMPT_CHARS);
        let mut r = rig(true);
        let p = r.pane;
        r.w.handle(p, prompt(&text), false);
        r.w.handle(p, stop(), false);
        let disk = disk_text(&r);
        assert!(!disk.contains("ghp_"), "no fragment of the token: {disk}");
        let recs = records(&r);
        let t = recs[0].data["text"].as_str().unwrap();
        assert!(t.contains(REDACTED));
        assert!(t.chars().count() <= MAX_PROMPT_CHARS);
    }

    #[test]
    fn a_cut_last_word_is_dropped_before_redaction() {
        let window = 10 * WINDOW;
        // Truncated at full length: the partial word goes.
        let cut = format!("{}…", "ab cd ef gh ij kl mn o"[..window - 1].to_owned());
        assert_eq!(cut.chars().count(), window);
        assert_eq!(drop_cut_word(&cut, window), "ab cd ef gh ij kl");
        // A secret prefix cut below the pattern minimum does not survive.
        let secret = format!("{} sk-ant-abc…", "x ".repeat(8));
        assert!(!drop_cut_word(&secret, window).contains("sk-"));
        // Not truncated (short, or no marker): untouched.
        assert_eq!(drop_cut_word("short word…", window), "short word…");
        assert_eq!(
            drop_cut_word(&"abc def ".repeat(5), window),
            "abc def ".repeat(5)
        );
        // No whitespace in a cut window: nothing safe to keep.
        assert_eq!(
            drop_cut_word(&format!("{}…", "a".repeat(window - 1)), window),
            ""
        );
        // End to end through red_opt.
        let mut o = Some(secret);
        red_opt(&mut o, 10);
        assert!(!o.unwrap().contains("sk-"));
    }

    #[test]
    fn a_long_directory_name_still_gets_git_stats() {
        let mut r = rig(false);
        let long = format!("/repo/{}", "a".repeat(45));
        r.w.env.ctx.cwd = long.clone();
        let p = r.pane;
        r.w.handle(p, prompt("go"), false);
        r.w.handle(
            p,
            tool("Write", json!({"file_path": format!("{long}/x.rs")})),
            false,
        );
        r.w.handle(p, stop(), false);
        assert_eq!(*r.calls.baseline_cwds.lock(), vec![long.clone()]);
        let end = records(&r).pop().unwrap();
        assert_eq!(end.data["stats"]["insertions"], 10, "stats were computed");
        // The record itself never carries the redactable segment.
        assert!(!disk_text(&r).contains(&"a".repeat(45)));
        assert!(end.cwd.contains(REDACTED));
        // Relative directories are computed from the raw paths.
        assert_eq!(top_dirs(&[format!("{long}/src/x.rs")], &long), ["src"]);
    }

    #[test]
    fn evicted_turns_drop_their_meta_and_baseline() {
        let mut r = rig(false);
        for i in 0..300u128 {
            r.w.handle(Uuid::from_u128(1000 + i), prompt("go"), false);
        }
        let open = r.w.tracker.open_count();
        assert!(open < 300, "the tracker evicted");
        assert_eq!(r.w.metas.len(), open);
        assert_eq!(r.w.baselines.len(), open);
    }

    #[test]
    fn housekeeping_runs_while_messages_keep_arriving() {
        let mut r = rig(false);
        let p = r.pane;
        r.w.handle(p, prompt("go"), false);
        assert_eq!(r.w.metas.len(), 1);
        // The pane dies; messages from elsewhere keep the channel busy, so the
        // idle timeout never fires, but a minute passes on the worker's clock.
        r.w.env.live = false;
        r.w.env.now += TICK.as_millis() as u64 + 1;
        r.w.handle(Uuid::from_u128(99), ActivityMsg::Forget, false);
        assert!(r.w.metas.is_empty(), "the dead pane was swept");
    }
}
