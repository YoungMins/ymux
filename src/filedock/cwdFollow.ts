// Decides when the file dock's yDir should change directory. It watches two
// inputs, "the active pane changed" and "a pane reported a new cwd", and
// emits the active pane's dir. Emission is debounced, so a burst of `cd`s
// (or fast pane cycling) produces a single ChangeDir, and deduplicated
// against the last dir actually sent. Pure: no DOM, no IPC.

export const FOLLOW_DEBOUNCE_MS = 200;

/// Key for deduping one cwd against the last one sent. **Comparison only:**
/// `send()` always gets the raw string the pane reported, never this.
///
/// NFC, because macOS hands back decomposed filenames: the same Korean
/// directory arrives composed from a shell's own OSC 7 payload and
/// decomposed from a path the filesystem produced, and an undeduped pair
/// re-navigates yDir for nothing.
///
/// Deliberately *only* NFC, unlike the Rust `ypath::comparison_key` this
/// mirrors. Case and separator folding is a property of the path's syntax,
/// and the Rust side has already applied the full rule in `CwdChange`
/// before a cwd is ever emitted to the frontend; repeating half of it here
/// with a different notion of which paths are case-insensitive would be the
/// one way to make the two layers disagree.
function dedupeKey(dir: string): string {
  return dir.normalize("NFC");
}

export class CwdFollow {
  private activeId: string | null = null;
  private pending: string | null = null;
  private lastSentKey: string | null = null;
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
    this.lastSentKey = dir === null ? null : dedupeKey(dir);
  }

  private schedule(dir: string): void {
    this.cancel();
    this.pending = dir;
    this.timer = setTimeout(() => {
      this.timer = null;
      const next = this.pending;
      this.pending = null;
      if (next === null) return;
      const key = dedupeKey(next);
      if (key === this.lastSentKey) return;
      this.lastSentKey = key;
      // `next`, not `key`: yDir has to open this, not compare it.
      this.send(next);
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
