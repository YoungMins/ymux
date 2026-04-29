//! Tauri commands for managing native browser child webview windows.
//!
//! Each browser pane gets its own borderless `WebviewWindow`. The frontend
//! polls the placeholder's screen position and calls `resize_webview` to keep
//! the child window glued to the layout.
//!
//! NOTE: deliberately NOT using `WebviewWindowBuilder::parent` because on
//! Windows that creates an owner-owned relationship that interferes with
//! both manual repositioning and webview navigation. We manage child
//! lifetime manually (closed in main.rs ExitRequested handler).

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
pub fn create_webview(
    app: AppHandle,
    id: String,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let label = format!("browser-{}", id);
    eprintln!("[webview] create {} url={} pos=({},{}) size=({}x{})", label, url, x, y, width, height);

    let parsed_url: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;

    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed_url))
        .title("ymux browser")
        .inner_size(width, height)
        .position(x, y)
        .decorations(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(false)
        .build()
        .map_err(|e| format!("create webview failed: {e}"))?;

    eprintln!("[webview] {} created", label);
    Ok(())
}

#[tauri::command]
pub fn destroy_webview(app: AppHandle, id: String) -> Result<(), String> {
    let label = format!("browser-{}", id);
    eprintln!("[webview] destroy {}", label);
    if let Some(win) = app.get_webview_window(&label) {
        win.close().map_err(|e| format!("close failed: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn navigate_webview(app: AppHandle, id: String, url: String) -> Result<(), String> {
    let label = format!("browser-{}", id);
    eprintln!("[webview] navigate {} -> {}", label, url);

    let win = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("webview '{label}' not found"))?;

    let parsed: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;

    // Try navigate() first.
    match win.navigate(parsed) {
        Ok(_) => {
            eprintln!("[webview] {} navigate() OK", label);
        }
        Err(e) => {
            eprintln!("[webview] {} navigate() failed: {} — trying eval fallback", label, e);
        }
    }

    // ALSO call eval() as a fallback — some Tauri/WebView2 combinations
    // ignore navigate() but accept window.location assignment from JS.
    let escaped = url.replace('\\', "\\\\").replace('"', "\\\"");
    let js = format!("window.location.href = \"{}\";", escaped);
    if let Err(e) = win.eval(&js) {
        eprintln!("[webview] {} eval fallback failed: {}", label, e);
    } else {
        eprintln!("[webview] {} eval fallback OK", label);
    }

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
    let win = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("webview '{label}' not found"))?;

    win.set_position(PhysicalPosition::new(x as i32, y as i32))
        .map_err(|e| format!("set_position failed: {e}"))?;
    win.set_size(PhysicalSize::new(width.max(1.0) as u32, height.max(1.0) as u32))
        .map_err(|e| format!("set_size failed: {e}"))?;
    Ok(())
}
