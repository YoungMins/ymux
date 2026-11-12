// Pure model behind the elapsed time, last tool and "stuck" badge on the
// workspace panel's agent rows. The stuck rule is the office control room's
// (`stuckReason`, officeQueue.ts) — thresholds live there only.

import type { AgentStatus } from "../types";
import { stuckReason, type Stuck } from "../office/officeQueue";

/// Compact elapsed time: "45s", "12m", "1h03m". Never negative.
export function elapsedText(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return `${h}h${String(m % 60).padStart(2, "0")}m`;
}

/// What the manager knows about one pane's lead agent, as epoch ms.
export interface AgentClock {
  /// When the agent entered its current status.
  since: number | null;
  /// Last terminal output in the pane.
  outputAt: number | null;
  /// When the current tool call started.
  toolSince: number | null;
}

export interface AgentBadges {
  /// null when the start of the status is unknown.
  elapsed: string | null;
  tool: string | null;
  stuck: Stuck | null;
}

export function agentBadges(
  status: AgentStatus,
  tool: string | null,
  clock: AgentClock,
  now: number,
): AgentBadges {
  return {
    elapsed: clock.since === null ? null : elapsedText(now - clock.since),
    tool,
    stuck: stuckReason({
      working: status === "working",
      tool,
      outputAt: clock.outputAt,
      toolSince: clock.toolSince,
      now,
    }),
  };
}
