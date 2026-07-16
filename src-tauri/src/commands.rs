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
    let snapshot = state.config.snapshot();
    let profile = snapshot
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
        // Development hosts (Linux/macOS).
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
    Ok(())
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
                }
            }
        })
        .expect("spawn pty event pump");
}
