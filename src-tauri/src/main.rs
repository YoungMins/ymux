#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{Manager, RunEvent};
use ymux_lib::commands::{start_pty_event_pump, AppState};
use ymux_lib::config::ConfigStore;
use ymux_lib::ipc_server::start_ipc_server;
use ymux_lib::pty::PtyManager;
use ymux_lib::sysmonitor::start_sysmonitor;
use ymux_lib::updater::start_update_checker;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ymux=info,ymux_lib=info,warn".into()),
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
    let eb_registry = ymux_lib::embedded_browser::EmbeddedBrowserRegistry::default();

    // `generate_handler!` requires the absolute path to each command so the
    // helper macros it expands into (`__cmd__<name>`) resolve through the
    // `ymux_lib::commands` module they were defined in. Importing the names
    // via `use` is not enough — macros are not re-exported by `use`.
    tauri::Builder::default()
        .manage(state)
        .manage(eb_registry)
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
            ymux_lib::commands::notify,
            ymux_lib::commands::save_scrollback,
            ymux_lib::commands::load_scrollback,
            ymux_lib::commands::delete_scrollback,
            ymux_lib::commands::git_is_repo,
            ymux_lib::commands::git_worktree_add,
            ymux_lib::commands::git_worktree_remove,
            ymux_lib::commands::git_worktree_list,
            ymux_lib::webview::create_webview,
            ymux_lib::webview::destroy_webview,
            ymux_lib::webview::navigate_webview,
            ymux_lib::webview::resize_webview,
            ymux_lib::webview::zoom_webview,
            ymux_lib::webview::set_webview_visible,
            ymux_lib::embedded_browser::create_embedded_browser,
            ymux_lib::embedded_browser::destroy_embedded_browser,
            ymux_lib::embedded_browser::navigate_embedded_browser,
            ymux_lib::embedded_browser::set_embedded_browser_bounds,
            ymux_lib::embedded_browser::set_embedded_browser_visible,
            ymux_lib::embedded_browser::forward_keystroke,
            ymux_lib::embedded_browser::child_webview_focused,
            ymux_lib::settings::load_syntax_theme,
            ymux_lib::settings::save_syntax_theme,
            ymux_lib::settings::open_config_path,
        ])
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let ipc_addr = start_ipc_server(app.handle().clone());
            // Inject YMUX_IPC into every PTY that will be spawned.
            let state = app.state::<AppState>();
            state.pty.set_extra_env(vec![("YMUX_IPC".into(), ipc_addr)]);
            start_pty_event_pump(app.handle().clone());
            start_update_checker(app.handle().clone());
            start_sysmonitor(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::ExitRequested { .. } = event {
                // Close any child browser webviews first so they don't
                // block the window close.
                for (label, wv) in app_handle.webview_windows() {
                    if label.starts_with("browser-") {
                        let _ = wv.close();
                    }
                }
                // Close embedded browser child webviews (eb-* labels).
                // These are Webview instances, not WebviewWindows, so they
                // don't appear in webview_windows() and need separate cleanup.
                let registry =
                    app_handle.state::<ymux_lib::embedded_browser::EmbeddedBrowserRegistry>();
                if let Ok(labels) = registry.labels.lock() {
                    for label in labels.iter() {
                        if let Some(wv) = app_handle.get_webview(label) {
                            let _ = wv.close();
                        }
                    }
                }

                let state = app_handle.state::<AppState>();
                let cwds = state.pty.cwds_snapshot();
                state.config.update(|c| c.patch_cwds(&cwds));
                state.pty.shutdown_all();
                if let Err(e) = state.config.flush() {
                    tracing::warn!(error = %e, "final config flush failed");
                }
            }
        });
}
