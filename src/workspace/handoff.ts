// "Switch account" hand-off: a note meant for the agent that starts in the
// respawned shell. Pure decision; WorkspaceManager owns the map and the queue.
//
// The old lead can linger in the snapshot after the kill, so the note is only
// queued once an agent that cannot be the old one is in the pane: the old lead
// was seen gone first, or a scan sampled well after the respawn still shows a
// lead.

export interface Handoff {
  text: string;
  /// When the switch began (epoch ms).
  startedAt: number;
  /// When the new shell was spawned (epoch ms); null while the respawn runs.
  respawnedAt: number | null;
  /// No lead was seen in the pane since the switch began.
  seenGone: boolean;
}

/// Dropped when no agent turned up by then.
export const HANDOFF_TTL_MS = 120_000;
/// A scan sampled this long after the respawn still showing a lead has the
/// new agent (the killed one is long gone from the process list).
export const HANDOFF_SCAN_GRACE_MS = 5_000;

export type HandoffStep = "wait" | "queue" | "drop";

/// Decide for one hand-off given the pane's current lead and the sample time
/// of the newest process scan. Records a gone lead in `h.seenGone`.
export function handoffStep(h: Handoff, hasLead: boolean, snapshotAt: number, now: number): HandoffStep {
  if (now - h.startedAt > HANDOFF_TTL_MS) return "drop";
  if (!hasLead) {
    h.seenGone = true;
    return "wait";
  }
  if (h.respawnedAt === null) return "wait";
  if (h.seenGone) return "queue";
  return snapshotAt >= h.respawnedAt + HANDOFF_SCAN_GRACE_MS ? "queue" : "wait";
}
