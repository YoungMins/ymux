// Recently closed panes, kept so "Reopen closed pane" can bring one back.
// A closed pane's PTY is gone at once, but its saved scrollback and agent
// session are only deleted when its entry expires or falls off the stack.
// Pure: WorkspaceManager owns the clock and does the deleting.

import type { PaneSpec, Uuid } from "../types";

export const MAX_CLOSED = 5;
export const CLOSED_GRACE_MS = 10 * 60 * 1000;

/// localStorage key of the ids whose files may still be on disk.
export const PENDING_KEY = "ymux.closedPanes.pending";

export interface ClosedPane {
  spec: PaneSpec;
  closedAt: number;
}

export class ClosedPaneStack {
  private items: ClosedPane[] = [];

  constructor(
    private readonly max = MAX_CLOSED,
    private readonly graceMs = CLOSED_GRACE_MS,
  ) {}

  /// Remember a closed pane. Returns the entries pushed out (the oldest ones
  /// beyond `max`), whose files the caller must now delete.
  push(entry: ClosedPane): ClosedPane[] {
    this.items.push(entry);
    return this.items.splice(0, Math.max(0, this.items.length - this.max));
  }

  /// The most recently closed pane that is still within its grace period.
  /// Expired entries are left for `expire` so the caller can delete them.
  pop(now: number): ClosedPane | null {
    for (let i = this.items.length - 1; i >= 0; i--) {
      if (now - this.items[i].closedAt < this.graceMs) return this.items.splice(i, 1)[0];
    }
    return null;
  }

  /// Remove and return every entry past its grace period.
  expire(now: number): ClosedPane[] {
    const gone = this.items.filter((e) => now - e.closedAt >= this.graceMs);
    if (gone.length) this.items = this.items.filter((e) => now - e.closedAt < this.graceMs);
    return gone;
  }

  /// Epoch ms the next entry expires, or null when empty.
  nextExpiry(): number | null {
    return this.items.length ? Math.min(...this.items.map((e) => e.closedAt)) + this.graceMs : null;
  }

  /// Whether `pop(now)` would find something.
  canReopen(now: number): boolean {
    return this.items.some((e) => now - e.closedAt < this.graceMs);
  }

  get ids(): Uuid[] {
    return this.items.map((e) => e.spec.id);
  }

  get size(): number {
    return this.items.length;
  }
}

/// The persisted id list, tolerant of anything a past version or a hand edit
/// left behind.
export function parsePending(raw: string | null): Uuid[] {
  if (!raw) return [];
  try {
    const v: unknown = JSON.parse(raw);
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}
