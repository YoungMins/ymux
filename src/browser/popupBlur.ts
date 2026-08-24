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

// NOTE: this module used to export `promptWithBlur` / `confirmWithBlur`,
// thin wrappers around `window.prompt` / `window.confirm` that bracketed the
// call with pushPopup/popPopup. They are gone: WKWebView shows a JavaScript
// dialog only if the host implements the matching `WKUIDelegate` method, and
// wry implements none — so on macOS both calls returned their "cancelled"
// value without ever drawing anything. `src/ui/Dialog.ts` renders real DOM
// instead and drives the same popup counter itself.
