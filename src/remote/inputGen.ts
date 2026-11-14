// Remote server: the per-pane input generation (PROTOCOL.md 5.2 `menu_id`).
//
// A counter per pane that advances on EVERY write the page makes to that
// pane's terminal, whatever its source: local keystrokes, paste, the prompt
// queue, the Office view's keys and prompt box, approve-all, a broadcast, a
// remote command. It is bumped at the one place all of them pass through,
// `api.writePane` in `ipc/bridge.ts` (the only caller of the PTY write
// command; `inputGen.test.ts` greps for that), and on every automatic answer
// (`auto-continue:answered`, `remoteWiring.ts`). A menu id is minted for one
// value of it, so anything typed since makes the id stale.
//
// No imports: the bridge loads this, so it must stay a leaf. Counters are
// never reset (a pane id that comes back must not reuse an old value).

const gens = new Map<string, number>();

/// Something is about to be written to `pane`'s terminal (or just was, for an
/// answer the backend gave): advance its generation. Synchronous, and called
/// before the write is issued, so a command that reads the generation after
/// the write cannot see the old one.
export function bumpInput(pane: string): void {
  gens.set(pane, (gens.get(pane) ?? 0) + 1);
}

/// The pane's generation now (0 before anything was written).
export function inputGeneration(pane: string): number {
  return gens.get(pane) ?? 0;
}
