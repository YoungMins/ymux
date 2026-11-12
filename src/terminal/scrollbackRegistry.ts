// Live terminal panes register a "save my scrollback now" function here, so a
// real quit (`tray/quitGuard.ts`) can flush them all without importing
// WorkspaceManager or TerminalPane (which would be a cycle).

const flushers = new Set<() => Promise<void>>();

/// Returns the unregister function.
export function registerScrollbackFlusher(fn: () => Promise<void>): () => void {
  flushers.add(fn);
  return () => {
    flushers.delete(fn);
  };
}

/// Run every registered flusher. Never rejects: one pane failing must not
/// stop the others, nor the quit.
export async function flushAllScrollback(): Promise<void> {
  await Promise.allSettled(
    [...flushers].map(async (f) => {
      await f();
    }),
  );
}
