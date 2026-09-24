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

use tauri::ipc::Request;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, Webview, WebviewBuilder,
    WebviewUrl, Window,
};

use crate::fspath::guard_local;

/// Per-webview JS injected via `initialization_script` so every embedded
/// child can:
///   1. Tell the main webview which `.pane` was clicked/focused (reliable
///      `focusedPaneId` updates that no longer depend on cursor mapping).
///   2. Forward ymux global shortcuts so they still trigger while the
///      child owns OS keyboard focus.
///
/// `{id}` is substituted with the pane UUID. Tauri injects
/// `window.__TAURI_INTERNALS__.invoke` into every webview regardless of
/// capabilities, and ymux's own commands are not ACL-checked (no
/// `AppManifest`), so no capability is needed — and none is granted: the two
/// commands this script calls are the whole of what the page can reach, and
/// each checks the caller with [`guard_embedded_child`]. Keep the shortcut
/// predicate below in sync with `ipc_guard::forwarded_shortcut_key`.
fn child_init_script(id: &str) -> String {
    format!(
        r#"
(function() {{
  var paneId = "{id}";
  // Diagnostic — visible in DevTools if IPC ever fails. Leaves a breadcrumb
  // in document.title before site code can clobber it; site title overrides
  // this almost immediately, which is fine for normal runs.
  try {{ document.title = 'ymux-init:' + (typeof window.__TAURI_INTERNALS__); }} catch (_) {{}}

  function invoke(cmd, args) {{
    // Skip on Chrome's internal error pages — their origin isn't a valid
    // URL, so Tauri's IPC rejects with "Origin header is not a valid URL".
    if (location.protocol === 'chrome-error:' || location.protocol === 'chrome:') {{
      return null;
    }}
    try {{
      if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {{
        var p = window.__TAURI_INTERNALS__.invoke(cmd, args);
        if (p && typeof p.catch === 'function') p.catch(function() {{}});
        return p;
      }}
    }} catch (_) {{}}
    return null;
  }}

  function reportFocus() {{
    invoke('child_webview_focused', {{ id: paneId }});
  }}
  // Fires when the user clicks/taps anywhere in the page or tabs into it.
  window.addEventListener('pointerdown', reportFocus, true);
  window.addEventListener('focus', reportFocus, true);

  function isYmuxShortcut(e) {{
    if (e.ctrlKey && e.altKey && !e.shiftKey && /^Digit[1-9]$/.test(e.code)) return true;
    if (e.ctrlKey && e.altKey && !e.shiftKey && e.code === 'KeyN') return true;
    if (e.ctrlKey && e.shiftKey && !e.altKey && /^Key[HVZPRET]$/.test(e.code)) return true;
    if (e.ctrlKey && e.shiftKey && !e.altKey && (e.code === 'BracketLeft' || e.code === 'BracketRight')) return true;
    if (e.ctrlKey && !e.altKey && e.code === 'Tab') return true;
    return false;
  }}
  window.addEventListener('keydown', function(e) {{
    if (!isYmuxShortcut(e)) return;
    e.preventDefault();
    e.stopPropagation();
    invoke('forward_keystroke', {{
      code: e.code,
      ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey
    }});
  }}, true);
}})();
"#
    )
}

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
// Each argument is a field of the frontend `invoke` payload.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_embedded_browser(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    state: State<'_, EmbeddedBrowserRegistry>,
    id: String,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    guard_local(&webview, &request, "create_embedded_browser").map_err(|e| e.to_string())?;
    let label = eb_label(&id);
    let parsed_url: url::Url = url.parse().map_err(|e| format!("invalid URL: {e}"))?;
    let parsed_url2 = parsed_url.clone();
    let escaped = url.replace('\\', "\\\\").replace('"', "\\\"");
    // Initialization script: redirect from about:blank to the target URL in
    // case add_child's WebviewUrl doesn't trigger navigation on Windows.
    let init_js = format!(
        "if (!location.href || location.href === 'about:blank') {{ location.replace(\"{escaped}\"); }}\n{}",
        child_init_script(&id)
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
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    state: State<'_, EmbeddedBrowserRegistry>,
    id: String,
) -> Result<(), String> {
    guard_local(&webview, &request, "destroy_embedded_browser").map_err(|e| e.to_string())?;
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
pub fn navigate_embedded_browser(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    id: String,
    url: String,
) -> Result<(), String> {
    guard_local(&webview, &request, "navigate_embedded_browser").map_err(|e| e.to_string())?;
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

// Each argument is a field of the frontend `invoke` payload.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn set_embedded_browser_bounds(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    guard_local(&webview, &request, "set_embedded_browser_bounds").map_err(|e| e.to_string())?;
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

/// The guard for the commands an `eb-*` child page calls (see
/// [`crate::ipc_guard::EMBEDDED_CHILD_COMMANDS`] for the list and why).
///
/// Accepts only a webview whose label is `eb-<canonical uuid>` **and** is
/// still in the registry — i.e. an embedded browser ymux created and has not
/// destroyed. Returns that pane's id, which is the only pane id the command
/// may act on: whatever the page claims in its arguments is not trusted.
///
/// It does not look at `Origin`: the page is a website by design, so its
/// origin says nothing. The label, which Tauri takes from the webview that
/// delivered the message and the page cannot choose, is the identity.
pub fn guard_embedded_child(
    webview: &Webview,
    registry: &State<'_, EmbeddedBrowserRegistry>,
    cmd: &str,
) -> Result<uuid::Uuid, String> {
    let label = webview.label();
    let Some(pane) = crate::ipc_guard::embedded_child_pane_id(label) else {
        return Err(format!(
            "{cmd}: only an embedded browser pane may call this (label {label:?})"
        ));
    };
    let live = registry
        .labels
        .lock()
        .map(|l| l.iter().any(|x| x == label))
        .unwrap_or(false);
    if !live {
        return Err(format!("{cmd}: no live embedded browser {label:?}"));
    }
    Ok(pane)
}

// Receive a click/focus signal from a child webview's init script. Emits
// `ymux:child-focused` so the frontend can set `focusedPaneId` to this
// pane reliably (replacing the cursor-mapping heuristic in window.blur).
//
// `id` must name the calling webview's own pane: a page may only report
// itself as focused, never steer focus to another pane.
#[tauri::command]
pub fn child_webview_focused(
    webview: Webview,
    registry: State<'_, EmbeddedBrowserRegistry>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    let pane = guard_embedded_child(&webview, &registry, "child_webview_focused")?;
    if id != pane.to_string() {
        return Err("child_webview_focused: id does not match the calling pane".into());
    }
    app.emit("ymux:child-focused", pane.to_string())
        .map_err(|e| format!("emit failed: {e}"))?;
    Ok(())
}

// Receive a synthetic keystroke from a child webview's init-script
// forwarder. Focuses the main webview (so the popup that this shortcut
// may open can take input focus), then emits an event the main webview
// listens for and replays as a `KeyboardEvent`.
//
// Only the exact (code, modifiers) combinations in
// `ipc_guard::forwarded_shortcut_key` are accepted, and the `key` the main
// window sees is derived from that table — a page-supplied `key` is not even
// read — so a page cannot synthesize arbitrary keystrokes or pass one
// shortcut's code off as another's key.
#[tauri::command]
pub fn forward_keystroke(
    webview: Webview,
    registry: State<'_, EmbeddedBrowserRegistry>,
    app: AppHandle,
    code: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Result<(), String> {
    guard_embedded_child(&webview, &registry, "forward_keystroke")?;
    let Some(key) = crate::ipc_guard::forwarded_shortcut_key(&code, ctrl, shift, alt) else {
        return Err("forward_keystroke: not a forwardable ymux shortcut".into());
    };
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_focus();
    }
    let payload = serde_json::json!({
        "key": key,
        "code": code,
        "ctrl": ctrl,
        "shift": shift,
        "alt": alt,
    });
    app.emit("ymux:forwarded-key", payload)
        .map_err(|e| format!("emit failed: {e}"))?;
    Ok(())
}

// Hide / restore an embedded child webview by moving it off-screen and
// shrinking it to 1x1 while a ymux popup is open. `Webview` (the
// `add_child`-returned handle) has no portable `set_visible`, so we use
// the position trick instead. On show, the frontend re-emits the real
// bounds via `set_embedded_browser_bounds`.
#[tauri::command]
pub fn set_embedded_browser_visible(
    webview: Webview,
    request: Request<'_>,
    app: AppHandle,
    id: String,
    visible: bool,
) -> Result<(), String> {
    guard_local(&webview, &request, "set_embedded_browser_visible").map_err(|e| e.to_string())?;
    if visible {
        // Frontend is expected to call set_embedded_browser_bounds()
        // immediately after this to restore the real placement.
        return Ok(());
    }
    let label = eb_label(&id);
    let app2 = app.clone();
    app.run_on_main_thread(move || {
        if let Some(wv) = app2.get_webview(&label) {
            let _ = wv.set_position(PhysicalPosition::new(-32000, -32000));
            let _ = wv.set_size(PhysicalSize::new(1, 1));
        }
    })
    .map_err(|e| format!("dispatch failed: {e}"))?;
    Ok(())
}
