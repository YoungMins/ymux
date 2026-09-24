//! Tauri command handlers exposed to the frontend. Each command is a thin
//! wrapper that validates arguments and delegates to the real implementation
//! in [`crate::config`], [`crate::pty`], or [`crate::shell`].
//!
//! The goal is to keep `#[tauri::command]` fns trivial so the actual logic can
//! be unit-tested without a running webview.

use portable_pty::PtySize;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use std::path::Path;

use crate::agent_sessions::{ResumePlan, SharedSessions};
use crate::agents::{AgentSnapshot, HookEvent, SharedAgents};
use crate::config::{Config, ConfigStore, ShellProfile};
use crate::error::{YmuxError, YmuxResult};
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
    /// Run this program directly instead of the `shell` profile. The file
    /// dock uses it for `ydir --dock <dir>`, so that ydir's exit is the
    /// pane's exit and no shell quoting is involved. Empty (the default)
    /// spawns the shell as before.
    #[serde(default)]
    pub argv: Vec<String>,
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
pub fn load_bootstrap(state: State<'_, AppState>) -> YmuxResult<BootstrapPayload> {
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
        }
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
pub fn detect_shells_cmd(state: State<'_, AppState>) -> YmuxResult<Vec<ShellProfile>> {
    let detected = shell::detect_shells();
    state.config.update(|c| c.shells = detected.clone());
    let _ = state.config.flush_if_dirty();
    Ok(detected)
}

#[tauri::command]
pub fn save_config(state: State<'_, AppState>, config: Config) -> YmuxResult<()> {
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
pub fn spawn_pane(state: State<'_, AppState>, args: SpawnArgs) -> YmuxResult<SpawnedPane> {
    let profile = match crate::pty::direct_profile(&args.argv, crate::pty::sidecar_dir().as_deref())
    {
        Some(direct) => direct,
        None => {
            let snapshot = state.config.snapshot();
            snapshot
                .shell(&args.shell)
                .ok_or_else(|| YmuxError::UnknownShell(args.shell.clone()))?
                .clone()
        }
    };

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
pub fn write_pane(state: State<'_, AppState>, args: WriteArgs) -> YmuxResult<()> {
    state.pty.write(args.id, &args.data)
}

#[tauri::command]
pub fn resize_pane(state: State<'_, AppState>, args: ResizeArgs) -> YmuxResult<()> {
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
pub fn kill_pane(state: State<'_, AppState>, id: Uuid) -> YmuxResult<()> {
    state.pty.kill(id)
}

#[tauri::command]
pub fn set_active_workspace(state: State<'_, AppState>, id: u32) -> YmuxResult<()> {
    state.config.update(|c| c.active_workspace = id);
    let _ = state.config.flush_if_dirty();
    Ok(())
}

/// Return the most recently reported working directory for a pane, or `None`
/// if the pane has not yet emitted an OSC 7 sequence.
#[tauri::command]
pub fn get_pane_cwd(state: State<'_, AppState>, id: Uuid) -> Option<String> {
    state.pty.cwd_for(id)
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

/// Route one `agent-hook` IPC payload (from `y agent-hook`) into the agent
/// registry. Payloads for panes this ymux doesn't own — malformed id, or a
/// pane that has since closed — are dropped.
pub fn apply_agent_hook(app: &AppHandle, payload: &serde_json::Value) {
    let Some(ev) = HookEvent::from_payload(payload) else {
        return;
    };
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
    if let Some(lead) = lead {
        let sessions = app.state::<SharedSessions>();
        let mut tracker = sessions.0.lock();
        if ev.event == "SessionEnd" {
            // The user quit the agent; keep the record but stop offering to
            // resume it (spec §4's "decline, don't delete").
            tracker.note_agent_exit(ev.pane_id);
        } else {
            observe_pane_session(
                app,
                &mut tracker,
                ev.pane_id,
                &lead.kind,
                lead.status,
                hook_id,
            );
        }
        flush_sessions(&mut tracker);
    }
}

/// The resume plan for one pane, or `None` when it should start normally.
///
/// Called by `TerminalPane.spawn()` *before* it decides whether to replay
/// scrollback (spec §4/§5): for a pane that is about to resume an agent, the
/// saved backlog is dead history that would sit above the resumed
/// conversation, so it is skipped entirely.
///
/// `startup_cmd` is the pane's own saved startup command, so any selector it
/// already carries can be stripped rather than fighting ours.
#[tauri::command]
pub fn get_agent_session(
    sessions: State<'_, SharedSessions>,
    pane_id: Uuid,
    startup_cmd: Option<String>,
) -> Option<ResumePlan> {
    let tracker = sessions.0.lock();
    crate::agent_sessions::plan_for(
        tracker.get(pane_id),
        startup_cmd.as_deref().unwrap_or_default(),
        crate::agent_sessions::now_secs(),
        crate::agent_sessions::transcript_exists,
    )
}

/// Forget a pane's session entirely. Called when the user closes a pane for
/// good, mirroring `delete_scrollback`.
#[tauri::command]
pub fn clear_agent_session(sessions: State<'_, SharedSessions>, pane_id: Uuid) -> YmuxResult<()> {
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

/// Feed one pane's current state into the session tracker.
///
/// Shared by the hook listener and the process scan so both write the same
/// store. The transcript lookup is only reached when the tracker's throttle
/// allows it, and it is skipped entirely for a pane with no known cwd —
/// without one there is nothing to match a transcript against.
pub fn observe_pane_session(
    app: &AppHandle,
    tracker: &mut crate::agent_sessions::SessionTracker,
    pane_id: Uuid,
    kind: &str,
    status: crate::agents::AgentStatus,
    hook_session_id: Option<String>,
) {
    let obs = crate::agent_sessions::PaneObservation {
        pane_id,
        kind: kind.to_string(),
        cwd: app.state::<AppState>().pty.cwd_for(pane_id),
        status,
        hook_session_id,
    };
    tracker.observe(
        &obs,
        crate::agent_sessions::now_secs(),
        crate::agent_scan_disk::newest_session,
    );
}

/// Current agent snapshot, for the frontend's initial render.
#[tauri::command]
pub fn get_agents(agents: State<'_, SharedAgents>) -> AgentSnapshot {
    agents.0.lock().snapshot()
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
    labels: State<'_, crate::agent_scan::SharedLabels>,
) -> std::collections::HashMap<Uuid, String> {
    labels.0.lock().clone()
}

/// Install (`true`) or remove (`false`) ymux's Claude Code hooks, then persist
/// the setting. The file is written first: if that fails (e.g. unparseable
/// settings.json) the error reaches the UI and the setting is not flipped.
#[tauri::command]
pub fn set_agent_tracking(state: State<'_, AppState>, enabled: bool) -> YmuxResult<()> {
    crate::agent_hooks::set_enabled(enabled)?;
    state.config.update(|c| c.agent_tracking = enabled);
    state.config.flush()?;
    Ok(())
}

/// Open a URL in the system default browser. Only `http://` and `https://`
/// URLs are accepted; anything else is rejected to prevent accidental
/// execution of arbitrary shell commands via `start` or `xdg-open`.
#[tauri::command]
pub fn open_url(url: String) -> YmuxResult<()> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(YmuxError::Other(
            "open_url: only http/https URLs are supported".into(),
        ));
    }
    #[cfg(windows)]
    {
        // `start "" <url>` — the empty string is the window title, required
        // when the URL contains query params so `cmd /C start` doesn't
        // misparse the first `=` as a window-title separator.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .spawn()
            .map_err(YmuxError::Io)?;
    }
    #[cfg(not(windows))]
    {
        // macOS (`open`) and Linux (`xdg-open`) — `opener` picks the right
        // launcher per platform. It is already a dependency for the Settings
        // "open config file" action, so this costs nothing extra and avoids
        // hardcoding `xdg-open`, which does not exist on macOS.
        opener::open_browser(&url).map_err(|e| YmuxError::Other(format!("open_url: {e}")))?;
    }
    Ok(())
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
    webview: tauri::Webview,
    paths: Vec<String>,
    cwd: Option<String>,
) -> YmuxResult<Vec<Option<crate::fspath::ResolvedPath>>> {
    // `capabilities/browser-children.json` hands `core:default` to every
    // `eb-*` child webview on http(s) origins, so without this an arbitrary
    // website open in an embedded browser pane could use this command as a
    // filesystem oracle.
    if !crate::fspath::caller_allowed(webview.label()) {
        return Err(YmuxError::Other(
            "resolve_paths: only the main webview may resolve local paths".into(),
        ));
    }
    tauri::async_runtime::spawn_blocking(move || crate::fspath::probe_batch(paths, cwd))
        .await
        .map_err(|e| YmuxError::Other(format!("resolve_paths: {e}")))
}

/// Open an absolute path with the OS default handler — `ShellExecuteW` on
/// Windows, `open` on macOS, both via `opener`. Neither builds a command
/// line, so `&`, `^`, `%` and quotes in a filename are inert.
///
/// A directory opens in the file manager. An executable or script is
/// *revealed* in the file manager rather than launched: clicking a path in
/// terminal output means "show me this", and terminal output is
/// attacker-influenced, so `ShellExecuteW` running `evil.bat` is not an
/// acceptable reading of the click. See [`crate::fspath::should_reveal`].
#[tauri::command]
pub async fn open_path(webview: tauri::Webview, path: String) -> YmuxResult<()> {
    if !crate::fspath::caller_allowed(webview.label()) {
        return Err(YmuxError::Other(
            "open_path: only the main webview may open local paths".into(),
        ));
    }
    // The path was vetted by `resolve_paths`, but it made a round trip
    // through the frontend to get here, so it is validated again from
    // scratch rather than trusted.
    crate::fspath::validate_open(&path).map_err(YmuxError::Other)?;
    tauri::async_runtime::spawn_blocking(move || {
        let p = Path::new(&path);
        let meta = std::fs::metadata(p)
            .map_err(|e| YmuxError::Other(format!("open_path: {path}: {e}")))?;
        let result = if crate::fspath::should_reveal(p, meta.is_dir()) {
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
pub fn notify(app: AppHandle, title: String, body: String) -> YmuxResult<()> {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
    Ok(())
}

/// Persist `blob` (serialized terminal scrollback) for `pane_id` to disk.
/// Thin wrapper — the actual fs logic lives in [`crate::scrollback`] so it
/// can be unit-tested without a running webview (and on Linux CI, where this
/// `desktop`-gated module doesn't even compile).
#[tauri::command]
pub fn save_scrollback(pane_id: String, blob: String) -> YmuxResult<()> {
    crate::scrollback::save_blob(&pane_id, &blob).map_err(YmuxError::Io)
}

/// Load the persisted scrollback for `pane_id`, or an empty string if none
/// has been saved yet.
#[tauri::command]
pub fn load_scrollback(pane_id: String) -> YmuxResult<String> {
    crate::scrollback::load_blob(&pane_id).map_err(YmuxError::Io)
}

/// Delete the persisted scrollback for `pane_id`, if any.
#[tauri::command]
pub fn delete_scrollback(pane_id: String) -> YmuxResult<()> {
    crate::scrollback::delete_blob(&pane_id).map_err(YmuxError::Io)
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
pub fn paste_clipboard_image(state: State<'_, AppState>) -> YmuxResult<Option<String>> {
    let Some(png) = crate::clipboard_image::read_clipboard_png().map_err(YmuxError::Io)? else {
        return Ok(None);
    };
    let hours = state.config.snapshot().paste_image_retention_hours;
    let retention = std::time::Duration::from_secs(u64::from(hours) * 3600);
    let path = crate::paste_images::save(&png, retention).map_err(YmuxError::Io)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Check whether `cwd` sits inside a git repository (main worktree or a
/// linked worktree). Thin wrapper over [`crate::git::is_git_repo`].
#[tauri::command]
pub fn git_is_repo(cwd: String) -> bool {
    git::is_git_repo(Path::new(&cwd))
}

/// Create a new git worktree for `branch`, rooted at the repo containing
/// `cwd`, under a suggested sibling path derived from `base`. Returns the
/// created worktree's path.
#[tauri::command]
pub fn git_worktree_add(cwd: String, branch: String, base: String) -> YmuxResult<String> {
    let repo = git::repo_root(Path::new(&cwd))?;
    let path = git::suggested_worktree_path(&repo, &branch, &base);
    git::worktree_add(&repo, &branch, &path)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Remove the worktree at `path`. `force` is passed through to `git worktree
/// remove --force` for worktrees with uncommitted changes.
#[tauri::command]
pub fn git_worktree_remove(path: String, force: bool) -> YmuxResult<()> {
    git::worktree_remove(Path::new(&path), force)
}

/// List all worktrees (main + linked) for the repository containing `cwd`.
#[tauri::command]
pub fn git_worktree_list(cwd: String) -> YmuxResult<Vec<git::WorktreeEntry>> {
    let repo = git::repo_root(Path::new(&cwd))?;
    git::worktree_list(&repo)
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
