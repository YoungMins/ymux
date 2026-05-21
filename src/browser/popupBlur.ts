// Native browser webviews (NativeBrowserPane, EmbeddedBrowserPane) are
// OS-level windows that paint above all HTML in the main webview. To keep
// ymux modal popups (Command Palette, Help, Notes, HotKey manager) visible
// while a browser pane is on screen, we momentarily hide every active
// browser webview whenever any popup is open.
//
// Contract:
//   - Each popup calls `pushPopup()` in its show() path and `popPopup()` in
//     its hide() path. Both must be idempotent (guard against double-call).
//   - Each BrowserPane registers a listener via `registerBlurListener` and
//     hides/restores its child webview on the `false → true` and
//     `true → false` count transitions.
//   - A pane spawning while a popup is already open MUST hide itself
//     immediately. Use `isPopupOpen()` after registering to catch up.

type BlurListener = (blurred: boolean) => void;

let openCount = 0;
const listeners = new Set<BlurListener>();

export function isPopupOpen(): boolean {
  return openCount > 0;
}

export function registerBlurListener(cb: BlurListener): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

export function pushPopup(): void {
  openCount += 1;
  if (openCount === 1) {
    for (const cb of listeners) cb(true);
  }
}

export function popPopup(): void {
  if (openCount === 0) return;
  openCount -= 1;
  if (openCount === 0) {
    for (const cb of listeners) cb(false);
  }
}

// Native `window.prompt()` displays a WebView2-level dialog that paints below
// OS-level child webviews (same z-order problem as our HTML modals). Wrap it
// so the dialog is shown with browser panes momentarily hidden. `try/finally`
// guarantees the popup count is decremented even if the call throws.
export function promptWithBlur(
  message: string,
  defaultValue?: string,
): string | null {
  pushPopup();
  try {
    return window.prompt(message, defaultValue);
  } finally {
    popPopup();
  }
}
