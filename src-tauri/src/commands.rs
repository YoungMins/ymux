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

use crate::agent_sessions::{ResumeOutcome, SharedSessions};
use crate::agents::{AgentSnapshot, HookEvent, SharedAgents};
use crate::config::{Config, ConfigStore, ShellProfile};
use crate::error::{YmuxError, YmuxResult};
use crate::fspath::guard_local;
use crate::git;
use crate::pty::{PtyManager, SpawnedPane};
use crate::shell;

/// State container registered via `Tauri::manage`.
pub struct AppState {
    pub config: ConfigStore,
    pub pty: PtyManager,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnArgs {
    pub id: Uuid,
    pub shell: String,
    pub cwd: Option<String>,
    pub rows: u16,
    pub cols: u16,
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

#[tauri::command]
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

#[tauri::command]
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
/// this cannot start anything. Off the main thread — it stats a few dozen
/// files across `PATH` and the known install directories.
#[tauri::command]
pub async fn detect_agents(
    webview: Webview,
    request: Request<'_>,
) -> YmuxResult<Vec<crate::agent_launch::DetectedAgent>> {
    guard_local(&webview, &request, "detect_agents")?;
    tauri::async_runtime::spawn_blocking(crate::agent_launch::detect_agents)
        .await
        .map_err(|e| YmuxError::Other(format!("detect_agents: {e}")))
}

#[tauri::command]
pub fn save_config(
    webview: Webview,
    request: Request<'_>,
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
    state.config.update(|c| c.merge_layouts_from(incoming));
    state.config.flush()?;
    Ok(())
}

#[tauri::command]
pub fn spawn_pane(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    args: SpawnArgs,
) -> YmuxResult<SpawnedPane> {
    guard_local(&webview, &request, "spawn_pane")?;
    let profile = state
        .config
        .snapshot()
        .shell(&args.shell)
        .ok_or_else(|| YmuxError::UnknownShell(args.shell.clone()))?
        .clone();

    let spec = crate::config::model::PaneSpec {
        id: args.id,
        title: None,
        shell: profile.name.clone(),
        cwd: args.cwd,
        startup_cmd: None,
        env: Vec::new(),
        pane_kind: crate::config::model::PaneKind::Terminal,
        url: None,
        hotkeys: Vec::new(),
        bg_color: String::new(),
        worktree_path: String::new(),
        file_path: String::new(),
    };

    state.pty.spawn(
        &spec,
        &profile,
        PtySize {
            rows: args.rows.max(1),
            cols: args.cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        },
    )
}

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
    state: State<'_, AppState>,
    id: Uuid,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "kill_pane")?;
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

/// Tauri event carrying the full agent snapshot after every registry change.
pub const AGENTS_CHANGED_EVENT: &str = "agents:changed";

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
/// `async` so it runs off the main thread: the session lock is shared with
/// the scan thread, and the transcript check touches the disk. The lock is
/// never held across that check.
#[tauri::command(async)]
pub fn get_agent_session(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    sessions: State<'_, SharedSessions>,
    pane_id: Uuid,
    startup_cmd: Option<String>,
    shell: Option<String>,
) -> YmuxResult<ResumeOutcome> {
    guard_local(&webview, &request, "get_agent_session")?;
    let family = shell
        .as_deref()
        .and_then(|name| {
            state
                .config
                .snapshot()
                .shell(name)
                .map(|p| crate::agent_sessions::ShellFamily::from_executable(&p.executable))
        })
        .unwrap_or(crate::agent_sessions::ShellFamily::Unknown);
    let now = crate::agent_sessions::now_secs();
    // Copy the record out; the disk check below runs without the lock.
    let record = sessions.0.lock().get(pane_id).cloned();
    let outcome = crate::agent_sessions::outcome_for(
        record.as_ref(),
        startup_cmd.as_deref().unwrap_or_default(),
        family,
        now,
        crate::agent_sessions::transcript_exists,
    );
    // A resume: the frontend types the command next, and until the scan sees
    // the resumed agent running the pane's old scrollback stays on disk. A
    // transcript that is gone: the record is declined now.
    sessions.0.lock().note_outcome(pane_id, &outcome, now);
    Ok(outcome)
}

/// Forget a pane's session entirely. Called when the user closes a pane for
/// good, mirroring `delete_scrollback`.
#[tauri::command]
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
    let obs = crate::agent_sessions::PaneObservation {
        pane_id: input.pane_id,
        kind: input.kind.to_string(),
        cwd: app.state::<AppState>().pty.cwd_for(input.pane_id),
        status: input.status,
        hook_session_id: input.hook_session_id,
        process: input.process,
        pid_file_session_id: input.pid_file_session_id,
    };
    tracker.observe(
        &obs,
        crate::agent_sessions::now_secs(),
        input.others,
        crate::agent_scan_disk::candidate_sessions,
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

/// Install (`true`) or remove (`false`) ymux's Claude Code hooks, then persist
/// the setting. The file is written first: if that fails (e.g. unparseable
/// settings.json) the error reaches the UI and the setting is not flipped.
#[tauri::command]
pub fn set_agent_tracking(
    webview: Webview,
    request: Request<'_>,
    state: State<'_, AppState>,
    hook_port: State<'_, AgentHookPort>,
    enabled: bool,
) -> YmuxResult<()> {
    guard_local(&webview, &request, "set_agent_tracking")?;
    let port = match (enabled, hook_port.0) {
        (true, None) => {
            return Err(YmuxError::Other(
                "this ymux has no Claude Code hook receiver (another ymux instance \
                 holds its port, or it failed to start)"
                    .into(),
            ))
        }
        // Uninstall doesn't need the port: it matches any ymux entry.
        (_, port) => port.unwrap_or(0),
    };
    crate::agent_hooks::set_enabled(enabled, port)?;
    state.config.update(|c| c.agent_tracking = enabled);
    state.config.flush()?;
    Ok(())
}

/// Port the Claude Code hook receiver listens on this run (`None` if it
/// could not start). Managed state, read by [`set_agent_tracking`].
pub struct AgentHookPort(pub Option<u16>);

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
    let served = hook_http::serve(
        listener,
        token,
        std::sync::Arc::new(move |pane| known.state::<AppState>().pty.has(pane)),
        std::sync::Arc::new(move |ev| apply_agent_hook(&apply, ev)),
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
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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
