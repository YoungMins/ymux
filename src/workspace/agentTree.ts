// Pure model behind the workspace panel's Workspace › Pane › Agent tree.
// No DOM, no IPC: WorkspacePanel renders whatever `buildAgentTree` returns.

import type {
  AgentSnapshot,
  AgentStatus,
  PaneAgents,
  PaneSpec,
  Uuid,
  Workspace,
} from "../types";
import type { PaneStatus } from "../terminal/paneStatus";
import { findPane } from "../layout/LayoutTree";
import { paneGroups } from "../layout/tabs";
import { tabLabel } from "../terminal/tabLabel";

/// localStorage key for per-workspace expansion (spec §1).
export const EXPANDED_KEY = "ymux.workspaceTree.expanded";

/// Workspace id (as string) → expanded. Missing ids count as expanded.
export type ExpandedMap = Record<string, boolean>;

export interface AgentRow {
  /// Stable per-row key: `${paneId}:lead` / `${paneId}:sub:${id}`.
  key: string;
  /// Lead: agent kind ("claude"). Subagent: its agent_type.
  label: string;
  status: AgentStatus;
  /// 1 = lead (or a subagent with no lead), 2 = subagent nested under the lead.
  depth: 1 | 2;
  tool: string | null;
}

/// One tab of a multi-tab pane. Spec §4: the tree becomes Workspace › Pane ›
/// Tab › Agent, and agents attach to the tab (pane id) they run in.
export interface TabRow {
  paneId: Uuid;
  label: string;
  status: PaneStatus;
  active: boolean;
  agents: AgentRow[];
}

export interface PaneRow {
  /// For a tab group, the *active* tab — so clicking the row focuses what is
  /// actually on screen.
  paneId: Uuid;
  label: string;
  status: PaneStatus;
  /// Agents of a plain pane. Always empty for a group: its agents hang off
  /// the tab rows instead.
  agents: AgentRow[];
  /// Empty when the pane has one tab, so today's depth is unchanged.
  tabs: TabRow[];
}

export interface WorkspaceTree {
  wsId: number;
  panes: PaneRow[];
}

/// Localised fallbacks, passed in so this module stays free of i18n state.
export interface TreeLabels {
  terminal: string;
  browser: string;
  subagent: string;
}

/// The backend can't know a process-only agent's status; the pane's own
/// PaneStatusMachine is the best evidence (spec §1, "Process scan").
export function paneStatusToAgentStatus(s: PaneStatus): AgentStatus {
  switch (s) {
    case "running":
      return "working";
    case "attention":
      return "waiting";
    case "done":
      return "done";
    default:
      return "idle";
  }
}

export function paneLabel(spec: PaneSpec, labels: TreeLabels): string {
  if ((spec.pane_kind ?? "terminal") === "terminal") {
    return spec.title || spec.shell || labels.terminal;
  }
  return spec.title || spec.url || labels.browser;
}

export function agentRows(
  paneId: Uuid,
  entry: PaneAgents | undefined,
  paneStatus: PaneStatus,
  labels: TreeLabels,
): AgentRow[] {
  if (!entry) return [];
  const rows: AgentRow[] = [];
  const lead = entry.lead;
  if (lead) {
    rows.push({
      key: `${paneId}:lead`,
      label: lead.kind,
      status: lead.source === "process" ? paneStatusToAgentStatus(paneStatus) : lead.status,
      depth: 1,
      tool: lead.tool,
    });
  }
  const subDepth: 1 | 2 = lead ? 2 : 1;
  for (const s of entry.subagents) {
    rows.push({
      key: `${paneId}:sub:${s.id}`,
      label: s.agent_type || labels.subagent,
      status: s.status,
      depth: subDepth,
      tool: null,
    });
  }
  return rows;
}

/// Every pane of every workspace (config order, depth-first within a layout),
/// each with its tabs and agents. Agent entries for pane ids that are in no
/// layout are simply never looked up.
///
/// `processLabels` is the backend's `panes:labels` snapshot; it only decides
/// what a *tab* is called, because a plain pane row keeps showing its title or
/// shell exactly as before.
export function buildAgentTree(
  workspaces: Workspace[],
  agents: AgentSnapshot,
  statusOf: (paneId: Uuid) => PaneStatus,
  labels: TreeLabels,
  processLabels: Record<Uuid, string> = {},
): WorkspaceTree[] {
  return workspaces.map((ws) => ({
    wsId: ws.id,
    panes: paneGroups(ws.root).map((group) => {
      const active = group.members[group.activeIndex] ?? group.members[0];
      const activeStatus = statusOf(active.id);
      if (group.members.length < 2) {
        return {
          paneId: active.id,
          label: paneLabel(active, labels),
          status: activeStatus,
          agents: agentRows(active.id, agents[active.id], activeStatus, labels),
          tabs: [],
        };
      }
      return {
        paneId: active.id,
        label: paneLabel(active, labels),
        status: activeStatus,
        agents: [],
        tabs: group.members.map((spec, idx) => {
          const status = statusOf(spec.id);
          return {
            paneId: spec.id,
            label: tabLabel({
              title: spec.title ?? null,
              shell: spec.shell,
              process: processLabels[spec.id] ?? null,
              fallback: labels.terminal,
            }),
            status,
            active: idx === group.activeIndex,
            agents: agentRows(spec.id, agents[spec.id], status, labels),
          };
        }),
      };
    }),
  }));
}

/// Owning workspace of `paneId` across *all* workspaces, hydrated or not.
export function workspaceIdOfPane(workspaces: Workspace[], paneId: Uuid): number | null {
  for (const ws of workspaces) {
    if (findPane(ws.root, paneId)) return ws.id;
  }
  return null;
}

/// Panes whose lead agent just entered `waiting` — each raises the pane's
/// attention status once, on the transition.
export function newlyWaitingPanes(prev: AgentSnapshot, next: AgentSnapshot): Uuid[] {
  return Object.keys(next).filter(
    (id) => next[id].lead?.status === "waiting" && prev[id]?.lead?.status !== "waiting",
  );
}

export function parseExpanded(raw: string | null): ExpandedMap {
  if (!raw) return {};
  try {
    const v: unknown = JSON.parse(raw);
    if (!v || typeof v !== "object" || Array.isArray(v)) return {};
    const out: ExpandedMap = {};
    for (const [k, b] of Object.entries(v as Record<string, unknown>)) {
      if (typeof b === "boolean") out[k] = b;
    }
    return out;
  } catch {
    return {};
  }
}

export function isExpanded(map: ExpandedMap, wsId: number): boolean {
  return map[String(wsId)] ?? true;
}
