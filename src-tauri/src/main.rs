#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{Manager, RunEvent};
use ymux_lib::commands::{start_pty_event_pump, AppState};
use ymux_lib::config::ConfigStore;
use ymux_lib::pty::PtyManager;
use ymux_lib::sysmonitor::start_sysmonitor;
use ymux_lib::updater::start_update_checker;

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

    // `generate_handler!` requires the absolute path to each command so the
    // helper macros it expands into (`__cmd__<name>`) resolve through the
    // `ymux_lib::commands` module they were defined in. Importing the names
    // via `use` is not enough — macros are not re-exported by `use`.
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            ymux_lib::commands::load_bootstrap,
            ymux_lib::commands::detect_shells_cmd,
            ymux_lib::commands::save_config,
            ymux_lib::commands::spawn_pane,
            ymux_lib::commands::write_pane,
            ymux_lib::commands::resize_pane,
            ymux_lib::commands::kill_pane,
            ymux_lib::commands::set_active_workspace,
            ymux_lib::commands::get_pane_cwd,
            ymux_lib::commands::open_url,
        ])
        .setup(|app| {
            start_pty_event_pump(app.handle().clone());
            start_update_checker(app.handle().clone());
            start_sysmonitor(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::ExitRequested { .. } = event {
                let state = app_handle.state::<AppState>();
                // Snapshot the live OSC 7 `cwd` reports *before* shutting
                // down the PTY sessions, then patch them into the config so
                // the final on-disk layout carries whatever directory each
                // shell was sitting in at exit time. This is what makes
                // "reopen in the last working directory" work across app
                // restarts without relying on the frontend's async
                // `beforeunload` save racing the window close.
                let cwds = state.pty.cwds_snapshot();
                state.config.update(|c| c.patch_cwds(&cwds));
                state.pty.shutdown_all();
                if let Err(e) = state.config.flush() {
                    tracing::warn!(error = %e, "final config flush failed");
                }
            }
        });
}
