// Decides when the file dock's yDir should change directory. It watches two
// inputs, "the active pane changed" and "a pane reported a new cwd", and
// emits the active pane's dir. Emission is debounced, so a burst of `cd`s
// (or fast pane cycling) produces a single ChangeDir, and deduplicated
// against the last dir actually sent. Pure: no DOM, no IPC.

export const FOLLOW_DEBOUNCE_MS = 200;

export class CwdFollow {
  private activeId: string | null = null;
  private pending: string | null = null;
  private lastSent: string | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly send: (dir: string) => void) {}

  /// The pane the user works in changed. `cwd` is its last known dir, or
  /// null when unknown or not a terminal. Null cancels any pending send.
  activePaneChanged(id: string | null, cwd: string | null): void {
    this.activeId = id;
    if (cwd) this.schedule(cwd);
    else this.cancel();
  }

  /// A pane reported a new cwd. Only the active pane's reports count.
  cwdChanged(id: string, cwd: string): void {
    if (id !== this.activeId || !cwd) return;
    this.schedule(cwd);
  }

  /// A new yDir started in `dir`. Treat it as already sent and drop
  /// anything pending.
  reset(dir: string | null): void {
    this.cancel();
    this.lastSent = dir;
  }

  private schedule(dir: string): void {
    this.cancel();
    this.pending = dir;
    this.timer = setTimeout(() => {
      this.timer = null;
      const next = this.pending;
      this.pending = null;
      if (next !== null && next !== this.lastSent) {
        this.lastSent = next;
        this.send(next);
      }
    }, FOLLOW_DEBOUNCE_MS);
  }

  private cancel(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.pending = null;
  }
}
