// Is this document the top-level one?
//
// Security-critical, not cosmetic. Framed inside a `browser` pane, ymux's
// own document still has the `main` label and a local Origin (and on
// Windows, the Tauri invoke key), so every `guard_local` command would pass
// for it: a website could boot a second ymux in an iframe — spawning real
// PTYs from the user's config — and clickjack the user into typing into it.
// The CSP's `frame-ancestors 'none'` stops the load in a release build (Tauri
// sends the CSP as a header); `bootGuard.ts` refuses to boot if that is ever
// bypassed, e.g. under the dev server, which sends no CSP header.
//
// Fails closed: an unreadable or missing `top` counts as framed.
export function isTopLevelWindow(win: { top: unknown }): boolean {
  try {
    return win.top != null && win.top === win;
  } catch {
    return false;
  }
}
