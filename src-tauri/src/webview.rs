//! Tauri commands for managing native browser child webviews.
//!
//! Each browser pane gets its own `Webview` **embedded inside the main
//! window** (not a separate OS window). The frontend positions a placeholder
//! `<div>` in the layout, and these commands create / move / resize the
//! native webview to overlay that placeholder. Because the webview lives in
//! the same window, it moves with it — no position sync needed.

use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, WebviewUrl};

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

    let main_window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;

    let parsed_url: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;
    let builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed_url));

    main_window
        .add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|e| format!("failed to create webview: {e}"))?;

    Ok(())
}

#[tauri::command]
pub fn destroy_webview(app: AppHandle, id: String) -> Result<(), String> {
    let label = format!("browser-{}", id);
    if let Some(wv) = app.get_webview(&label) {
        wv.close().map_err(|e| format!("close failed: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn navigate_webview(app: AppHandle, id: String, url: String) -> Result<(), String> {
    let label = format!("browser-{}", id);
    let wv = app
        .get_webview(&label)
        .ok_or_else(|| format!("webview '{label}' not found"))?;

    let parsed: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;
    wv.navigate(parsed)
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
    let wv = app
        .get_webview(&label)
        .ok_or_else(|| format!("webview '{label}' not found"))?;

    wv.set_position(LogicalPosition::new(x, y))
        .map_err(|e| format!("set_position failed: {e}"))?;
    wv.set_size(LogicalSize::new(width, height))
        .map_err(|e| format!("set_size failed: {e}"))?;
    Ok(())
}
