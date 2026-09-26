//! Close-to-tray: the tray icon, hiding/showing the main window, and the
//! real-quit handshake with the frontend. The decisions are the pure
//! [`crate::quit_gate::QuitGate`]; this module only wires them to Tauri.
//!
//! - The main window's close request hides it (see [`on_close_requested`]);
//!   the PTYs and agents keep running behind the tray icon. Native browser
//!   windows (`browser-*`, owned by main) are hidden and re-shown with it.
//! - Left-click on the tray icon shows the window; right-click opens the
//!   menu (Open ymux / Quit). macOS Dock clicks (`RunEvent::Reopen`) show it
//!   too (`main.rs`).
//! - Quit ([`request_quit`]) asks the frontend's unsaved-editors prompt via
//!   `ymux://quit-requested`; the frontend acks it (`ack_quit`), answers
//!   with `answer_quit`, and a confirmed quit is `app.exit(0)` →
//!   `ExitRequested` → `final_flush`. Every main-page load disarms the
//!   handshake (`main.rs`'s `on_page_load` → [`main_page_loading`]).
//!
//! The menu labels and tooltip start in English and are replaced by the
//! frontend's i18n strings through `set_tray_labels` (rule 7) — the
//! translations live in `src/i18n/i18n.ts` only.

use std::sync::Mutex;
use std::time::Instant;

use tauri::ipc::Request;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Webview, Window};

use crate::fspath::guard_local;
use crate::quit_gate::{clean_label, menu_text, CloseAction, QuitGate, QuitStart};

/// The tray icon's id (`AppHandle::tray_by_id`).
pub const TRAY_ID: &str = "ymux-tray";
/// Menu item ids. Menu events are global in Tauri, so `main.rs`'s single
/// `on_menu_event` routes these through [`handle_menu_event`].
pub const MENU_OPEN: &str = "ymux-tray-open";
pub const MENU_QUIT: &str = "ymux-tray-quit";
/// ymux's own macOS app-menu Quit (Cmd+Q) item (`main.rs`'s `macos_menu`).
pub const APP_MENU_QUIT: &str = "ymux-quit";

/// Emitted to the main webview: run the unsaved-editors prompt, then answer
/// with `answer_quit`.
pub const QUIT_REQUESTED_EVENT: &str = "ymux://quit-requested";
/// Emitted to the main webview after the window was hidden to the tray /
/// shown again, so the frontend can mark itself unseen / refit (rule 14).
pub const HIDDEN_EVENT: &str = "ymux://main-hidden";
pub const SHOWN_EVENT: &str = "ymux://main-shown";

const MAIN: &str = "main";

/// Labels of the native browser windows `hide_main` hid, so `show_main`
/// re-shows exactly those (not ones that were already hidden).
static HIDDEN_BROWSERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn menu(app: &AppHandle, open: &str, quit: &str) -> tauri::Result<Menu<tauri::Wry>> {
    let windows = cfg!(windows);
    let open = MenuItem::with_id(app, MENU_OPEN, menu_text(open, windows), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, menu_text(quit, windows), true, None::<&str>)?;
    Menu::with_items(app, &[&open, &quit])
}

/// Build the tray icon. Records in the [`QuitGate`] whether it exists: with
/// no tray, closing the window quits instead of hiding it into nowhere.
pub fn build_tray(app: &AppHandle) {
    let gate = app.state::<QuitGate>();
    let result = (|| -> tauri::Result<()> {
        let icon = app
            .default_window_icon()
            .cloned()
            .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?;
        TrayIconBuilder::with_id(TRAY_ID)
            // The full-colour app icon, not a template: icon.png is a
            // mostly-opaque rounded square, which as a macOS template image
            // would render as a solid blob. A monochrome asset would be
            // needed for a proper template icon.
            .icon(icon)
            .tooltip("ymux")
            .menu(&menu(app, "Open ymux", "Quit")?)
            // Left click shows the window (below); the menu is on right click.
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show_main(tray.app_handle());
                }
            })
            .build(app)?;
        Ok(())
    })();
    match result {
        Ok(()) => gate.set_tray_present(true),
        Err(e) => {
            gate.set_tray_present(false);
            tracing::warn!(error = %e, "tray icon unavailable; closing the window will quit");
        }
    }
}

/// Show, unminimize and focus the main window.
pub fn show_main(app: &AppHandle) {
    // Cmd+H (the app menu's Hide) hides the whole app, not the window.
    #[cfg(target_os = "macos")]
    let _ = app.show();
    let Some(w) = app.get_webview_window(MAIN) else {
        return;
    };
    let _ = w.unminimize();
    let _ = w.show();
    let hidden = std::mem::take(&mut *HIDDEN_BROWSERS.lock().unwrap_or_else(|e| e.into_inner()));
    for label in hidden {
        if let Some(b) = app.get_webview_window(&label) {
            let _ = b.show();
        }
    }
    let _ = w.set_focus();
    let _ = app.emit_to(MAIN, SHOWN_EVENT, ());
}

fn hide_main(window: &Window) {
    if let Err(e) = window.hide() {
        tracing::warn!(error = %e, "failed to hide the main window");
        return;
    }
    // Native browser windows are owned by main, and on Windows hiding the
    // owner (SW_HIDE) leaves owned windows on screen: hide them too.
    let app = window.app_handle();
    {
        let mut hidden = HIDDEN_BROWSERS.lock().unwrap_or_else(|e| e.into_inner());
        for (label, b) in app.webview_windows() {
            if label.starts_with("browser-") && b.is_visible().unwrap_or(false) && b.hide().is_ok()
            {
                hidden.push(label);
            }
        }
    }
    let _ = app.emit_to(MAIN, HIDDEN_EVENT, ());
}

/// The main page started (re)loading: its quit listener is gone until it
/// arms again, so Quit must not wait on it.
pub fn main_page_loading(webview: &Webview) {
    if webview.label() == MAIN {
        webview.app_handle().state::<QuitGate>().disarm();
    }
}

/// A real quit: ask the frontend's unsaved-editors prompt, or exit now.
pub fn request_quit(app: &AppHandle) {
    match app.state::<QuitGate>().request_quit(Instant::now()) {
        QuitStart::AskFrontend => {
            if let Err(e) = app.emit_to(MAIN, QUIT_REQUESTED_EVENT, ()) {
                // Nobody will answer: never leave ymux unquittable.
                tracing::warn!(error = %e, "quit request not delivered; exiting");
                let _ = app.state::<QuitGate>().answer(true);
                app.exit(0);
            }
        }
        // The prompt is already up somewhere: bring it forward.
        QuitStart::AlreadyAsking => show_main(app),
        QuitStart::ExitNow => app.exit(0),
    }
}

/// Route a menu event (tray menu, macOS app-menu Quit). A second Quit while
/// the prompt is up is swallowed by the gate.
pub fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        MENU_OPEN => show_main(app),
        MENU_QUIT | APP_MENU_QUIT => request_quit(app),
        _ => {}
    }
}

/// `WindowEvent::CloseRequested` on any window. Only the main window is
/// kept alive; native browser windows (`browser-*`) close normally.
pub fn on_close_requested(window: &Window, api: &tauri::CloseRequestApi) {
    if window.label() != MAIN {
        return;
    }
    match window
        .app_handle()
        .state::<QuitGate>()
        .on_main_close(Instant::now())
    {
        CloseAction::Allow => {}
        CloseAction::Ignore => api.prevent_close(),
        CloseAction::HideToTray => {
            api.prevent_close();
            hide_main(window);
        }
        CloseAction::Quit => {
            api.prevent_close();
            request_quit(window.app_handle());
        }
    }
}

/// Replace the tray menu labels and tooltip with the frontend's
/// translations. Called after load and on every language change.
#[tauri::command]
pub fn set_tray_labels(
    webview: Webview,
    request: Request<'_>,
    open: String,
    quit: String,
    tooltip: String,
) -> Result<(), String> {
    guard_local(&webview, &request, "set_tray_labels").map_err(|e| e.to_string())?;
    let app = webview.app_handle();
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    let open = clean_label(&open).unwrap_or_else(|| "Open ymux".into());
    let quit = clean_label(&quit).unwrap_or_else(|| "Quit".into());
    let menu = menu(app, &open, &quit).map_err(|e| e.to_string())?;
    tray.set_menu(Some(menu)).map_err(|e| e.to_string())?;
    if let Some(tip) = clean_label(&tooltip) {
        tray.set_tooltip(Some(tip)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The frontend's `ymux://quit-requested` listener is subscribed: from now
/// on Quit asks it instead of exiting at once.
#[tauri::command]
pub fn quit_guard_ready(webview: Webview, request: Request<'_>) -> Result<(), String> {
    guard_local(&webview, &request, "quit_guard_ready").map_err(|e| e.to_string())?;
    webview.app_handle().state::<QuitGate>().arm();
    Ok(())
}

/// The frontend received `ymux://quit-requested` (sent before its prompt):
/// the page is alive, so a second Quit while it asks is swallowed rather
/// than taken for a hung page.
#[tauri::command]
pub fn ack_quit(webview: Webview, request: Request<'_>) -> Result<(), String> {
    guard_local(&webview, &request, "ack_quit").map_err(|e| e.to_string())?;
    webview.app_handle().state::<QuitGate>().ack();
    Ok(())
}

/// The frontend's answer to `ymux://quit-requested`: `proceed` exits the
/// app (through `ExitRequested` → `final_flush`), otherwise the quit is
/// cancelled and ymux keeps running. Returns whether the app is exiting —
/// false for a cancel, or for an answer with no quit pending.
#[tauri::command]
pub fn answer_quit(webview: Webview, request: Request<'_>, proceed: bool) -> Result<bool, String> {
    guard_local(&webview, &request, "answer_quit").map_err(|e| e.to_string())?;
    let app = webview.app_handle();
    let exiting = app.state::<QuitGate>().answer(proceed);
    if exiting {
        app.exit(0);
    }
    Ok(exiting)
}

/// The command palette's "Quit ymux": the same real quit as the tray's
/// Quit (on Windows the only keyboard way to quit, now that closing the
/// window hides it).
#[tauri::command]
pub fn quit_app(webview: Webview, request: Request<'_>) -> Result<(), String> {
    guard_local(&webview, &request, "quit_app").map_err(|e| e.to_string())?;
    request_quit(webview.app_handle());
    Ok(())
}

/// Bring the main window forward — the quit prompt needs to be seen when
/// Quit came from the tray while the window was hidden.
#[tauri::command]
pub fn show_main_window(webview: Webview, request: Request<'_>) -> Result<(), String> {
    guard_local(&webview, &request, "show_main_window").map_err(|e| e.to_string())?;
    show_main(webview.app_handle());
    Ok(())
}
