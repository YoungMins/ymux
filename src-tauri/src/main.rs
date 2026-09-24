#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{Manager, RunEvent};
use ymux_lib::agent_scan::start_agent_scan;
use ymux_lib::commands::{start_pty_event_pump, AppState};
use ymux_lib::config::ConfigStore;
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

    // Whether the layout below was really read from the user's config file,
    // rather than defaulted: only then may it prune agent-session records.
    let mut layout_from_disk = ymux_lib::config::config_path().exists();
    let config = ConfigStore::load_default().unwrap_or_else(|e| {
        tracing::error!(error = %e, "failed to load config, using default");
        layout_from_disk = false;
        // Fall back to an in-memory default at a throwaway path if load
        // somehow fails after the empty-file path — this keeps the app from
        // refusing to start on permission issues.
        ConfigStore::load(std::env::temp_dir().join("ymux-fallback.toml"))
            .expect("default load cannot fail")
    });

    // Resumable agent sessions, loaded from disk once at startup. A missing
    // or corrupt file loads as an empty store (see `agent_sessions::load_from`)
    // — panes then just start normally. Records for panes the saved layout no
    // longer has are dropped here, once.
    let mut sessions =
        ymux_lib::agent_sessions::SessionTracker::from_store(ymux_lib::agent_sessions::load());
    if layout_from_disk {
        let panes: std::collections::HashSet<uuid::Uuid> = config
            .snapshot()
            .workspaces
            .iter()
            .flat_map(|w| w.panes())
            .map(|p| p.id)
            .collect();
        sessions.retain_panes(&panes);
        ymux_lib::commands::flush_sessions(&mut sessions);
    }

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
        .manage(ymux_lib::agent_sessions::SharedSessions(
            parking_lot::Mutex::new(sessions),
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
            ymux_lib::commands::list_editor_drafts,
            ymux_lib::commands::get_agent_session,
            ymux_lib::commands::clear_agent_session,
            ymux_lib::commands::paste_clipboard_image,
            // The filesystem / text-file / git surface for the files,
            // editor and git panes.
            ymux_lib::fsops::fs_list_dir,
            ymux_lib::fsops::fs_roots,
            ymux_lib::fsops::fs_home_dir,
            ymux_lib::fsops::fs_stat,
            ymux_lib::fsops::fs_paths_within,
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
            ymux_lib::commands::git_checkout_track,
            ymux_lib::commands::git_work_status,
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
            // Claude Code hook receiver. Its per-run token goes into every
            // PTY spawned from here on, so only a Claude running inside one
            // of this ymux's panes can report agent events (rule 13).
            let refresh_allowed = {
                use ymux_lib::agent_hooks::{startup_refresh_allowed, DEV_HOOKS_ENV};
                let opt_in = std::env::var(DEV_HOOKS_ENV).ok();
                startup_refresh_allowed(cfg!(debug_assertions), opt_in.as_deref())
            };
            let token = ymux_lib::hook_http::new_token();
            let hook_port = ymux_lib::commands::start_hook_receiver(
                app.handle(),
                token.clone(),
                refresh_allowed,
            );
            app.manage(ymux_lib::commands::AgentHookPort(hook_port));
            let state = app.state::<AppState>();
            state.pty.set_extra_env(vec![(
                ymux_lib::hook_http::TOKEN_ENV.to_string(),
                token,
            )]);
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
            // launch: it points the hooks at a port that had to change, and
            // migrates the retired `y` command entries of an upgraded user.
            // Debug builds skip it (unless opted in) so `tauri dev` doesn't
            // repoint the real hooks at the dev build's receiver.
            if state.config.snapshot().agent_tracking {
                match (refresh_allowed, hook_port) {
                    (true, Some(port)) => {
                        if let Err(e) = ymux_lib::agent_hooks::set_enabled(true, port) {
                            tracing::warn!(error = %e, "failed to refresh Claude Code hooks at startup");
                        }
                    }
                    (true, None) => {}
                    (false, _) => tracing::info!(
                        "debug build: skipping Claude Code hook refresh so the installed \
                         hooks keep pointing at the release build; set {}=1 to refresh \
                         them to this build",
                        ymux_lib::agent_hooks::DEV_HOOKS_ENV
                    ),
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
