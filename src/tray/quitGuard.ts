// The frontend half of a real quit (src-tauri/src/tray.rs, quit_gate.rs).
// Closing the window only hides ymux to its tray; Quit (tray menu, macOS
// Cmd+Q) emits `ymux://quit-requested`, main.ts runs `runQuitGuard`, and
// answers the backend with the result.
//
// Any failure in the guard answers "go ahead": a bug here must never leave
// the user unable to quit. The editors' local drafts are the safety net for
// that case, so the ones still in their 2 s debounce are written first.

export interface QuitGuardDeps {
  hasUnsavedEditors(): boolean;
  /// Bring the (possibly hidden) window forward so the prompt can be seen.
  showWindow(): Promise<void>;
  /// The unsaved-editors prompt. Resolves true to go ahead.
  confirmCloseAll(): Promise<boolean>;
  flushDrafts(): Promise<void>;
  /// Write the layout now. `app.exit` may tear the page down without a
  /// `beforeunload`, so the debounced save must not be left pending.
  flushLayout(): Promise<void>;
}

/// Wait for `p`, but never longer than `ms` — a hung IPC cannot hold the quit.
export function bounded(p: Promise<unknown>, ms: number): Promise<void> {
  return Promise.race([
    p.then(
      () => {},
      () => {},
    ),
    new Promise<void>((r) => setTimeout(r, ms)),
  ]);
}

/// Resolves true when the app may exit.
export async function runQuitGuard(deps: QuitGuardDeps, timeoutMs = 1500): Promise<boolean> {
  let ok: boolean;
  try {
    if (deps.hasUnsavedEditors()) await bounded(deps.showWindow(), timeoutMs);
    ok = await deps.confirmCloseAll();
  } catch (e) {
    console.error("quit guard failed; quitting anyway", e);
    ok = true;
    await bounded(deps.flushDrafts(), timeoutMs);
  }
  if (ok) await bounded(deps.flushLayout(), timeoutMs);
  return ok;
}
