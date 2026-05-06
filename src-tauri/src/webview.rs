//! Tauri commands for managing native browser child webview windows.
//!
//! Each browser pane gets its own borderless `WebviewWindow`. The frontend
//! polls the placeholder's screen position and calls `resize_webview` to keep
//! the child window glued to the layout.
//!
//! IMPORTANT: window operations are dispatched to the main thread via
//! `app.run_on_main_thread` and the commands return immediately. Calling
//! `WebviewWindowBuilder::build()` (or `set_position` / `navigate`) directly
//! from the Tauri command worker thread on Windows causes the IPC response
//! to hang because internal main-thread dispatch waits for the window
//! operation to complete, blocking the message pump from processing the
//! IPC reply.

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

// async so the IPC response is sent back to the frontend before
// run_on_main_thread occupies the main thread with build(). When this
// was a sync command, WebView2 needed the main thread to deliver the
// IPC response, but build() was already holding it — causing the
// "createWebview replied OK" message to never appear and blocking all
// subsequent innerPosition() / resize polls.
#[tauri::command]
pub async fn create_webview(
    app: AppHandle,
    id: String,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    user_agent: Option<String>,
) -> Result<(), String> {
    let label = format!("browser-{}", id);
    eprintln!(
        "[webview] create {} url={} pos=({},{}) size=({}x{}) ua={}",
        label,
        url,
        x,
        y,
        width,
        height,
        user_agent
            .as_deref()
            .map(|s| if s.is_empty() { "default" } else { "custom" })
            .unwrap_or("default"),
    );
    let parsed_url: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;
    let escaped = url.replace('\\', "\\\\").replace('"', "\\\"");
    let init_js = format!(
        "if (location.href === 'about:blank') {{ location.replace(\"{}\"); }}",
        escaped
    );

    let app_run = app.clone();
    let app_builder = app.clone();
    let label2 = label.clone();
    tauri::async_runtime::spawn(async move {
        let _ = app_run.run_on_main_thread(move || {
            let main_win = app_builder.get_webview_window("main");
            let builder =
                WebviewWindowBuilder::new(&app_builder, &label2, WebviewUrl::External(parsed_url))
                    .title("ymux browser")
                    .inner_size(width, height)
                    .position(x, y)
                    .decorations(false)
                    .resizable(false)
                    .skip_taskbar(true)
                    .focused(false)
                    .initialization_script(&init_js);
            let builder = match user_agent.as_deref() {
                Some(ua) if !ua.is_empty() => builder.user_agent(ua),
                _ => builder,
            };
            let builder = match main_win {
                Some(ref w) => match builder.owner(w) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("[webview] {} owner() failed: {}", label2, e);
                        return;
                    }
                },
                None => builder,
            };
            match builder.build() {
                Ok(_) => eprintln!("[webview] {} created on main thread", label2),
                Err(e) => eprintln!("[webview] {} create FAILED: {}", label2, e),
            }
        });
    });

    Ok(())
}

#[tauri::command]
pub fn destroy_webview(app: AppHandle, id: String) -> Result<(), String> {
    let label = format!("browser-{}", id);
    let app2 = app.clone();
    app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window(&label) {
            let _ = win.close();
        }
    })
    .map_err(|e| format!("dispatch failed: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn navigate_webview(app: AppHandle, id: String, url: String) -> Result<(), String> {
    let label = format!("browser-{}", id);
    eprintln!("[webview] navigate {} -> {}", label, url);
    let parsed: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;
    let escaped = url.replace('\\', "\\\\").replace('"', "\\\"");
    let app2 = app.clone();
    let label2 = label.clone();

    app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window(&label2) {
            match win.navigate(parsed) {
                Ok(_) => eprintln!("[webview] {} navigate() OK", label2),
                Err(e) => eprintln!("[webview] {} navigate() failed: {}", label2, e),
            }
            // Eval fallback for cases where navigate() is silently a no-op.
            let js = format!("window.location.href = \"{}\";", escaped);
            if let Err(e) = win.eval(&js) {
                eprintln!("[webview] {} eval fallback failed: {}", label2, e);
            }
        } else {
            eprintln!("[webview] navigate: '{}' not found", label2);
        }
    })
    .map_err(|e| format!("dispatch failed: {e}"))?;

    Ok(())
}

#[tauri::command]
pub fn zoom_webview(app: AppHandle, id: String, factor: f64) -> Result<(), String> {
    let label = format!("browser-{}", id);
    let factor = factor.clamp(0.1, 5.0);
    let app2 = app.clone();
    app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window(&label) {
            let js = format!("document.documentElement.style.zoom = '{}';", factor);
            let _ = win.eval(&js);
        }
    })
    .map_err(|e| format!("dispatch failed: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn resize_webview(
    app: AppHandle,
    id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let label = format!("browser-{}", id);
    let app2 = app.clone();

    app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window(&label) {
            let _ = win.set_position(PhysicalPosition::new(x as i32, y as i32));
            let _ = win.set_size(PhysicalSize::new(
                width.max(1.0) as u32,
                height.max(1.0) as u32,
            ));
        }
    })
    .map_err(|e| format!("dispatch failed: {e}"))?;
    Ok(())
}
