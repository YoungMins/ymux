//! Tauri command handlers exposed to the frontend. Each command is a thin
//! wrapper that validates arguments and delegates to the real implementation
//! in [`crate::config`], [`crate::pty`], or [`crate::shell`].
//!
//! The goal is to keep `#[tauri::command]` fns trivial so the actual logic can
//! be unit-tested without a running webview.

use portable_pty::PtySize;
use serde::{Deserialize, Serialize};
use tauri::ipc::Request;
use tauri::{AppHandle, Emitter, Manager, State, Webview};
use uuid::Uuid;

use std::path::Path;

use crate::agent_hooks::HookDirIssue;
use crate::agent_sessions::{ResumeOutcome, SharedSessions};
use crate::agents::{AgentSnapshot, HookEvent, SharedAgents};
use crate::config::{Config, ConfigStore, ShellProfile};
use crate::error::{YmuxError, YmuxResult};
use crate::fspath::guard_local;
use crate::git;
use crate::pty::{PtyManager, SpawnedPane};
use crate::shell;

#[tauri::command]
pub async fn get_token_usage(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    refresh_quota: Option<bool>,
) -> YmuxResult<crate::token_usage::TokenSnapshot> {
    guard_local(&webview, &request, "get_token_usage")?;
    let accounts = state.config.snapshot().accounts;
    tauri::async_runtime::spawn_blocking(move || {
        let mut snapshot = crate::token_usage::collect(&accounts);
        crate::token_usage::quota::enrich(&mut snapshot, refresh_quota.unwrap_or(false));
        snapshot
    })
    .await
    .map_err(|_| YmuxError::Other("token collector unavailable".into()))
}

/// State container registered via `Tauri::manage`.
pub struct AppState {
    pub config: ConfigStore,
    pub pty: PtyManager,
    /// Panes with auto-continue on (runtime only; see `auto_continue`).
    pub auto_continue: crate::auto_continue::AutoPanes,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnArgs {
    pub id: Uuid,
    pub shell: String,
    pub cwd: Option<String>,
    pub rows: u16,
    pub cols: u16,
    /// The pane's own `PaneSpec::env`; wins over account-injected variables.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// `PaneSpec::account` (empty = inherit the workspace's).
    #[serde(default)]
    pub account: String,
    /// `Workspace::account` of the workspace owning the pane.
    #[serde(default)]
    pub workspace_account: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResizeArgs {
    pub id: Uuid,
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

#[derive(Debug, Deserialize)]
pub struct WriteArgs {
    pub id: Uuid,
    pub data: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapPayload {
    pub config: Config,
    pub shells: Vec<ShellProfile>,
    pub config_path: String,
}

/// `async`: shell detection runs probe processes and writes the shell
/// integration files, and the config flush writes to disk.
#[tauri::command(async)]
pub fn load_bootstrap(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
) -> YmuxResult<BootstrapPayload> {
    guard_local(&webview, &request, "load_bootstrap")?;
    // Make sure the cached shell list in `state.config` is populated *before*
    // we snapshot it, so the snapshot the frontend receives carries the
    // shells. Otherwise the frontend's `this.config.shells` would stay empty
    // and the next debounced `save_config` would round-trip an empty list
    // back to the backend, wiping the cache and breaking the next
    // `spawn_pane` call.
    {
        let snap = state.config.snapshot();
        if snap.shells.is_empty() {
            let detected = shell::detect_shells();
            state.config.update(|c| c.shells = detected);
            let _ = state.config.flush_if_dirty();
        } else {
            // Detection writes the zsh shim / bash rcfiles; with a cached
            // list it never runs, so rewrite them here or a fixed shim
            // would never reach an existing install.
            shell::refresh_shell_integration();
        }
    }
    // Pin every terminal pane's "" shell sentinel to the current default, so
    // a pane keeps the shell it first opened with instead of re-resolving
    // (e.g. zsh coming back as bash after `default_shell` changes). Probe a
    // copy first: `update` marks the store dirty unconditionally.
    if state.config.snapshot().pin_pane_shells() {
        state.config.update(|c| c.pin_pane_shells());
        let _ = state.config.flush_if_dirty();
    }
    let config = state.config.snapshot();
    let shells = config.shells.clone();
    Ok(BootstrapPayload {
        config,
        shells,
        config_path: state.config.path().display().to_string(),
    })
}

/// `async`: shell detection runs probe processes and stats many paths.
#[tauri::command(async)]
pub fn detect_shells_cmd(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
) -> YmuxResult<Vec<ShellProfile>> {
    guard_local(&webview, &request, "detect_shells_cmd")?;
    let detected = shell::detect_shells();
    state.config.update(|c| c.shells = detected.clone());
    let _ = state.config.flush_if_dirty();
    Ok(detected)
}

/// Which agent CLIs are installed, for the top bar's "+" launcher. Only
/// detects: the frontend types the command into a pane it created itself, so
/// this cannot start anything. Off the main thread — on macOS/Linux the first
/// call (and every `refresh`, the launcher's "Rescan") runs the user's login
/// shell to learn its `PATH`, then it stats a few dozen files.
#[tauri::command]
pub async fn detect_agents(
    webview: Webview,
    request: Request<'_>,
    refresh: Option<bool>,
) -> YmuxResult<Vec<crate::agent_launch::DetectedAgent>> {
    guard_local(&webview, &request, "detect_agents")?;
    let refresh = refresh.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || crate::agent_launch::detect_agents(refresh))
        .await
        .map_err(|e| YmuxError::Other(format!("detect_agents: {e}")))
}

#[tauri::command]
pub fn save_config(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    state: State<'_, AppState>,
    config: Config,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "save_config")?;
    // Treat the frontend as the source of truth for layouts and the active
    // workspace, but keep `shells` as a backend-owned detection cache. If the
    // frontend ships a non-empty shell list we accept it (e.g. after a
    // re-detect); otherwise we preserve whatever is already cached so a stale
    // frontend snapshot can't blow away the list and break subsequent
    // `spawn_pane` calls.
    //
    // Before persisting, patch each pane's `cwd` with whatever the per-pane
    // OSC 7 reader last observed. This is how "reopen in the last working
    // directory" works: as the user `cd`s around, the shell's prompt emits
    // `ESC ] 7 ; file://.../current/dir ESC \`, the reader thread drops that
    // into `PtyManager.cwds`, and we snapshot it here so the saved layout
    // tree carries the live directory instead of the stale initial one.
    let mut incoming = config;
    incoming.patch_cwds(&state.pty.cwds_snapshot());
    let accounts_before = state.config.snapshot().accounts;
    state.config.update(|c| c.merge_layouts_from(incoming));
    state.config.flush()?;
    if let Some(rt) = app.try_state::<crate::activity::runtime::ActivityRuntime>() {
        rt.apply_config(&state.config.snapshot().activity);
    }
    if let Some(wh) = app.try_state::<crate::webhooks::runtime::WebhookRuntime>() {
        wh.sync_destinations(&state.config.snapshot().webhooks);
    }
    // A changed account list may add or drop Claude config dirs that carry
    // hooks. Separate thread: the dirs can be on a slow network share.
    if state.config.snapshot().accounts != accounts_before {
        std::thread::spawn(move || {
            reconcile_hooks_quietly(&app, HookTrigger::AccountsChanged, None)
        });
    }
    Ok(())
}

/// What a pane's account changes in its child's environment: the variables to
/// add (after the pane's own `explicit` env, which wins) and the inherited
/// ones to scrub ("none" is a hard opt-out). Shared by `spawn_pane` and
/// `get_agent_session`, so the resume check sees the directories the spawn
/// will really use.
fn pane_env(
    config: &Config,
    explicit: &[(String, String)],
    account: &str,
    workspace_account: &str,
) -> (Vec<(String, String)>, Vec<&'static str>) {
    (
        crate::accounts::account_env(
            &config.accounts,
            account,
            workspace_account,
            dirs::home_dir().as_deref(),
            explicit,
        ),
        crate::accounts::account_env_removals(account, workspace_account, explicit),
    )
}

/// `async` (runs on a worker thread, not the main one): the spawn stats the
/// saved cwd with `is_dir`, which on a dead network/UNC path hangs for tens
/// of seconds, and starting the child itself can be slow.
#[tauri::command(async)]
pub fn spawn_pane(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    state: State<'_, AppState>,
    args: SpawnArgs,
) -> YmuxResult<SpawnedPane> {
    guard_local(&webview, &request, "spawn_pane")?;
    let config = state.config.snapshot();
    let profile = config
        .shell(&args.shell)
        .ok_or_else(|| YmuxError::UnknownShell(args.shell.clone()))?
        .clone();

    // Account-selected CLAUDE_CONFIG_DIR / CODEX_HOME, appended after the
    // pane's own env (which wins, see `accounts::account_env`), and the
    // inherited variables an opt-out ("none") scrubs.
    let (account_env, remove_env) =
        pane_env(&config, &args.env, &args.account, &args.workspace_account);
    // The account dirs to create and the Claude dir the child will use (to
    // hook it once it exists). Acted on only after the spawn succeeded: a
    // failed spawn must not create directories or rewrite hook files.
    let account_dirs = (!account_env.is_empty()).then(|| {
        let claude_dir = crate::accounts::agent_dirs(
            &remove_env,
            &[&profile.env, &args.env, &account_env],
            |k| std::env::var(k).ok(),
            dirs::home_dir().as_deref(),
        )
        .claude;
        (account_env.clone(), claude_dir)
    });
    let mut env = args.env;
    env.extend(account_env);

    let spec = crate::config::model::PaneSpec {
        id: args.id,
        title: None,
        shell: profile.name.clone(),
        cwd: args.cwd,
        startup_cmd: None,
        env,
        pane_kind: crate::config::model::PaneKind::Terminal,
        url: None,
        hotkeys: Vec::new(),
        bg_color: String::new(),
        worktree_path: String::new(),
        file_path: String::new(),
        account: args.account,
    };

    let spawned = state.pty.spawn(
        &spec,
        &profile,
        PtySize {
            rows: args.rows.max(1),
            cols: args.cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        },
        &remove_env,
    )?;
    // Ids always start OFF: a `set_auto_continue(id, true)` that raced past
    // `kill_pane` must not leave a respawned pane auto-answering with no badge.
    state.auto_continue.set(args.id, false);
    // Off this thread: a dead UNC path would otherwise stall the command.
    // Failures are only logged, and the tool creates its own dir on first
    // use anyway.
    if let Some((dirs, claude_dir)) = account_dirs {
        std::thread::spawn(move || {
            crate::accounts::ensure_dirs(&dirs);
            reconcile_hooks_quietly(&app, HookTrigger::SpawnPane, Some(&claude_dir));
        });
    }
    Ok(spawned)
}

/// Stays a main-thread (sync) command on purpose: commands run in arrival
/// order there, while `async` ones run concurrently and could reorder two
/// keystrokes or paste chunks. It cannot block: the bytes are only queued,
/// and the pane's writer thread does the (possibly blocking) PTY write
/// (`PtySession::write`).
#[tauri::command]
pub fn write_pane(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    args: WriteArgs,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "write_pane")?;
    state.pty.write(args.id, &args.data)
}

/// Sync for the same ordering reason as [`write_pane`]: two resizes swapped
/// by concurrent async dispatch would leave the PTY at a stale size. It is a
/// window-size ioctl (`TIOCSWINSZ` / `ResizePseudoConsole`) that does not
/// wait on the child, and it no longer holds the session map lock.
#[tauri::command]
pub fn resize_pane(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    args: ResizeArgs,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "resize_pane")?;
    state.pty.resize(
        args.id,
        PtySize {
            rows: args.rows.max(1),
            cols: args.cols.max(1),
            pixel_width: args.pixel_width,
            pixel_height: args.pixel_height,
        },
    )
}

#[tauri::command]
pub fn kill_pane(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "kill_pane")?;
    state.auto_continue.set(id, false);
    if let Some(rt) = app.try_state::<crate::activity::runtime::ActivityRuntime>() {
        rt.forget_pane(id);
    }
    state.pty.kill(id)
}

#[tauri::command]
pub fn set_active_workspace(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    id: u32,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "set_active_workspace")?;
    state.config.update(|c| c.active_workspace = id);
    let _ = state.config.flush_if_dirty();
    Ok(())
}

/// Return the most recently reported working directory for a pane, or `None`
/// if the pane has not yet emitted an OSC 7 sequence.
#[tauri::command]
pub fn get_pane_cwd(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    id: Uuid,
) -> YmuxResult<Option<String>> {
    guard_local(&webview, &request, "get_pane_cwd")?;
    Ok(state.pty.cwd_for(id))
}

/// Turn auto-continue (answering Claude Code's `AskUserQuestion` from its
/// hook, CLAUDE.md rule 13) on or off for one pane. Runtime only.
#[tauri::command]
pub fn set_auto_continue(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    id: Uuid,
    enabled: bool,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "set_auto_continue")?;
    if enabled && !state.pty.has(id) {
        return Err(YmuxError::UnknownPane(id));
    }
    state.auto_continue.set(id, enabled);
    Ok(())
}

/// Tauri event after each auto-continue decision (see [`AutoAnswerEvent`]).
pub const AUTO_ANSWER_EVENT: &str = "auto-continue:answered";

/// One auto-continue decision, for the pane's log.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoAnswerEvent {
    pub pane_id: Uuid,
    /// No option was recommended: the question was denied back to the model.
    pub denied: bool,
    /// `(question, answer)` pairs; empty when `denied`.
    pub answers: Vec<(String, String)>,
}

impl AutoAnswerEvent {
    pub fn new(pane_id: Uuid, decision: &crate::auto_continue::Decision) -> Self {
        use crate::auto_continue::{clip_for_log, Decision};
        match decision {
            Decision::Answer { answers, .. } => Self {
                pane_id,
                denied: false,
                answers: answers
                    .iter()
                    .map(|(q, a)| (clip_for_log(q), clip_for_log(a)))
                    .collect(),
            },
            Decision::Deny | Decision::DenyDuplicate => Self {
                pane_id,
                denied: true,
                answers: Vec::new(),
            },
        }
    }
}

/// Tauri event carrying the full agent snapshot after every registry change.
pub const AGENTS_CHANGED_EVENT: &str = "agents:changed";
/// Emitted after every process scan (changed or not); the payload is the
/// epoch-ms time the process list was sampled.
pub const AGENTS_SCANNED_EVENT: &str = "agents:scanned";

/// Callers hold the [`SharedAgents`] lock across this call so snapshots reach
/// the frontend in the order they were taken.
pub fn emit_agents_changed(app: &AppHandle, snapshot: &AgentSnapshot) {
    if let Err(e) = app.emit(AGENTS_CHANGED_EVENT, snapshot) {
        tracing::warn!(error = %e, "emit agents:changed failed");
    }
}

/// Route one Claude Code hook event (from the `hook_http` receiver) into
/// the agent registry. Events for panes this ymux doesn't own — one that has
/// closed since the receiver checked — are dropped.
pub fn apply_agent_hook(app: &AppHandle, ev: HookEvent) {
    if !app.state::<AppState>().pty.has(ev.pane_id) {
        return;
    }
    let agents = app.state::<SharedAgents>();
    let mut reg = agents.0.lock();
    let changed = reg.apply_hook(&ev);
    // Record the id the hook just carried, then drop the registry lock before
    // taking the session lock — the scan thread takes them in this order too,
    // so there is one lock order and no way to deadlock against it.
    let hook_id = reg.hook_session_id(ev.pane_id).map(str::to_string);
    let lead = reg.snapshot().get(&ev.pane_id).and_then(|p| p.lead.clone());
    if changed {
        // Emit while still holding the lock: the hook listener and the scan
        // thread both write the registry, and emitting after release let a
        // newer snapshot overtake an older one, leaving the UI stale. Safe —
        // nothing in Rust listens for this event, so emit never re-enters
        // the registry, and it only queues the JS dispatch (non-blocking).
        emit_agents_changed(app, &reg.snapshot());
    }
    drop(reg);
    // `SessionEnd` clears the pane's agents, so there is no lead to read here
    // and nothing to record — which is right: a hook cannot distinguish "the
    // user quit Claude" from "ymux is killing every PTY on the way out", and
    // deactivating on the second would erase exactly the records the next
    // launch needs. The process scan's `exited_panes` makes that distinction
    // (it only reports panes still in `live`), so the decision belongs there.
    let Some(lead) = lead else { return };
    let sessions = app.state::<SharedSessions>();
    let mut tracker = sessions.0.lock();
    observe_pane_session(
        app,
        &mut tracker,
        PaneSessionInput {
            pane_id: ev.pane_id,
            kind: &lead.kind,
            status: Some(lead.status),
            hook_session_id: hook_id,
            process: None,
            pid_file_session_id: None,
            others: &[],
        },
    );
    flush_sessions(&mut tracker);
}

/// The resume plan for one pane, or `None` when it should start normally.
///
/// Called by `TerminalPane.spawn()` *before* it decides whether to replay
/// scrollback (spec §4/§5): for a pane that is about to resume an agent, the
/// saved backlog is dead history that would sit above the resumed
/// conversation, so it is skipped entirely.
///
/// `startup_cmd` is the pane's own saved startup command, so the flags it
/// provably carries over are kept (and any selector dropped). `shell` is the
/// pane's shell profile name: the command is typed into that shell, so it is
/// quoted by that shell's rules (`agent_sessions::ShellFamily`).
///
/// `env`, `account` and `workspace_account` are what `spawn_pane` will get:
/// the pane's agents will use the directories they imply, and a record from
/// another directory (another account) resumes nothing.
///
/// `async` so it runs off the main thread: the session lock is shared with
/// the scan thread, and the transcript check touches the disk. The lock is
/// never held across that check.
#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
pub fn get_agent_session(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    sessions: State<'_, SharedSessions>,
    pane_id: Uuid,
    startup_cmd: Option<String>,
    shell: Option<String>,
    env: Option<Vec<(String, String)>>,
    account: Option<String>,
    workspace_account: Option<String>,
) -> YmuxResult<ResumeOutcome> {
    guard_local(&webview, &request, "get_agent_session")?;
    let config = state.config.snapshot();
    let profile = shell.as_deref().and_then(|name| config.shell(name));
    let family = profile
        .map(|p| crate::agent_sessions::ShellFamily::from_executable(&p.executable))
        .unwrap_or(crate::agent_sessions::ShellFamily::Unknown);
    // The directories the pane agents will use once spawned, from the same
    // layers `PtyManager::spawn` records (minus its manager-level extras,
    // which are `YMUX_*` only).
    let explicit = env.unwrap_or_default();
    let (account_env, remove_env) = pane_env(
        &config,
        &explicit,
        account.as_deref().unwrap_or_default(),
        workspace_account.as_deref().unwrap_or_default(),
    );
    let home = dirs::home_dir();
    let pane_dirs = crate::accounts::agent_dirs(
        &remove_env,
        &[
            profile.map(|p| p.env.as_slice()).unwrap_or_default(),
            &explicit,
            &account_env,
        ],
        |k| std::env::var(k).ok(),
        home.as_deref(),
    );
    let home_dirs = crate::accounts::home_dirs(home.as_deref());
    let now = crate::agent_sessions::now_secs();
    // Copy the record out; the disk check below runs without the lock.
    let record = sessions.0.lock().get(pane_id).cloned();
    let outcome = crate::agent_sessions::outcome_for_pane(
        record.as_ref(),
        &pane_dirs,
        &home_dirs,
        startup_cmd.as_deref().unwrap_or_default(),
        family,
        now,
        crate::agent_sessions::transcript_exists,
    );
    // A resume: the frontend types the command next, and until the scan sees
    // the resumed agent running the pane's old scrollback stays on disk. A
    // transcript that is gone: the record is declined now.
    let owned = record.as_ref().map_or(true, |s| {
        crate::agent_sessions::record_matches_pane(s, &pane_dirs, &home_dirs)
    });
    let mut tracker = sessions.0.lock();
    tracker.note_outcome(pane_id, &outcome, now);
    // A record of another account is not resumed, so it must not delete the
    // scrollback this spawn restores either.
    tracker.note_record_owner(pane_id, owned);
    Ok(outcome)
}

/// Forget a pane's session entirely. Called when the user closes a pane for
/// good, mirroring `delete_scrollback`.
/// `async`: waits on the session lock the scan thread shares, then writes
/// the store to disk.
#[tauri::command(async)]
pub fn clear_agent_session(
    webview: Webview,
    request: Request<'_>,
    sessions: State<'_, SharedSessions>,
    pane_id: Uuid,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "clear_agent_session")?;
    let mut tracker = sessions.0.lock();
    tracker.forget(pane_id);
    flush_sessions(&mut tracker);
    Ok(())
}

/// Persist the session store if anything changed. A no-op on an idle tick, so
/// the 2 s scan does not rewrite the file forever.
pub fn flush_sessions(tracker: &mut crate::agent_sessions::SessionTracker) {
    if tracker.take_dirty() {
        if let Err(e) = crate::agent_sessions::save(tracker.store()) {
            tracing::warn!(error = %e, "saving agent sessions failed");
        }
    }
}

/// What one caller knows about a pane's agent, for [`observe_pane_session`].
pub struct PaneSessionInput<'a> {
    pub pane_id: Uuid,
    pub kind: &'a str,
    pub status: Option<crate::agents::AgentStatus>,
    pub hook_session_id: Option<String>,
    /// The agent process the scan found; `None` from the hook listener.
    pub process: Option<crate::agent_binding::PaneProcess>,
    /// What Claude's pid file says that process is running, already vetted.
    pub pid_file_session_id: Option<String>,
    /// Every other agent process on the machine (empty from the hook
    /// listener, which never guesses).
    pub others: &'a [crate::agent_binding::OtherAgent],
}

/// Feed one pane's current state into the session tracker.
///
/// Shared by the hook listener and the process scan so both write the same
/// store. The transcript lookup is only reached while the pane's agent process
/// is not yet tied to a conversation, and it is skipped entirely for a pane
/// with no known cwd — without one there is nothing to match against.
pub fn observe_pane_session(
    app: &AppHandle,
    tracker: &mut crate::agent_sessions::SessionTracker,
    input: PaneSessionInput<'_>,
) {
    let state = app.state::<AppState>();
    let pty = &state.pty;
    // The pane's own dirs; a pane ymux no longer tracks reads the home ones.
    let dirs = pty
        .agent_dirs(input.pane_id)
        .unwrap_or_else(|| crate::accounts::home_dirs(dirs::home_dir().as_deref()));
    let config_dir = crate::agent_sessions::AgentKind::from_kind(input.kind)
        .map(|a| dirs.for_agent(a).to_string())
        .unwrap_or_default();
    let obs = crate::agent_sessions::PaneObservation {
        pane_id: input.pane_id,
        kind: input.kind.to_string(),
        config_dir: config_dir.clone(),
        cwd: pty.cwd_for(input.pane_id),
        status: input.status,
        hook_session_id: input.hook_session_id,
        process: input.process,
        pid_file_session_id: input.pid_file_session_id,
    };
    tracker.observe(
        &obs,
        crate::agent_sessions::now_secs(),
        input.others,
        |agent, cwd| crate::agent_scan_disk::candidate_sessions_in(agent, &config_dir, cwd),
    );
}

/// Current agent snapshot, for the frontend's initial render.
#[tauri::command]
pub fn get_agents(
    webview: Webview,
    request: Request<'_>,
    agents: State<'_, SharedAgents>,
) -> YmuxResult<AgentSnapshot> {
    guard_local(&webview, &request, "get_agents")?;
    Ok(agents.0.lock().snapshot())
}

/// Tauri event carrying `pane id -> running-program label` (tab labels).
const PANE_LABELS_EVENT: &str = "panes:labels";

pub fn emit_pane_labels(app: &AppHandle, labels: &std::collections::HashMap<Uuid, String>) {
    if let Err(e) = app.emit(PANE_LABELS_EVENT, labels) {
        tracing::warn!(error = %e, "emit panes:labels failed");
    }
}

/// Latest tab labels, for a frontend that has just mounted.
#[tauri::command]
pub fn get_pane_labels(
    webview: Webview,
    request: Request<'_>,
    labels: State<'_, crate::agent_scan::SharedLabels>,
) -> YmuxResult<std::collections::HashMap<Uuid, String>> {
    guard_local(&webview, &request, "get_pane_labels")?;
    Ok(labels.0.lock().clone())
}

/// Install (`true`) or remove (`false`) ymux's Claude Code hooks in every
/// account's config dir, then persist the setting. The default and home dirs
/// are written first: if that fails (e.g. unparseable settings.json) the error
/// reaches the UI and the setting is not flipped. A failure in any other
/// account's dir does not block it; those come back as issues. Off the main
/// thread: a profile dir on a dead network path would stall the UI.
#[tauri::command(async)]
pub fn set_agent_tracking(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    enabled: bool,
) -> YmuxResult<Vec<HookDirIssue>> {
    guard_local(&webview, &request, "set_agent_tracking")?;
    let trigger = if enabled {
        HookTrigger::Enable
    } else {
        HookTrigger::Disable
    };
    reconcile_hooks(&app, trigger)
}

/// Port the Claude Code hook receiver listens on this run (`None` if it
/// could not start), and whether this run may rewrite hook files on its own
/// (`agent_hooks::startup_refresh_allowed`). Managed state, read by
/// [`reconcile_hooks`].
pub struct AgentHookPort {
    pub port: Option<u16>,
    pub writes_allowed: bool,
}

/// What asked for a hook reconcile (CLAUDE.md rules 11-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTrigger {
    /// The user turned tracking on: needs a port, installs everywhere.
    Enable,
    /// The user turned tracking off: removes from every dir ymux knows of.
    Disable,
    /// App launch: reinstalls (the port may have changed) or clears pending.
    Startup,
    /// The account list changed.
    AccountsChanged,
    /// A pane spawned into an account dir that has no hooks yet.
    SpawnPane,
}

/// Serializes every hook-file write and the config fields that record them.
pub(crate) static HOOK_WRITES: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Bring the hooks in line with the account list under one lock, reading the
/// latest config snapshot only after taking it. `Enable`/`Disable` flip
/// `agent_tracking` and write `agent_hook_dirs` inside that lock, so an
/// implicit reconcile can never interleave with the toggle. Implicit triggers
/// do nothing in debug builds and in a stand-down instance (rule 13).
pub fn reconcile_hooks(app: &AppHandle, trigger: HookTrigger) -> YmuxResult<Vec<HookDirIssue>> {
    use crate::agent_hooks::{
        apply_plan, desired_dirs, next_installed, plan_hooks, plan_hooks_with, same_location,
    };
    let _guard = HOOK_WRITES.lock();
    let state = app.state::<AppState>();
    let Some(hook) = app.try_state::<AgentHookPort>() else {
        return Err(YmuxError::Other("hook receiver state is not ready".into()));
    };
    let explicit = matches!(trigger, HookTrigger::Enable | HookTrigger::Disable);
    if !explicit && !hook.writes_allowed {
        return Ok(Vec::new());
    }
    let config = state.config.snapshot();
    let desired = desired_dirs(
        &config.accounts,
        |k| std::env::var(k).ok(),
        dirs::home_dir().as_deref(),
    );
    let installed = config.agent_hook_dirs.clone();
    let (tracking, plan, port) = match trigger {
        HookTrigger::Enable => {
            let port = hook.port.ok_or_else(|| {
                YmuxError::Other(
                    "this ymux has no Claude Code hook receiver (another ymux instance \
                     holds its port, or it failed to start)"
                        .into(),
                )
            })?;
            (
                true,
                plan_hooks_with(&desired.all, &installed, true, same_location),
                port,
            )
        }
        HookTrigger::Disable => {
            let known: Vec<String> = installed.iter().chain(&desired.all).cloned().collect();
            (false, plan_hooks(&[], &known, false), 0)
        }
        _ if config.agent_tracking => {
            let Some(port) = hook.port else {
                return Ok(Vec::new());
            };
            let refresh_all = trigger == HookTrigger::Startup;
            (
                true,
                plan_hooks_with(&desired.all, &installed, refresh_all, same_location),
                port,
            )
        }
        // Tracking off: only hooks a failed uninstall left behind.
        _ => (false, plan_hooks(&[], &installed, false), 0),
    };
    if plan.is_empty() && !explicit {
        return Ok(Vec::new());
    }
    let may_create = |d: &str| desired.is_required(d);
    let outcome = if trigger == HookTrigger::Enable {
        // Required dirs (default, home) first: if one fails, no profile dir
        // may keep live hooks while tracking stays off.
        let (req, rest): (Vec<String>, Vec<String>) = plan
            .install
            .iter()
            .cloned()
            .partition(|d| desired.is_required(d));
        let first = apply_plan(
            &crate::agent_hooks::HookPlan {
                uninstall: Vec::new(),
                install: req,
            },
            port,
            may_create,
        );
        if let Some(i) = first
            .issues()
            .into_iter()
            .find(|i| desired.is_required(&i.dir))
        {
            // Record only what the first pass really wrote, so a later
            // disable still finds it.
            let next = next_installed(&installed, &first);
            state.config.update(|c| c.agent_hook_dirs = next);
            let _ = state.config.flush();
            return Err(YmuxError::Other(format!("{}: {}", i.dir, i.error)));
        }
        let mut second = apply_plan(
            &crate::agent_hooks::HookPlan {
                uninstall: plan.uninstall.clone(),
                install: rest,
            },
            port,
            may_create,
        );
        second.install.splice(0..0, first.install);
        second
    } else {
        apply_plan(&plan, port, may_create)
    };
    let next = next_installed(&installed, &outcome);
    let issues = outcome.issues();
    // A failure in the default or home dir blocks the toggle; other accounts'
    // dirs only report (and, for a removal, stay recorded as pending).
    let blocking = if explicit {
        issues.iter().find(|i| desired.is_required(&i.dir)).cloned()
    } else {
        None
    };
    state.config.update(|c| {
        if explicit && blocking.is_none() {
            c.agent_tracking = tracking;
        }
        c.agent_hook_dirs = next;
    });
    let flushed = state.config.flush();
    if let Some(b) = blocking {
        return Err(YmuxError::Other(format!("{}: {}", b.dir, b.error)));
    }
    flushed?;
    Ok(issues)
}

/// [`reconcile_hooks`] for the implicit triggers: runs inline (callers are
/// already on a worker thread), skips when `pane_claude_dir` already carries
/// hooks or is not an account dir, and logs instead of returning.
fn reconcile_hooks_quietly(app: &AppHandle, trigger: HookTrigger, pane_claude_dir: Option<&str>) {
    if let Some(dir) = pane_claude_dir {
        let config = app.state::<AppState>().config.snapshot();
        let desired = crate::agent_hooks::desired_dirs(
            &config.accounts,
            |k| std::env::var(k).ok(),
            dirs::home_dir().as_deref(),
        );
        if !config.agent_tracking
            || crate::agent_hooks::has_dir(&config.agent_hook_dirs, dir)
            || !crate::agent_hooks::has_dir(&desired.all, dir)
        {
            return;
        }
    }
    match reconcile_hooks(app, trigger) {
        Ok(issues) => {
            for i in issues {
                tracing::warn!(dir = %i.dir, error = %i.error, "Claude Code hooks: directory not updated");
            }
        }
        Err(e) => tracing::warn!(error = %e, "failed to reconcile Claude Code hooks"),
    }
}

/// App launch: re-run the hook install for every account dir (or clear
/// leftovers when tracking is off). Off the setup thread — a profile dir on a
/// network share must not delay the window.
pub fn reconcile_hooks_at_startup(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || reconcile_hooks_quietly(&app, HookTrigger::Startup, None));
}

/// Start the loopback receiver for Claude Code's http hooks
/// (`crate::hook_http`, CLAUDE.md rule 13) and return its port.
///
/// The port is persisted in `Config::agent_hook_port` and reused, because the
/// hooks in `~/.claude/settings.json` carry it literally. When it is taken:
/// if another ymux answers the ping there, this instance runs **without** a
/// receiver (`None`) and never touches the port or the hooks — moving them
/// would strand that instance's panes. (Release builds are single-instance,
/// so that other ymux is a `tauri dev` build or a startup race; see
/// `main.rs`.) Otherwise an OS-assigned port is used
/// and — if `may_persist` (see `agent_hooks::startup_refresh_allowed`) —
/// saved, so the startup hook refresh that follows points the hooks at it.
/// Accepted events are applied after the receiver has already answered, so
/// Claude never waits on a lock.
pub fn start_hook_receiver(app: &AppHandle, token: String, may_persist: bool) -> Option<u16> {
    use crate::hook_http;
    let state = app.state::<AppState>();
    let persisted = state.config.snapshot().agent_hook_port;
    let bound = hook_http::choose_port(persisted, hook_http::bind_loopback, hook_http::probe_ymux);
    let (listener, reused) = match bound {
        Ok(hook_http::Bound::Listening { listener, reused }) => (listener, reused),
        Ok(hook_http::Bound::HeldByYmux) => {
            tracing::warn!(
                port = persisted,
                "another ymux is receiving Claude Code hooks on this port; this instance \
                 runs without a receiver, so its panes get no hook events (the process \
                 scan still lists their agents)"
            );
            return None;
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to bind the Claude Code hook receiver");
            return None;
        }
    };
    let port = listener.local_addr().ok()?.port();
    if !reused && may_persist {
        state.config.update(|c| c.agent_hook_port = port);
        if let Err(e) = state.config.flush() {
            tracing::warn!(error = %e, "failed to persist the hook receiver port");
        }
    }
    let known = app.clone();
    let apply = app.clone();
    let auto_on = app.clone();
    let auto_log = app.clone();
    // Created in `setup` before this runs; without it capture is simply off.
    let tap = app
        .try_state::<crate::activity::runtime::ActivityRuntime>()
        .map(|rt| rt.tap.clone())
        .unwrap_or_else(hook_http::ActivityTap::off);
    let served = hook_http::serve(
        listener,
        token,
        std::sync::Arc::new(move |pane| known.state::<AppState>().pty.has(pane)),
        std::sync::Arc::new(move |ev| apply_agent_hook(&apply, ev)),
        hook_http::AutoContinue {
            enabled: std::sync::Arc::new(move |pane| {
                auto_on.state::<AppState>().auto_continue.contains(pane)
            }),
            on_answer: std::sync::Arc::new(move |pane, decision| {
                let ev = AutoAnswerEvent::new(pane, decision);
                if let Err(e) = auto_log.emit(AUTO_ANSWER_EVENT, &ev) {
                    tracing::warn!(error = %e, "emit auto-continue:answered failed");
                }
            }),
        },
        tap,
    );
    if let Err(e) = served {
        tracing::error!(error = %e, "failed to start the Claude Code hook receiver");
        return None;
    }
    tracing::info!(
        port,
        reused,
        "Claude Code hook receiver listening on 127.0.0.1"
    );
    Some(port)
}

/// Open a URL in the system default browser. Only `http://` and `https://`
/// URLs are accepted; anything else is rejected to prevent accidental
/// execution of arbitrary shell commands via `start` or `xdg-open`.
/// `async`: `opener` may wait for the launcher process (`xdg-open`, `open`).
#[tauri::command(async)]
pub fn open_url(webview: Webview, request: Request<'_>, url: String) -> YmuxResult<()> {
    guard_local(&webview, &request, "open_url")?;
    let url = browsable_url(&url)?;
    // Never through `cmd /C start`: cmd re-parses the URL, and an `&`, `|`
    // or `^` in a space-free URL (which Rust does not quote) runs as a
    // command — a crafted link in a rendered README would be RCE. `opener`
    // hands it to ShellExecuteW on Windows and `open`/`xdg-open` elsewhere,
    // as one argument, with no shell in between.
    opener::open_browser(&url).map_err(|e| YmuxError::Other(format!("open_url: {e}")))?;
    Ok(())
}

/// The http(s) URL `open_url` may hand to the OS, re-serialised by the URL
/// parser so what reaches the launcher is a canonical URL, not the raw
/// string a page supplied.
fn browsable_url(raw: &str) -> YmuxResult<String> {
    let parsed =
        url::Url::parse(raw.trim()).map_err(|e| YmuxError::Other(format!("open_url: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(YmuxError::Other(
            "open_url: only http/https URLs are supported".into(),
        ));
    }
    Ok(parsed.into())
}

#[cfg(test)]
mod open_url_tests {
    use super::browsable_url;

    #[test]
    fn only_http_and_https_pass() {
        assert!(browsable_url("https://example.com/a?b=1").is_ok());
        assert!(browsable_url("http://example.com").is_ok());
        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "mailto:a@b.c",
            "calc",
            "",
        ] {
            assert!(browsable_url(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn shell_metacharacters_are_just_url_text() {
        // Passed as one argument to ShellExecuteW / open, never to cmd.
        let u = browsable_url("https://example.com/&calc").unwrap();
        assert!(u.starts_with("https://example.com/"));
    }
}

/// Resolve terminal-output path candidates against a pane's live cwd,
/// reporting which of them actually exist. Backs the terminal's path
/// linkifier: a candidate only becomes a clickable link if this says it is
/// real, and the answer is what the tooltip shows.
///
/// `async` on purpose. Tauri runs a non-async command on the main thread,
/// and `metadata` on a mapped network drive that has gone away blocks for
/// tens of seconds — long enough to freeze the whole UI on a mouse move.
/// The work goes to a blocking pool and carries its own timeout
/// ([`fspath::PROBE_TIMEOUT`]).
///
/// All the interesting logic — validation, tilde expansion, the UNC policy
/// that stops a hover from leaking SMB credentials — lives in
/// [`crate::fspath`], where it is unit-tested.
#[tauri::command]
pub async fn resolve_paths(
    webview: Webview,
    request: Request<'_>,
    paths: Vec<String>,
    cwd: Option<String>,
) -> YmuxResult<Vec<Option<crate::fspath::ResolvedPath>>> {
    guard_local(&webview, &request, "resolve_paths")?;
    // An `Err` from the probe ("busy" / "timed out") is surfaced as an IPC
    // rejection, which `PathProbeCache` treats as "no answer" and does not
    // cache — unlike a `None`, which means "does not exist".
    tauri::async_runtime::spawn_blocking(move || crate::fspath::probe_batch(paths, cwd))
        .await
        .map_err(|e| YmuxError::Other(format!("resolve_paths: {e}")))?
        .map_err(|e| YmuxError::Other(format!("resolve_paths: {e}")))
}

/// Open an absolute path with the OS default handler — `ShellExecuteW` on
/// Windows, `open` on macOS, both via `opener`. Neither builds a command
/// line, so `&`, `^`, `%` and quotes in a filename are inert.
///
/// A directory opens in the file manager. Only an allowlist of document
/// types is opened with its program; everything else is *revealed* in the
/// file manager rather than launched: clicking a path in terminal output
/// means "show me this", and terminal output is attacker-influenced, so
/// `ShellExecuteW` running `evil.bat` is not an acceptable reading of the
/// click. See [`crate::fspath::should_reveal`].
#[tauri::command]
pub async fn open_path(webview: Webview, request: Request<'_>, path: String) -> YmuxResult<()> {
    guard_local(&webview, &request, "open_path")?;
    // The path was vetted by `resolve_paths`, but it made a round trip
    // through the frontend to get here, so it is validated again from
    // scratch rather than trusted.
    crate::fspath::validate_open(&path).map_err(YmuxError::Other)?;
    tauri::async_runtime::spawn_blocking(move || {
        // Same gate as the hover probe: no share, no device, and every
        // symlink/junction classified before it is followed. A terminal link
        // never names a share (the probe refuses them), so this refuses
        // nothing a link could produce — it keeps a replayed or stale path
        // from reaching SMB through a `metadata` call here.
        let resolved = crate::fspath::resolve_local(Path::new(&path))
            .map_err(|e| YmuxError::Other(format!("open_path: {path}: {e}")))?;
        // A junction's target comes back verbatim (`\\?\C:\…`), which
        // `ShellExecuteW` does not reliably accept.
        let resolved = crate::fspath::strip_verbatim(&resolved);
        let p = resolved.as_path();
        let meta = std::fs::symlink_metadata(p)
            .map_err(|e| YmuxError::Other(format!("open_path: {path}: {e}")))?;
        // Judged on the resolved name, so a `notes.md` symlink to `evil.bat`
        // is judged as `evil.bat`. An allowlist: anything not a known
        // document type is revealed.
        let reveal = crate::fspath::should_reveal(p, meta.is_dir(), crate::fspath::exec_bit(&meta));
        let result = if reveal {
            opener::reveal(p)
        } else {
            opener::open(p)
        };
        result.map_err(|e| YmuxError::Other(format!("open_path: {path}: {e}")))
    })
    .await
    .map_err(|e| YmuxError::Other(format!("open_path: {e}")))?
}

/// Show an OS desktop notification with the given title and body.
#[tauri::command]
pub fn notify(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    title: String,
    body: String,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "notify")?;
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
    Ok(())
}

/// Persist `blob` (serialized terminal scrollback) for `pane_id` to disk.
/// Thin wrapper — the actual fs logic lives in [`crate::scrollback`] so it
/// can be unit-tested without a running webview (and on Linux CI, where this
/// `desktop`-gated module doesn't even compile).
///
/// `async` so the file write, and the wait for the session lock the scan
/// thread shares, happen off the main thread.
#[tauri::command(async)]
pub fn save_scrollback(
    webview: Webview,
    request: Request<'_>,
    sessions: State<'_, SharedSessions>,
    pane_id: String,
    blob: String,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "save_scrollback")?;
    // The backend alone decides (spec §5). A pane whose agent is
    // mid-conversation neither restores nor saves — the condition is "has a
    // fresh record", not "was resumed at spawn": the *first* Claude session
    // in a pane is not resumed, and a blob it wrote would sit unread until
    // the record went stale and then be replayed, putting back exactly the
    // dead screen this feature removes. A pane whose resume is still
    // unconfirmed keeps its old blob untouched: if the resume fails, that
    // blob is what the next launch restores.
    use crate::agent_sessions::ScrollbackAction;
    let action = Uuid::parse_str(&pane_id).map_or(ScrollbackAction::Save, |id| {
        sessions
            .0
            .lock()
            .scrollback_action(id, crate::agent_sessions::now_secs())
    });
    match action {
        ScrollbackAction::Save => {
            crate::scrollback::save_blob(&pane_id, &blob).map_err(YmuxError::Io)
        }
        ScrollbackAction::Skip => Ok(()),
        ScrollbackAction::Delete => crate::scrollback::delete_blob(&pane_id).map_err(YmuxError::Io),
    }
}

/// Load the persisted scrollback for `pane_id`, or an empty string if none
/// has been saved yet.
/// `async`: reads a file that may be megabytes long.
#[tauri::command(async)]
pub fn load_scrollback(
    webview: Webview,
    request: Request<'_>,
    pane_id: String,
) -> YmuxResult<String> {
    guard_local(&webview, &request, "load_scrollback")?;
    crate::scrollback::load_blob(&pane_id).map_err(YmuxError::Io)
}

/// Delete the persisted scrollback for `pane_id`, if any.
#[tauri::command]
pub fn delete_scrollback(
    webview: Webview,
    request: Request<'_>,
    pane_id: String,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "delete_scrollback")?;
    crate::scrollback::delete_blob(&pane_id).map_err(YmuxError::Io)
}

/// Persist an editor pane's draft of unsaved content (spec §3.5). The blob
/// is opaque here; see [`crate::drafts`].
#[tauri::command]
pub fn save_editor_draft(
    webview: Webview,
    request: Request<'_>,
    pane_id: String,
    blob: String,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "save_editor_draft")?;
    // Its own kind, so the pane can tell the user the safety net is off for
    // this file rather than failing silently.
    if blob.len() > crate::drafts::MAX_DRAFT_BYTES {
        return Err(YmuxError::TooLarge(format!(
            "draft of {} bytes is over the {} byte cap",
            blob.len(),
            crate::drafts::MAX_DRAFT_BYTES
        )));
    }
    crate::drafts::save(&pane_id, &blob).map_err(YmuxError::Io)
}

/// An editor pane's draft, or an empty string if it has none.
/// `async`: file read off the main thread.
#[tauri::command(async)]
pub fn load_editor_draft(
    webview: Webview,
    request: Request<'_>,
    pane_id: String,
) -> YmuxResult<String> {
    guard_local(&webview, &request, "load_editor_draft")?;
    crate::drafts::load(&pane_id).map_err(YmuxError::Io)
}

/// The pane ids that have an editor draft on disk, for the startup sweep
/// that removes drafts whose pane no longer exists anywhere in the config.
/// `async`: directory listing off the main thread.
#[tauri::command(async)]
pub fn list_editor_drafts(webview: Webview, request: Request<'_>) -> YmuxResult<Vec<String>> {
    guard_local(&webview, &request, "list_editor_drafts")?;
    crate::drafts::list().map_err(YmuxError::Io)
}

/// Forget an editor pane's draft, if any.
#[tauri::command]
pub fn delete_editor_draft(
    webview: Webview,
    request: Request<'_>,
    pane_id: String,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "delete_editor_draft")?;
    crate::drafts::delete(&pane_id).map_err(YmuxError::Io)
}

/// If the system clipboard holds an image, save it to the paste-images dir as
/// a PNG (pruning images older than the configured retention window first) and
/// return its absolute path, so the frontend can type that path into the PTY.
/// `Ok(None)` means "no image on the clipboard" — the frontend falls back to
/// pasting text — while `Err` means an image was found but could not be saved,
/// which the frontend surfaces in the pane.
///
/// Deliberately synchronous: it runs on the main thread, which is where both
/// the Windows OLE clipboard and macOS's `NSPasteboard` want to be touched.
#[tauri::command]
pub fn paste_clipboard_image(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
) -> YmuxResult<Option<String>> {
    guard_local(&webview, &request, "paste_clipboard_image")?;
    let Some(png) = crate::clipboard_image::read_clipboard_png().map_err(YmuxError::Io)? else {
        return Ok(None);
    };
    let hours = state.config.snapshot().paste_image_retention_hours;
    let retention = std::time::Duration::from_secs(u64::from(hours) * 3600);
    let path = crate::paste_images::save(&png, retention).map_err(YmuxError::Io)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

// ---------------------------------------------------------------------------
// Git pane commands. Like every other command, each starts with
// `guard_local` (CLAUDE.md rule 16).
//
// All of them are `#[tauri::command(async)]`: the retired `ygit` TUI blocked its render
// thread on every git call, and a slow repository must not be able to do that
// to the window. That includes the worktree ones — `worktree remove` deletes
// a whole checkout, `node_modules/` and all.
// ---------------------------------------------------------------------------

/// Commits reachable from any ref, newest first, in the repository
/// containing `cwd`. An empty repository returns an empty list.
#[tauri::command(async)]
pub fn git_log(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    limit: u32,
    skip: u32,
) -> YmuxResult<Vec<git::CommitInfo>> {
    guard_local(&webview, &request, "git_log")?;
    git::log(Path::new(&cwd), limit, skip)
}

/// Local and remote branches, with the current one named.
#[tauri::command(async)]
pub fn git_branches(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
) -> YmuxResult<git::BranchList> {
    guard_local(&webview, &request, "git_branches")?;
    git::branches(Path::new(&cwd))
}

/// Check out `branch`. See [`crate::git::checkout`] for what that runs.
#[tauri::command(async)]
pub fn git_checkout(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    branch: String,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "git_checkout")?;
    git::checkout(Path::new(&cwd), &branch)
}

/// Make a local branch tracking the remote-tracking `remote_branch`
/// (`origin/x`) and check it out. See [`crate::git::checkout_track`].
#[tauri::command(async)]
pub fn git_checkout_track(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    remote_branch: String,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "git_checkout_track")?;
    git::checkout_track(Path::new(&cwd), &remote_branch)
}

/// The changes, ignored files (when asked) and stranded detached commits a
/// checkout or worktree removal would touch. See [`crate::git::WorkStatus`].
#[tauri::command(async)]
pub fn git_work_status(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    include_ignored: bool,
) -> YmuxResult<git::WorkStatus> {
    guard_local(&webview, &request, "git_work_status")?;
    git::work_status(Path::new(&cwd), include_ignored)
}

/// Top-level directory of the repository containing `cwd`.
#[tauri::command(async)]
pub fn git_repo_root(webview: Webview, request: Request<'_>, cwd: String) -> YmuxResult<String> {
    guard_local(&webview, &request, "git_repo_root")?;
    git::repo_root_checked(Path::new(&cwd))
}

/// Check whether `cwd` sits inside a git repository (main worktree or a
/// linked worktree). Thin wrapper over [`crate::git::is_git_repo`].
/// `async` like the other git commands: it stats `cwd`, which may be on a
/// dead network share.
#[tauri::command(async)]
pub fn git_is_repo(webview: Webview, request: Request<'_>, cwd: String) -> YmuxResult<bool> {
    guard_local(&webview, &request, "git_is_repo")?;
    Ok(git::is_git_repo(Path::new(&cwd)))
}

/// Create a new git worktree for `branch`, rooted at the repo containing
/// `cwd`, under a suggested sibling path derived from `base`. Returns the
/// created worktree's path.
#[tauri::command(async)]
pub fn git_worktree_add(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    branch: String,
    base: String,
) -> YmuxResult<String> {
    guard_local(&webview, &request, "git_worktree_add")?;
    let repo = git::repo_root(Path::new(&cwd))?;
    let path = git::suggested_worktree_path(&repo, &branch, &base);
    git::worktree_add(&repo, &branch, &path)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Create a fan-out lane: a worktree on the new branch `branch` started at
/// `start`, under `base_dir` (empty = sibling `.ymux-worktrees`), with the
/// start sha and `run_id` stored as branch config. Returns the path.
#[tauri::command(async)]
pub fn git_worktree_add_from(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    branch: String,
    base_dir: String,
    start: String,
    run_id: String,
) -> YmuxResult<String> {
    guard_local(&webview, &request, "git_worktree_add_from")?;
    let repo = git::repo_root(Path::new(&cwd))?;
    let path = git::suggested_worktree_path(&repo, &branch, &base_dir);
    git::worktree_add_lane(&repo, &branch, &path, &start, &run_id)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Resolve `rev` (a branch or sha) to a full commit sha, once, so every
/// fan-out lane starts from the same commit.
#[tauri::command(async)]
pub fn git_resolve_commit(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    rev: String,
) -> YmuxResult<String> {
    guard_local(&webview, &request, "git_resolve_commit")?;
    git::resolve_commit(Path::new(&cwd), &rev)
}

/// Commits in HEAD that `base` (a hex sha) lacks: `git rev-list --count`.
#[tauri::command(async)]
pub fn git_commits_ahead(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    base: String,
) -> YmuxResult<u32> {
    guard_local(&webview, &request, "git_commits_ahead")?;
    git::commits_ahead(Path::new(&cwd), &base)
}

/// Fan-out lane metadata of the branch checked out at `cwd`.
#[tauri::command(async)]
pub fn git_lane_meta(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
) -> YmuxResult<git::LaneMeta> {
    guard_local(&webview, &request, "git_lane_meta")?;
    git::lane_meta(Path::new(&cwd))
}

/// `git branch -d` (merged only). Refuses the branch checked out at `cwd`.
#[tauri::command(async)]
pub fn git_branch_delete(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    branch: String,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "git_branch_delete")?;
    git::branch_delete(Path::new(&cwd), &branch)
}

/// Remove the worktree at `path`. `force` is passed through to `git worktree
/// remove --force` for worktrees with uncommitted changes.
#[tauri::command(async)]
pub fn git_worktree_remove(
    webview: Webview,
    request: Request<'_>,
    path: String,
    force: bool,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "git_worktree_remove")?;
    git::worktree_remove(Path::new(&path), force)
}

/// List all worktrees (main + linked) for the repository containing `cwd`,
/// the one `cwd` is in flagged `current`. Outside a repository: `NotARepo`.
#[tauri::command(async)]
pub fn git_worktree_list(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
) -> YmuxResult<Vec<git::WorktreeEntry>> {
    guard_local(&webview, &request, "git_worktree_list")?;
    let repo = git::repo_root_checked(Path::new(&cwd))?;
    git::worktree_list(Path::new(&repo))
}

/// Files that differ between `base` (empty = HEAD; a branch = its merge-base
/// with HEAD) and the working tree of the repository containing `cwd`,
/// untracked files included. See [`crate::git::diff_summary`].
#[tauri::command(async)]
pub fn git_diff_files(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    base: String,
    ignore_cr: bool,
) -> YmuxResult<git::DiffSummary> {
    guard_local(&webview, &request, "git_diff_files")?;
    git::diff_summary(Path::new(&cwd), &base, ignore_cr)
}

/// One file's hunks against the `base_rev` a [`git_diff_files`] call returned.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn git_diff_file(
    webview: Webview,
    request: Request<'_>,
    cwd: String,
    base_rev: String,
    path: String,
    orig: String,
    untracked: bool,
    ignore_cr: bool,
) -> YmuxResult<git::FileDiff> {
    guard_local(&webview, &request, "git_diff_file")?;
    git::diff_file(
        Path::new(&cwd),
        &base_rev,
        &path,
        &orig,
        untracked,
        ignore_cr,
    )
}

/// Panes that run an agent (a lead in the agents snapshot) with their shell
/// in the worktree `root`, not in a worktree nested under it. See
/// [`crate::git::pick_agent_panes`].
#[tauri::command(async)]
pub fn agent_panes_for_root(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    agents: State<'_, SharedAgents>,
    root: String,
) -> YmuxResult<Vec<Uuid>> {
    guard_local(&webview, &request, "agent_panes_for_root")?;
    let root = git::repo_root_checked(Path::new(&root))?;
    let worktrees: Vec<String> = git::worktree_list(Path::new(&root))?
        .into_iter()
        .filter(|w| !w.bare)
        .map(|w| w.path)
        .collect();
    let snapshot = agents.0.lock().snapshot();
    let cwds = state.pty.cwds_snapshot();
    // BTreeMap order: the answer is stable between calls.
    let with_agent: Vec<(Uuid, String)> = snapshot
        .iter()
        .filter(|(_, p)| p.lead.is_some())
        .filter_map(|(id, _)| cwds.get(id).map(|c| (*id, c.clone())))
        .collect();
    Ok(git::pick_agent_panes(&root, &worktrees, &with_agent))
}

/// Start the reader thread that drains PTY output and forwards it to the
/// frontend as Tauri events. Must be called once, at startup, after the
/// [`AppState`] is installed.
pub fn start_pty_event_pump(app: AppHandle) {
    let state = app.state::<AppState>();
    let rx = match state.pty.take_event_receiver() {
        Some(rx) => rx,
        None => {
            tracing::warn!("pty event pump already running");
            return;
        }
    };
    let app_for_thread = app.clone();
    std::thread::Builder::new()
        .name("ymux-pty-pump".into())
        .spawn(move || {
            while let Ok(event) = rx.recv() {
                match event {
                    crate::pty::session::PaneEvent::Data(id, bytes) => {
                        // Emit as a named event per pane. Payload is a raw
                        // `Vec<u8>` — Tauri serialises it as a JSON array of
                        // numbers, which xterm.js can write via
                        // `Uint8Array.from(payload)`.
                        let channel = format!("pty:data:{id}");
                        if let Err(e) = app_for_thread.emit(&channel, bytes) {
                            tracing::warn!(error = %e, "emit pty data failed");
                        }
                    }
                    crate::pty::session::PaneEvent::Exit(id, code) => {
                        let channel = format!("pty:exit:{id}");
                        if let Err(e) = app_for_thread.emit(&channel, code) {
                            tracing::warn!(error = %e, "emit pty exit failed");
                        }
                    }
                    crate::pty::session::PaneEvent::Cwd(id, cwd) => {
                        let channel = format!("pty:cwd:{id}");
                        if let Err(e) = app_for_thread.emit(&channel, cwd) {
                            tracing::warn!(error = %e, "emit pty cwd failed");
                        }
                    }
                }
            }
        })
        .expect("spawn pty event pump");
}
