// Holds a cwd-follow navigation while the user is typing in the dock's list.
//
// The spec's replacement for ydir's `PendingDir` (deleted in step 2): swap
// the listing under someone mid-keystroke and their next Delete or F2 acts
// on a row of a different folder. So a followed directory lands only when
// the list is not focused, or no key has gone to it for `QUIET_MS`. Only
// the latest directory is kept. Clock and timer are injected so the rule is
// tested without a DOM or real time.

export const QUIET_MS = 300;

/// How long to hold a follow, in ms; 0 means apply now.
export function followDelay(now: number, lastKeyAt: number | null, listFocused: boolean): number {
  if (!listFocused || lastKeyAt === null) return 0;
  return Math.max(0, QUIET_MS - (now - lastKeyAt));
}

export interface FollowGateDeps {
  now: () => number;
  focused: () => boolean;
  apply: (dir: string) => void;
  /// Schedule `fn` after `ms`; returns a cancel function.
  setTimer: (fn: () => void, ms: number) => () => void;
}

export class FollowGate {
  private lastKeyAt: number | null = null;
  private pending: string | null = null;
  private cancelTimer: (() => void) | null = null;

  constructor(private readonly d: FollowGateDeps) {}

  /// A key went to the list.
  onKey(): void {
    this.lastKeyAt = this.d.now();
  }

  /// The followed pane moved to `dir`.
  offer(dir: string): void {
    this.pending = dir;
    this.check();
  }

  /// Re-evaluate a held directory now (focus left the list).
  recheck(): void {
    this.check();
  }

  /// Forget a held directory (the user navigated on their own).
  cancel(): void {
    this.pending = null;
    this.cancelTimer?.();
    this.cancelTimer = null;
  }

  private check(): void {
    this.cancelTimer?.();
    this.cancelTimer = null;
    if (this.pending === null) return;
    const wait = followDelay(this.d.now(), this.lastKeyAt, this.d.focused());
    if (wait === 0) {
      const dir = this.pending;
      this.pending = null;
      this.d.apply(dir);
      return;
    }
    this.cancelTimer = this.d.setTimer(() => {
      this.cancelTimer = null;
      this.check();
    }, wait);
  }
}
