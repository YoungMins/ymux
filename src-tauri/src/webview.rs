//! Tauri commands for managing native browser child webview windows.
//!
//! Each browser pane gets its own `WebviewWindow` parented to the main window.
//! The frontend keeps the child window positioned over a placeholder `<div>` by
//! listening for layout changes AND main-window move/resize events.

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

    let parent = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;

    let parsed_url: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;

    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed_url))
        .title("ymux browser")
        .inner_size(width, height)
        .position(x, y)
        .decorations(false)
        .parent(&parent)
        .map_err(|e| format!("set parent failed: {e}"))?
        .build()
        .map_err(|e| format!("create webview failed: {e}"))?;

    Ok(())
}

#[tauri::command]
pub fn destroy_webview(app: AppHandle, id: String) -> Result<(), String> {
    let label = format!("browser-{}", id);
    if let Some(win) = app.get_webview_window(&label) {
        win.close().map_err(|e| format!("close failed: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn navigate_webview(app: AppHandle, id: String, url: String) -> Result<(), String> {
    let label = format!("browser-{}", id);
    let win = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("webview '{label}' not found"))?;

    let parsed: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;
    win.navigate(parsed)
        .map_err(|e| format!("navigate failed: {e}"))?;
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
    win.set_size(PhysicalSize::new(width as u32, height as u32))
        .map_err(|e| format!("set_size failed: {e}"))?;
    Ok(())
}
