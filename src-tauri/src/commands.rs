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

use crate::config::{Config, ConfigStore, ShellProfile};
use crate::error::{YmuxError, YmuxResult};
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
    let config = state.config.snapshot();
    // Prefer cached shells; if empty, detect and persist.
    let shells = if config.shells.is_empty() {
        let detected = shell::detect_shells();
        state.config.update(|c| c.shells = detected.clone());
        let _ = state.config.flush_if_dirty();
        detected
    } else {
        config.shells.clone()
    };
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
    state.config.replace(config);
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
