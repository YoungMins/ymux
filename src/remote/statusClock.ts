// Remote server: when a pane entered the status the PHONE is shown. The pane's
// own status machine (output pauses, a click, done -> idle when the PC shows
// the workspace) moves on its own schedule and says nothing about the status
// reported, so the age must be kept against the reported status itself.
//
// The clock runs on every evaluation (the scan heartbeat), whether or not the
// remote link is up, so "since" after the link comes back (or is switched on)
// is when the clock first SAW that status, never a start carried over an
// outage. `observe` is the only writer; `read` has no side effects.
// Pure: the clock is injected.

export class StatusClock {
  private seen = new Map<string, { status: string; at: number }>();

  constructor(private readonly now: () => number = Date.now) {}

  /// Record that `id` is in `status` now, and return when it entered it.
  /// Unchanged status keeps its time; a different one starts now. The first
  /// sighting takes `seed` (when the status is known to have begun earlier,
  /// e.g. the office log's time for a hook agent), else now.
  observe(id: string, status: string, seed: number | null): number {
    const e = this.seen.get(id);
    if (e && e.status === status) return e.at;
    const now = this.now();
    const at = e ? now : seed !== null && seed <= now ? seed : now;
    this.seen.set(id, { status, at });
    return at;
  }

  /// When `id` entered `status`, as last observed; null when the last
  /// observation was of another status (or none): an evaluation is pending.
  read(id: string, status: string): number | null {
    const e = this.seen.get(id);
    return e && e.status === status ? e.at : null;
  }

  /// Forget every pane not in `ids` (closed, or no longer listed).
  retainOnly(ids: Iterable<string>): void {
    const keep = new Set(ids);
    for (const id of [...this.seen.keys()]) if (!keep.has(id)) this.seen.delete(id);
  }

  /// The pane is gone: its next sighting is a first one.
  forget(id: string): void {
    this.seen.delete(id);
  }
}
