#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use tauri::{Manager, RunEvent};
use ymux_lib::config::ConfigStore;
use ymux_lib::pty::PtyManager;

use crate::commands::{
    detect_shells_cmd, kill_pane, load_bootstrap, resize_pane, save_config, set_active_workspace,
    spawn_pane, start_pty_event_pump, write_pane, AppState,
};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ymux=info,warn".into()),
        )
        .init();

    let config = ConfigStore::load_default().unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to load config, using default");
        // Fall back to an in-memory default at a throwaway path if load
        // somehow fails after the empty-file path — this keeps the app from
        // refusing to start on permission issues.
        ConfigStore::load(std::env::temp_dir().join("ymux-fallback.toml"))
            .expect("default load cannot fail")
    });

    let state = AppState {
        config,
        pty: PtyManager::default(),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            load_bootstrap,
            detect_shells_cmd,
            save_config,
            spawn_pane,
            write_pane,
            resize_pane,
            kill_pane,
            set_active_workspace,
        ])
        .setup(|app| {
            start_pty_event_pump(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::ExitRequested { .. } = event {
                let state = app_handle.state::<AppState>();
                state.pty.shutdown_all();
                if let Err(e) = state.config.flush() {
                    tracing::warn!(error = %e, "final config flush failed");
                }
            }
        });
}
