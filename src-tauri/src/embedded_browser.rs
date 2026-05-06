//! Tauri commands for managing embedded child webview browser panes.
//!
//! Uses Tauri 2's `Window::add_child` + `WebviewBuilder` to embed a webview
//! as a true child of the main window. Unlike the legacy `WebviewWindow`
//! approach in `webview.rs`, child webviews are parented at the OS level —
//! no polling is needed, z-order and virtual-desktop behaviour are correct,
//! and sites that block iframes (X-Frame-Options) work without restriction.
//!
//! THREADING: Like `webview.rs`, all Tauri/wry operations are dispatched to
//! the main thread via `app.run_on_main_thread`. Calling these directly from
//! the IPC worker thread on Windows causes the reply to hang.

use std::sync::Mutex;

use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, State, WebviewBuilder, WebviewUrl, Window,
};

/// Tracks active embedded browser webview labels so the exit handler can
/// close them even though they are not `WebviewWindow`s (and therefore
/// are not returned by `Manager::webview_windows()`).
pub struct EmbeddedBrowserRegistry {
    pub labels: Mutex<Vec<String>>,
}

impl Default for EmbeddedBrowserRegistry {
    fn default() -> Self {
        Self {
            labels: Mutex::new(Vec::new()),
        }
    }
}

fn eb_label(id: &str) -> String {
    format!("eb-{}", id)
}

// async so the IPC response is returned to the frontend before
// run_on_main_thread occupies the main thread with add_child(). When this
// was a sync command, WebView2 needed the main thread to deliver the IPC
// response, but run_on_main_thread was already holding it — causing the
// webview to never initialize (gray screen).
#[tauri::command]
pub async fn create_embedded_browser(
    app: AppHandle,
    state: State<'_, EmbeddedBrowserRegistry>,
    id: String,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let label = eb_label(&id);
    let parsed_url: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;
    let parsed_url2 = parsed_url.clone();
    let escaped = url.replace('\\', "\\\\").replace('"', "\\\"");
    // Initialization script: redirect from about:blank to the target URL in
    // case add_child's WebviewUrl doesn't trigger navigation on Windows.
    let init_js = format!(
        "if (!location.href || location.href === 'about:blank') {{ location.replace(\"{escaped}\"); }}"
    );

    if let Ok(mut labels) = state.labels.lock() {
        labels.push(label.clone());
    }

    let app_spawn = app.clone();
    let app_inner = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = app_spawn.run_on_main_thread(move || {
            let main: Window<_> = match app_inner.get_window("main") {
                Some(w) => w,
                None => {
                    tracing::error!(label = %label, "create_embedded_browser: main window not found");
                    return;
                }
            };
            let builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed_url))
                .initialization_script(&init_js);
            match main.add_child(
                builder,
                PhysicalPosition::new(x as i32, y as i32),
                PhysicalSize::new(width.max(1.0) as u32, height.max(1.0) as u32),
            ) {
                Ok(wv) => {
                    tracing::info!(label = %label, x, y, width, height, "embedded browser created");
                    if let Err(e) = wv.navigate(parsed_url2) {
                        tracing::warn!(label = %label, error = %e, "post-create navigate failed");
                    }
                }
                Err(e) => tracing::error!(label = %label, error = %e, "embedded browser create failed"),
            }
        });
    });

    Ok(())
}

#[tauri::command]
pub fn destroy_embedded_browser(
    app: AppHandle,
    state: State<'_, EmbeddedBrowserRegistry>,
    id: String,
) -> Result<(), String> {
    let label = eb_label(&id);
    let app2 = app.clone();
    let label2 = label.clone();

    if let Ok(mut labels) = state.labels.lock() {
        labels.retain(|l| l != &label);
    }

    app.run_on_main_thread(move || {
        if let Some(wv) = app2.get_webview(&label2) {
            if let Err(e) = wv.close() {
                tracing::warn!(label = %label2, error = %e, "embedded browser close failed");
            } else {
                tracing::info!(label = %label2, "embedded browser destroyed");
            }
        }
    })
    .map_err(|e| format!("dispatch failed: {e}"))?;

    Ok(())
}

#[tauri::command]
pub fn navigate_embedded_browser(app: AppHandle, id: String, url: String) -> Result<(), String> {
    let label = eb_label(&id);
    let parsed: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;
    let app2 = app.clone();

    app.run_on_main_thread(move || {
        if let Some(wv) = app2.get_webview(&label) {
            if let Err(e) = wv.navigate(parsed) {
                tracing::warn!(label = %label, error = %e, "embedded browser navigate failed");
            }
        } else {
            tracing::warn!(label = %label, "navigate: embedded browser not found");
        }
    })
    .map_err(|e| format!("dispatch failed: {e}"))?;

    Ok(())
}

#[tauri::command]
pub fn set_embedded_browser_bounds(
    app: AppHandle,
    id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let label = eb_label(&id);
    let app2 = app.clone();

    app.run_on_main_thread(move || {
        if let Some(wv) = app2.get_webview(&label) {
            let _ = wv.set_position(PhysicalPosition::new(x as i32, y as i32));
            let _ = wv.set_size(PhysicalSize::new(
                width.max(1.0) as u32,
                height.max(1.0) as u32,
            ));
        }
    })
    .map_err(|e| format!("dispatch failed: {e}"))?;

    Ok(())
}
