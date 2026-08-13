export type PaneStatus = "idle" | "running" | "done" | "attention";

/// Frontend-only, per-pane status. Heuristic by design.
///
/// `attention` comes from the solid OSC 9 / bell signal; `running` starts at
/// command submission. Completion is trickier: most CLIs never ring the bell,
/// so "the output stopped" is the only completion evidence available, and the
/// machine treats a quiet period as `done` rather than dropping back to
/// `idle` — otherwise finishing a command looked identical to never having
/// run one. Because that inference can be wrong (a long silent build step),
/// resumed output revokes it.
///
/// `done` is an *unread* marker, so how long it lives depends on whether the
/// user can see the pane: held indefinitely while off-screen (cleared when
/// they finally focus it), expired after `doneHoldMs` while on-screen — long
/// enough to notice, short enough that green doesn't become the resting
/// colour of every pane sitting at a prompt.
///
/// There is no reliable cross-shell "waiting for input" signal, so that state
/// is intentionally not modelled.
export class PaneStatusMachine {
  private _status: PaneStatus = "idle";
  private lastActivity = 0;
  /// When the current `done` began, for the watched-hold expiry.
  private doneSince = 0;
  /// Whether the current `done` was inferred from silence rather than an
  /// explicit bell / OSC 9. Only an inferred `done` may be revoked by new
  /// output — a real completion signal has to survive the trailing repaints a
  /// TUI emits right after it finishes.
  private doneInferred = false;

  constructor(
    private onChange: (s: PaneStatus) => void,
    private idleAfterMs = 4000,
    private doneHoldMs = 8000,
  ) {}

  get status(): PaneStatus {
    return this._status;
  }

  private set(next: PaneStatus): void {
    if (next === this._status) return;
    this._status = next;
    this.onChange(next);
  }

  private enterDone(now: number, inferred: boolean): void {
    this.doneSince = now;
    this.doneInferred = inferred;
    this.set("done");
  }

  /// User pressed Enter in this pane — a command likely started. Also called
  /// for the writes that bypass xterm's `onData` (hotkey bar, `startup_cmd`).
  onSubmit(now: number): void {
    this.lastActivity = now;
    this.doneInferred = false;
    this.set("running");
  }

  /// The PTY produced output — keep the running window alive, and take back a
  /// `done` we only inferred from silence.
  onOutput(now: number): void {
    this.lastActivity = now;
    if (this._status === "done" && this.doneInferred) {
      this.doneInferred = false;
      this.set("running");
    }
  }

  /// OSC 9 / bell fired. `visible` = the pane is on screen right now (window
  /// focused and its workspace the active one) — not the pane's own focus
  /// flag, which nothing lowers on a workspace switch or an alt-tab.
  onAttention(visible: boolean, now: number): void {
    if (visible) this.enterDone(now, false);
    else this.set("attention");
  }

  /// Pane gained focus — the user has now seen whatever it was flagging.
  onFocus(): void {
    if (this._status === "attention" || this._status === "done") {
      this.doneInferred = false;
      this.set("idle");
    }
  }

  /// Called on a timer. Turns a quiet `running` into `done`, and lets an
  /// on-screen `done` fade back to `idle` once it has been up long enough to
  /// see — which is also how an unread `done` clears when the user finally
  /// switches to that workspace. `attention` is never expired here; the loud
  /// signal wants an explicit acknowledgement, so only focus clears it.
  tick(now: number, visible: boolean): void {
    if (this._status === "running") {
      if (now - this.lastActivity >= this.idleAfterMs) this.enterDone(now, true);
      return;
    }
    if (this._status === "done" && visible && now - this.doneSince >= this.doneHoldMs) {
      this.doneInferred = false;
      this.set("idle");
    }
  }
}
