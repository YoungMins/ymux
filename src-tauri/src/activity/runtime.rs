//! Desktop glue for the activity log: the worker thread, its [`Env`] over the
//! app's backend state, the Codex tail on the agent-scan tick, and the
//! commands the UI uses.
//!
//! Everything pure lives in the sibling modules; this file only wires them to
//! `AppState`. The runtime is managed state (`app.manage`) created in `setup`
//! *before* the hook receiver, which takes its [`ActivityTap`] from it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::ipc::Request;
use tauri::{AppHandle, Manager, State, Webview};
use uuid::Uuid;

use super::gitstat::{self, GitBaseline};
use super::hub::ActivityHub;
use super::store::{ActivityStore, Record};
use super::tail::CodexTailer;
use super::turn::DiffStat;
use super::worker::{self, Env, PaneCtx, Settings, Worker};
use super::ActivityMsg;
use crate::agent_sessions::{AgentKind, IdSource, SharedSessions};
use crate::commands::AppState;
use crate::config::model::{ActivityConfig, Config};
use crate::fspath::guard_local;
use crate::hook_http::{ActivityTap, ACTIVITY_QUEUE};
use crate::YmuxResult;

/// How long a config snapshot is reused for pane lookups.
const CONFIG_TTL: Duration = Duration::from_secs(2);
/// Most records one `activity_read` returns.
const MAX_READ_RECORDS: usize = 1000;

/// `<config>/ymux/activity/`.
pub fn activity_dir() -> PathBuf {
    ytheme::config_dir()
        .unwrap_or_else(|| PathBuf::from("./ymux-config"))
        .join("activity")
}

impl Settings {
    pub fn from_config(c: &ActivityConfig) -> Self {
        Self {
            enabled: c.enabled,
            record_prompts: c.record_prompts,
            lang: c.language.clone(),
            retention_days: c.retention_days,
            max_total_mb: c.max_total_mb,
            publish_disk_sourced: false,
        }
    }
}

/// Managed state.
pub struct ActivityRuntime {
    pub tap: ActivityTap,
    /// Finished `turn_end` / `permission` records, for the webhook dispatcher
    /// (`hub.subscribe`); nothing is delivered from here.
    pub hub: Arc<ActivityHub>,
    pub dir: PathBuf,
    settings: Arc<Mutex<Settings>>,
    tailer: Mutex<CodexTailer>,
}

impl ActivityRuntime {
    /// Apply the user's settings: the capture switch and the worker's live
    /// settings. Cheap; called on startup and on every `save_config`.
    pub fn apply_config(&self, c: &ActivityConfig) {
        *self.settings.lock() = Settings::from_config(c);
        self.tap.enabled.store(c.enabled, Ordering::Relaxed);
    }

    fn store(&self) -> ActivityStore {
        let s = self.settings.lock();
        ActivityStore::new(self.dir.clone(), s.retention_days, s.max_total_mb)
    }

    /// A pane was killed: the worker drops what it had open, the tail stops.
    pub fn forget_pane(&self, pane: Uuid) {
        self.tailer.lock().forget(pane);
        // Best effort: a full queue just leaves it to the worker's sweep.
        self.tap.offer(pane, ActivityMsg::Forget);
    }
}

/// The worker's view of the app.
struct AppEnv {
    app: AppHandle,
    cache: Option<(Instant, Config)>,
}

impl AppEnv {
    fn config(&mut self) -> &Config {
        let stale = self
            .cache
            .as_ref()
            .map_or(true, |(at, _)| at.elapsed() > CONFIG_TTL);
        if stale {
            let cfg = self.app.state::<AppState>().config.snapshot();
            self.cache = Some((Instant::now(), cfg));
        }
        &self.cache.as_ref().expect("just filled").1
    }
}

impl Env for AppEnv {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }

    fn pane_ctx(&mut self, pane: Uuid) -> PaneCtx {
        let mut ctx = PaneCtx::default();
        let mut configured_cwd = String::new();
        {
            let cfg = self.config();
            for ws in &cfg.workspaces {
                if let Some(p) = ws.panes().into_iter().find(|p| p.id == pane) {
                    ctx.ws = ws.id;
                    ctx.title = p.title.clone().unwrap_or_default();
                    ctx.account =
                        crate::accounts::effective_account(&cfg.accounts, &p.account, &ws.account)
                            .map(|a| a.id.clone())
                            .unwrap_or_default();
                    configured_cwd = p.cwd.clone().unwrap_or_default();
                    break;
                }
            }
        }
        // The live OSC 7 directory wins over the layout's saved one.
        ctx.cwd = self
            .app
            .state::<AppState>()
            .pty
            .cwd_for(pane)
            .unwrap_or(configured_cwd);
        ctx
    }

    fn is_live(&self, pane: Uuid) -> bool {
        self.app.state::<AppState>().pty.has(pane)
    }

    fn branch(&mut self, cwd: &str) -> String {
        crate::git::current_branch(Path::new(cwd)).unwrap_or_default()
    }

    fn baseline(&mut self, cwd: &str) -> Option<GitBaseline> {
        gitstat::baseline(cwd)
    }

    fn stat(&mut self, baseline: &GitBaseline) -> Option<DiffStat> {
        gitstat::stat(baseline)
    }
}

/// Create the runtime, start the worker thread and manage the state. Call
/// once in `setup`, before `commands::start_hook_receiver`.
pub fn start(app: &AppHandle) {
    let (tap, rx) = ActivityTap::new(ACTIVITY_QUEUE);
    let hub = Arc::new(ActivityHub::new());
    let cfg = app.state::<AppState>().config.snapshot();
    let settings = Arc::new(Mutex::new(Settings::from_config(&cfg.activity)));
    tap.enabled.store(cfg.activity.enabled, Ordering::Relaxed);
    let dir = activity_dir();
    let w = Worker::new(
        AppEnv {
            app: app.clone(),
            cache: None,
        },
        dir.clone(),
        settings.clone(),
        hub.clone(),
    );
    let stats = tap.stats.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("ymux-activity".into())
        .spawn(move || worker::run(w, rx, stats))
    {
        tracing::error!(error = %e, "failed to start the activity worker");
    }
    app.manage(ActivityRuntime {
        tap,
        hub,
        dir,
        settings,
        tailer: Mutex::new(CodexTailer::new()),
    });
}

/// One tick of the Codex tail, called by the agent scan with the panes that
/// are alive. Reads only for panes whose tracker record is an active Codex
/// session, at most 1 MiB each, and queues the parsed items without waiting.
pub fn codex_tick(app: &AppHandle, live: &HashSet<Uuid>) {
    let Some(rt) = app.try_state::<ActivityRuntime>() else {
        return;
    };
    if !rt.tap.enabled.load(Ordering::Relaxed) {
        // Off: follow nothing, so turning it on starts at the end of files.
        rt.tailer.lock().retain(&HashSet::new());
        return;
    }
    let targets: Vec<Target> = {
        let sessions = app.state::<SharedSessions>();
        let tracker = sessions.0.lock();
        tracker
            .store()
            .sessions
            .values()
            .filter(|s| s.agent == AgentKind::Codex && s.active && live.contains(&s.pane_id))
            .map(|s| Target {
                pane: s.pane_id,
                session: s.session_id.clone(),
                config_dir: s.config_dir.clone(),
                record_cwd: s.cwd.clone(),
                source: s.source,
            })
            .collect()
    };
    let now = crate::agent_sessions::now_secs();
    rt.tailer
        .lock()
        .retain(&targets.iter().map(|t| t.pane).collect());
    let state = app.state::<AppState>();
    for t in targets {
        let dir = if t.config_dir.is_empty() {
            state.pty.agent_dirs(t.pane).map_or_else(
                || crate::accounts::home_dirs(dirs::home_dir().as_deref()).codex,
                |d| d.codex,
            )
        } else {
            t.config_dir
        };
        let root = crate::agent_scan_disk::transcript_root(AgentKind::Codex, &dir);
        // The rollout must have run where the pane is; the recorded session
        // cwd is only the fallback for a pane whose live directory is unknown.
        let cwd = state.pty.cwd_for(t.pane).unwrap_or(t.record_cwd);
        // The tailer's lock is held only to take the pane's state out and to
        // put it back: the disk reads below must never make `kill_pane` (which
        // forgets the pane through the same lock) wait on I/O.
        let mut st = rt.tailer.lock().take(t.pane);
        let items = st.poll(&t.session, &root, &cwd, now);
        if state.pty.has(t.pane) {
            rt.tailer.lock().put(t.pane, st);
        }
        for item in items {
            rt.tap.offer(
                t.pane,
                ActivityMsg::Codex {
                    session: t.session.clone(),
                    source: t.source,
                    item,
                },
            );
        }
    }
}

/// A Codex pane the tail should look at this tick.
struct Target {
    pane: Uuid,
    session: String,
    config_dir: String,
    record_cwd: String,
    source: IdSource,
}

/// Newest `n` records (at most 1000), oldest first. Already redacted on disk.
#[tauri::command(async)]
pub fn activity_read(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, ActivityRuntime>,
    n: usize,
) -> YmuxResult<Vec<Record>> {
    guard_local(&webview, &request, "activity_read")?;
    Ok(rt.store().read_recent(n.min(MAX_READ_RECORDS)))
}

#[derive(Debug, Serialize)]
pub struct ActivityCleared {
    pub files: usize,
}

/// Delete every daily log file.
#[tauri::command(async)]
pub fn activity_clear(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, ActivityRuntime>,
) -> YmuxResult<ActivityCleared> {
    guard_local(&webview, &request, "activity_clear")?;
    Ok(ActivityCleared {
        files: rt.store().clear(),
    })
}

/// Open the log folder in the OS file manager. The path is ymux's own, never
/// frontend input.
#[tauri::command(async)]
pub fn activity_open_dir(
    webview: Webview,
    request: Request<'_>,
    rt: State<'_, ActivityRuntime>,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "activity_open_dir")?;
    std::fs::create_dir_all(&rt.dir)?;
    opener::open(&rt.dir).map_err(|e| crate::YmuxError::Other(format!("activity_open_dir: {e}")))
}
