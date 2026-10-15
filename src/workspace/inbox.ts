import type { PaneStatus } from "../terminal/paneStatus";

/// Pure attention-inbox model: "unread" is exactly PaneStatus attention/done
/// (focusing a pane clears it), so there is no second read store.

export type InboxStatus = "attention" | "done";

export interface InboxItem {
  paneId: string;
  wsId: number;
  wsLabel: string;
  paneLabel: string;
  agentKind: string | null;
  status: InboxStatus;
  since: number;
}

/// One row per pane, any status; `buildInbox` filters.
export interface InboxCandidate extends Omit<InboxItem, "status"> {
  status: PaneStatus;
}

export function buildInbox(input: readonly InboxCandidate[]): InboxItem[] {
  const rank = (s: InboxStatus): number => (s === "attention" ? 0 : 1);
  return input
    .filter((c): c is InboxCandidate & { status: InboxStatus } => c.status === "attention" || c.status === "done")
    .map((c) => ({ ...c }))
    .sort((a, b) => rank(a.status) - rank(b.status) || a.since - b.since);
}

/// Next pane to jump to after `currentPaneId`, wrapping. Attention items win
/// over done ones; the current pane is only returned when it is the sole item.
export function nextAttention(items: readonly InboxItem[], currentPaneId: string | null): string | null {
  if (items.length === 0) return null;
  const idx = currentPaneId === null ? -1 : items.findIndex((i) => i.paneId === currentPaneId);
  if (idx < 0) return items[0].paneId;
  const order = [...items.slice(idx + 1), ...items.slice(0, idx)];
  const pick = order.find((i) => i.status === "attention") ?? order[0];
  return pick ? pick.paneId : items[idx].paneId;
}

/// What is remembered about an acknowledged item. Labels are not stored: they
/// are resolved from the live tree when the menu opens, so a renamed or closed
/// pane never shows stale text.
export interface ReadEntry {
  paneId: string;
  readAt: number;
  status: InboxStatus;
  agentKind: string | null;
}

/// A `ReadEntry` with its labels resolved against the live tree.
export interface ReadItem extends ReadEntry {
  wsLabel: string;
  paneLabel: string;
}

export const READ_HISTORY_CAP = 20;

/// Runtime-only ring of recently read items, newest first.
export class ReadHistory {
  private entries: ReadEntry[] = [];

  push(entry: ReadEntry): void {
    this.entries = [entry, ...this.entries.filter((e) => e.paneId !== entry.paneId)].slice(0, READ_HISTORY_CAP);
  }

  remove(paneId: string): void {
    this.entries = this.entries.filter((e) => e.paneId !== paneId);
  }

  list(): ReadEntry[] {
    return [...this.entries];
  }
}

/// Accessible bell label, e.g. "Inbox: 2 need attention, 3 done". `summary`
/// carries `{attention}` and `{done}`; with nothing pending it is just the title.
export function inboxSummary(
  counts: { attention: number; done: number },
  text: { none: string; summary: string },
): string {
  if (counts.attention + counts.done === 0) return text.none;
  return text.summary.replace("{attention}", String(counts.attention)).replace("{done}", String(counts.done));
}
