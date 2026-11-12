// Which agents a close would kill, for the confirm dialog. Pure.

import type { AgentSnapshot, Uuid } from "../types";

/// "claude" -> "Claude".
export function agentDisplayName(kind: string): string {
  return kind ? kind[0].toUpperCase() + kind.slice(1) : kind;
}

/// Display names of the lead agents running in `paneIds`, in pane order.
/// Any lead counts, whatever its status.
export function liveAgentNames(agents: AgentSnapshot, paneIds: readonly Uuid[]): string[] {
  const out: string[] = [];
  for (const id of paneIds) {
    const kind = agents[id]?.lead?.kind;
    if (kind) out.push(agentDisplayName(kind));
  }
  return out;
}

/// Same names with repeats folded: ["Claude", "Claude", "Codex"] ->
/// "Claude x2, Codex".
export function joinAgentNames(names: readonly string[]): string {
  const counts = new Map<string, number>();
  for (const n of names) counts.set(n, (counts.get(n) ?? 0) + 1);
  return [...counts].map(([n, c]) => (c > 1 ? `${n} ×${c}` : n)).join(", ");
}
