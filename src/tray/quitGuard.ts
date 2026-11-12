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
  /// Write every live terminal pane's scrollback now (its save timer may still
  /// be waiting out a quiet period). Bounded like the layout flush.
  flushScrollback?(): Promise<void>;
  /// Write the office's fun stats now (their save is debounced). Synchronous
  /// in the app; a throw is tolerated like the other flushes.
  flushFun?(): void | Promise<void>;
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

/// Run an optional flush, turning a synchronous throw into a rejection.
async function safely(fn: (() => void | Promise<void>) | undefined): Promise<void> {
  await fn?.();
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
  if (ok) {
    await Promise.all([
      bounded(deps.flushLayout(), timeoutMs),
      bounded(safely(deps.flushScrollback), timeoutMs),
      bounded(safely(deps.flushFun), timeoutMs),
    ]);
  }
  return ok;
}
