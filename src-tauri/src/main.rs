#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{Manager, RunEvent};
use ymux_lib::agent_scan::start_agent_scan;
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
        .manage(ymux_lib::agents::SharedAgents::default())
        .manage(ymux_lib::agent_scan::SharedLabels::default())
        // Resumable agent sessions, loaded from disk once at startup. A
        // missing or corrupt file loads as an empty store (see
        // `agent_sessions::load_from`) — panes then just start normally.
        .manage(ymux_lib::agent_sessions::SharedSessions(
            parking_lot::Mutex::new(ymux_lib::agent_sessions::SessionTracker::from_store(
                ymux_lib::agent_sessions::load(),
            )),
        ))
        // EVERY command below must start with a guard — `fspath::guard_local`
        // (ymux's own document only), or `embedded_browser::guard_embedded_child`
        // for the two listed in `ipc_guard::EMBEDDED_CHILD_COMMANDS`. There is
        // no ACL behind them: without an `AppManifest` Tauri never checks app
        // commands, so any page ymux loads can call anything registered here.
        // `ipc_guard::tests::every_registered_command_starts_with_a_guard`
        // parses this list and each command's body; adding a line here and a
        // `#[tauri::command]` must change together. CLAUDE.md rule 16.
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
            ymux_lib::commands::resolve_paths,
            ymux_lib::commands::open_path,
            ymux_lib::commands::notify,
            ymux_lib::commands::save_scrollback,
            ymux_lib::commands::load_scrollback,
            ymux_lib::commands::delete_scrollback,
            ymux_lib::commands::save_editor_draft,
            ymux_lib::commands::load_editor_draft,
            ymux_lib::commands::delete_editor_draft,
            ymux_lib::commands::get_agent_session,
            ymux_lib::commands::clear_agent_session,
            ymux_lib::commands::paste_clipboard_image,
            // The filesystem / text-file / git surface for the files,
            // editor and git panes.
            ymux_lib::fsops::fs_list_dir,
            ymux_lib::fsops::fs_roots,
            ymux_lib::fsops::fs_home_dir,
            ymux_lib::fsops::fs_stat,
            ymux_lib::fsops::fs_create_dir,
            ymux_lib::fsops::fs_create_file,
            ymux_lib::fsops::fs_rename,
            ymux_lib::fsops::fs_copy,
            ymux_lib::fsops::fs_move,
            ymux_lib::fsops::fs_delete,
            ymux_lib::fsops::fs_read_text,
            ymux_lib::fsops::fs_read_head,
            ymux_lib::fsops::fs_peek_dir,
            ymux_lib::fsops::fs_write_text,
            ymux_lib::fsops::fs_reveal,
            ymux_lib::fsops::fs_open_default,
            ymux_lib::commands::git_log,
            ymux_lib::commands::git_branches,
            ymux_lib::commands::git_checkout,
            ymux_lib::commands::git_repo_root,
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
            ymux_lib::commands::get_agents,
            ymux_lib::commands::get_pane_labels,
            ymux_lib::commands::set_agent_tracking,
        ])
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let ipc_addr = start_ipc_server(app.handle().clone());
            // Inject YMUX_IPC into every PTY that will be spawned, plus the
            // app's own directory on PATH so the bundled y* tools resolve by
            // name. On Windows the installer already put that directory on the
            // system PATH; on macOS nothing can, so this is the only thing
            // making `ydir` work from a pane.
            let state = app.state::<AppState>();
            let mut pty_env = vec![("YMUX_IPC".to_string(), ipc_addr)];
            if let Some(path) = ymux_lib::pty::sidecar_path_entry() {
                pty_env.push(path);
            }
            state.pty.set_extra_env(pty_env);
            // Prune paste-image temp files left over from a previous
            // session — otherwise the last paste of a session would outlive
            // its retention window forever, since `save` only prunes on the
            // *next* paste. Best-effort: a prune failure must never block
            // startup.
            let retention_hours = state.config.snapshot().paste_image_retention_hours;
            let retention = std::time::Duration::from_secs(u64::from(retention_hours) * 3600);
            if let Err(e) = ymux_lib::paste_images::prune(retention) {
                tracing::warn!(error = %e, "failed to prune old paste images at startup");
            }
            // While agent tracking is on, re-run the hook install on every
            // launch: a reinstall to another directory would otherwise leave
            // Claude Code calling a stale `y` path.
            // Debug builds skip it (unless opted in) so `tauri dev` doesn't
            // repoint the real hooks at `target/debug/y.exe`.
            if state.config.snapshot().agent_tracking {
                use ymux_lib::agent_hooks::{startup_refresh_allowed, DEV_HOOKS_ENV};
                let opt_in = std::env::var(DEV_HOOKS_ENV).ok();
                if startup_refresh_allowed(cfg!(debug_assertions), opt_in.as_deref()) {
                    if let Err(e) = ymux_lib::agent_hooks::set_enabled(true) {
                        tracing::warn!(error = %e, "failed to refresh Claude Code hooks at startup");
                    }
                } else {
                    tracing::info!(
                        "debug build: skipping Claude Code hook refresh so the installed \
                         hooks keep pointing at the release `y`; set {DEV_HOOKS_ENV}=1 to \
                         refresh them to this build"
                    );
                }
            }
            start_pty_event_pump(app.handle().clone());
            start_update_checker(app.handle().clone());
            start_sysmonitor(app.handle().clone());
            start_agent_scan(app.handle().clone());
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
