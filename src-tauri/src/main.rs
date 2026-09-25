#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, RunEvent};
use ymux_lib::agent_scan::start_agent_scan;
use ymux_lib::commands::{start_pty_event_pump, AppState};
use ymux_lib::config::ConfigStore;
use ymux_lib::pty::PtyManager;
use ymux_lib::sysmonitor::start_sysmonitor;
use ymux_lib::updater::start_update_checker;

/// Set once the final save below has run, so the second of
/// `ExitRequested` / `Exit` does nothing.
static FINAL_FLUSH_DONE: AtomicBool = AtomicBool::new(false);

/// Close the browser webviews, record every pane's cwd, stop the PTYs and
/// write the config -- once. It runs from `RunEvent::ExitRequested` (the last
/// window closed) and again from `RunEvent::Exit`, because on macOS Quit from
/// the Dock, logout and shutdown go straight through tao's
/// `applicationWillTerminate` to `Exit` and never raise `ExitRequested`.
fn final_flush(app_handle: &AppHandle) {
    if FINAL_FLUSH_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
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
    let registry = app_handle.state::<ymux_lib::embedded_browser::EmbeddedBrowserRegistry>();
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

/// The menu id of ymux's own macOS Quit item (see `macos_menu`).
#[cfg(target_os = "macos")]
const QUIT_MENU_ID: &str = "ymux-quit";

/// Set when the Quit item asked the main window to close, so that window's
/// destruction ends the app even if a native browser window is still open.
#[cfg(target_os = "macos")]
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Tauri's default macOS menu (`Menu::default`), minus the two items that end
/// ymux behind the frontend's back:
///
/// - **Quit (Cmd+Q)** is a predefined item that calls `terminate:` directly,
///   skipping the unsaved-editor prompt. It is replaced by a plain item
///   whose handler closes the main window instead -- exactly what the red
///   close button does, so the same `onCloseRequested` guard in `main.ts`
///   runs (prompt -> destroy -> `ExitRequested` -> `final_flush`).
/// - **Close Window (Cmd+W)** closes ymux's only window, i.e. quits. It is
///   left out (with the File submenu that held it), so Cmd+W reaches the
///   webview like any other key.
///
/// Edit keeps the predefined items: WKWebView's copy/paste/undo key
/// equivalents only work through them.
#[cfg(target_os = "macos")]
fn macos_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
    let pkg = app.package_info();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: app.config().bundle.copyright.clone(),
        authors: app.config().bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
    let quit = MenuItem::with_id(
        app,
        QUIT_MENU_ID,
        format!("Quit {}", pkg.name),
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                pkg.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::show_all(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &Submenu::with_items(
                app,
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(app, None)?,
                    &PredefinedMenuItem::maximize(app, None)?,
                ],
            )?,
        ],
    )
}

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
    let builder = tauri::Builder::default()
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
        .plugin(ymux_lib::fspath::navigation_guard_plugin());
    #[cfg(target_os = "macos")]
    let builder = builder.menu(macos_menu).on_menu_event(|app, event| {
        if event.id() != QUIT_MENU_ID {
            return;
        }
        // Close, not destroy: `close()` raises CloseRequested, which the
        // frontend's guard answers (and holds while its prompt is up; a
        // second Cmd+Q meanwhile is swallowed there). Without a main window
        // there is nothing to guard.
        match app.get_webview_window("main") {
            Some(w) => {
                QUIT_REQUESTED.store(true, Ordering::SeqCst);
                if w.close().is_err() {
                    app.exit(0);
                }
            }
            None => app.exit(0),
        }
    });
    builder
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
            // Without a receiver the token is set *empty* rather than left
            // out: a ymux started from another ymux's pane would otherwise
            // hand its panes the parent's token, and their hooks would reach
            // the parent as 403s (hook errors). Empty is a quiet no-op there.
            let token = if hook_port.is_some() { token } else { String::new() };
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
        .run(|app_handle, event| match event {
            RunEvent::ExitRequested { .. } | RunEvent::Exit => final_flush(app_handle),
            // The Quit item closed the main window and the guard let it go:
            // quit, even if a native browser window would keep the app alive.
            #[cfg(target_os = "macos")]
            RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Destroyed,
                ..
            } if label == "main" && QUIT_REQUESTED.load(Ordering::SeqCst) => app_handle.exit(0),
            _ => {}
        });
}
