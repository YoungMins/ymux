export type PaneStatus = "idle" | "running" | "done" | "attention";

/// Frontend-only, per-pane status. Heuristic by design: `done`/`attention`
/// come from the solid OSC 9 / bell signal (reused from v0.8.18); `running`
/// is inferred from command submission + output activity, cleared after an
/// idle window. There is no reliable cross-shell "waiting for input" signal,
/// so that state is intentionally not modelled.
export class PaneStatusMachine {
  private _status: PaneStatus = "idle";
  private lastActivity = 0;

  constructor(
    private onChange: (s: PaneStatus) => void,
    private idleAfterMs = 4000,
  ) {}

  get status(): PaneStatus {
    return this._status;
  }

  private set(next: PaneStatus): void {
    if (next === this._status) return;
    this._status = next;
    this.onChange(next);
  }

  /// User pressed Enter in this pane — a command likely started.
  onSubmit(now: number): void {
    this.lastActivity = now;
    this.set("running");
  }

  /// The PTY produced output — keep the running window alive.
  onOutput(now: number): void {
    this.lastActivity = now;
  }

  /// OSC 9 / bell fired. `focused` = the pane is currently visible+focused.
  onAttention(focused: boolean): void {
    this.set(focused ? "done" : "attention");
  }

  /// Pane gained focus — clear a pending attention flag.
  onFocus(): void {
    if (this._status === "attention") this.set("idle");
  }

  /// Called on a timer; drops running→idle once the idle window elapses.
  tick(now: number): void {
    if (this._status === "running" && now - this.lastActivity >= this.idleAfterMs) {
      this.set("idle");
    }
  }
}
