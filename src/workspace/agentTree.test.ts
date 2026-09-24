import { describe, it, expect } from "vitest";
import {
  buildAgentTree,
  isExpanded,
  newlyWaitingPanes,
  paneLabel,
  parseExpanded,
  showsAgentTrackingHint,
  workspaceIdOfPane,
  type TreeLabels,
} from "./agentTree";
import { newBrowserPane, newPane, paneNode } from "../layout/LayoutTree";
import type { AgentSnapshot, PaneSpec, Workspace } from "../types";
import type { PaneStatus } from "../terminal/paneStatus";

const labels: TreeLabels = { terminal: "pane", browser: "Browser", subagent: "subagent" };

const a: PaneSpec = { ...newPane("pwsh"), title: "build" };
const b: PaneSpec = newPane("Git Bash");
const c: PaneSpec = newBrowserPane("https://example.com");

const workspaces: Workspace[] = [
  {
    id: 1,
    name: "one",
    root: {
      kind: "split",
      direction: "horizontal",
      ratio: 0.5,
      a: paneNode(a),
      b: { kind: "split", direction: "vertical", ratio: 0.5, a: paneNode(b), b: paneNode(c) },
    },
  },
  { id: 3, name: "three", root: paneNode(newPane("zsh")) },
];

describe("buildAgentTree", () => {
  it("lists every pane of every workspace in depth-first order", () => {
    const tree = buildAgentTree(workspaces, {}, () => "idle", labels);
    expect(tree.map((w) => w.wsId)).toEqual([1, 3]);
    expect(tree[0].panes.map((p) => p.paneId)).toEqual([a.id, b.id, c.id]);
    expect(tree[0].panes.map((p) => p.label)).toEqual(["build", "Git Bash", "https://example.com"]);
    expect(tree[1].panes).toHaveLength(1);
  });

  it("attaches agents to their pane, subagents nested under the lead", () => {
    const agents: AgentSnapshot = {
      [b.id]: {
        lead: { kind: "claude", status: "working", source: "hook", tool: "Bash" },
        subagents: [
          { id: "s1", agent_type: "Explore", status: "working" },
          { id: "s2", agent_type: "", status: "done" },
        ],
      },
    };
    const tree = buildAgentTree(workspaces, agents, () => "idle", labels);
    const rows = tree[0].panes[1].agents;
    expect(rows.map((r) => [r.label, r.status, r.depth])).toEqual([
      ["claude", "working", 1],
      ["Explore", "working", 2],
      ["subagent", "done", 2],
    ]);
    expect(rows[0].tool).toBe("Bash");
    expect(tree[0].panes[0].agents).toEqual([]);
  });

  it("derives a process-only lead's status from the pane status", () => {
    const agents: AgentSnapshot = {
      [a.id]: { lead: { kind: "codex", status: "idle", source: "process", tool: null }, subagents: [] },
    };
    const statusOf = (id: string): PaneStatus => (id === a.id ? "attention" : "idle");
    const tree = buildAgentTree(workspaces, agents, statusOf, labels);
    expect(tree[0].panes[0].status).toBe("attention");
    expect(tree[0].panes[0].agents[0].status).toBe("waiting");
  });

  it("ignores agent entries for pane ids not in any layout", () => {
    const agents: AgentSnapshot = {
      "00000000-0000-4000-8000-000000000000": {
        lead: { kind: "claude", status: "working", source: "hook", tool: null },
        subagents: [],
      },
    };
    const tree = buildAgentTree(workspaces, agents, () => "idle", labels);
    expect(tree.flatMap((w) => w.panes.flatMap((p) => p.agents))).toEqual([]);
  });
});

describe("paneLabel", () => {
  it("falls back to shell, then to the generic label", () => {
    expect(paneLabel({ ...newPane(""), title: null }, labels)).toBe("pane");
    expect(paneLabel({ ...newBrowserPane(""), title: null }, labels)).toBe("Browser");
    expect(paneLabel({ ...c, title: "Docs" }, labels)).toBe("Docs");
  });

  it("names an editor pane by its file", () => {
    const ed: PaneSpec = { ...newPane(""), pane_kind: "editor", file_path: "C:\\w\\main.rs" };
    expect(paneLabel(ed, labels)).toBe("main.rs");
    expect(paneLabel({ ...ed, title: "Notes" }, labels)).toBe("Notes");
  });

  it("names a git pane by its repository folder", () => {
    const git: PaneSpec = { ...newPane("", "C:/src/ymux"), pane_kind: "git" };
    expect(paneLabel(git, labels)).toBe("ymux");
    expect(paneLabel({ ...git, cwd: "D:\\작업\\저장소" }, labels)).toBe("저장소");
    expect(paneLabel({ ...git, cwd: null }, labels)).toBe("Git");
  });
});

describe("workspaceIdOfPane", () => {
  it("finds the owning workspace or returns null", () => {
    expect(workspaceIdOfPane(workspaces, c.id)).toBe(1);
    expect(workspaceIdOfPane(workspaces, "missing")).toBeNull();
  });
});

describe("newlyWaitingPanes", () => {
  it("reports only panes whose lead just entered waiting", () => {
    const working = { kind: "claude", status: "working", source: "hook", tool: null } as const;
    const waiting = { ...working, status: "waiting" } as const;
    const prev: AgentSnapshot = { p1: { lead: waiting, subagents: [] }, p2: { lead: working, subagents: [] } };
    const next: AgentSnapshot = {
      p1: { lead: waiting, subagents: [] },
      p2: { lead: waiting, subagents: [] },
      p3: { lead: waiting, subagents: [] },
    };
    expect(newlyWaitingPanes(prev, next).sort()).toEqual(["p2", "p3"]);
  });
});

describe("expansion state", () => {
  it("defaults every workspace to expanded and survives garbage", () => {
    expect(parseExpanded(null)).toEqual({});
    expect(parseExpanded("not json")).toEqual({});
    expect(parseExpanded("[1,2]")).toEqual({});
    expect(parseExpanded('{"1":false,"2":"x"}')).toEqual({ "1": false });
    expect(isExpanded({}, 4)).toBe(true);
    expect(isExpanded({ "4": false }, 4)).toBe(false);
  });
});

describe("buildAgentTree with tabs", () => {
  const G = "gggggggg-gggg-4ggg-8ggg-gggggggggggg";
  const t1 = newPane("pwsh");
  const t2 = newPane("pwsh");
  const plain = newPane("cmd");
  const tabLabels: TreeLabels = {
    terminal: "Terminal",
    browser: "Browser",
    subagent: "subagent",
  };

  const ws: Workspace = {
    id: 1,
    name: "main",
    root: {
      kind: "split",
      direction: "horizontal",
      ratio: 0.5,
      a: { kind: "tabs", id: G, active: 1, children: [paneNode(t1), paneNode(t2)] },
      b: paneNode(plain),
    },
  };

  it("collapses a group into one pane row that carries its tabs", () => {
    const tree = buildAgentTree([ws], {}, () => "idle", tabLabels, {});
    expect(tree[0].panes).toHaveLength(2);
    const group = tree[0].panes[0];
    // The row addresses the active tab, so clicking it focuses what is shown.
    expect(group.paneId).toBe(t2.id);
    expect(group.tabs.map((x) => x.paneId)).toEqual([t1.id, t2.id]);
    expect(group.tabs.map((x) => x.active)).toEqual([false, true]);
  });

  it("omits the tab level for a pane with a single tab", () => {
    const tree = buildAgentTree([ws], {}, () => "idle", tabLabels, {});
    expect(tree[0].panes[1].paneId).toBe(plain.id);
    expect(tree[0].panes[1].tabs).toEqual([]);
  });

  it("labels tabs from the running program, and a title still wins", () => {
    const named = structuredClone(ws);
    (named.root as { a: { children: { title: string | null }[] } }).a.children[0].title = "build";
    const tree = buildAgentTree([named], {}, () => "idle", tabLabels, {
      [t2.id]: "claude",
    });
    expect(tree[0].panes[0].tabs.map((x) => x.label)).toEqual(["build", "claude"]);
  });

  it("attaches agents to the tab they run in, not to the group", () => {
    const agents: AgentSnapshot = {
      [t1.id]: {
        lead: { kind: "claude", status: "working", source: "hook", tool: null },
        subagents: [],
      },
    };
    const tree = buildAgentTree([ws], agents, () => "running", tabLabels, {});
    expect(tree[0].panes[0].agents).toEqual([]);
    expect(tree[0].panes[0].tabs[0].agents.map((a) => a.label)).toEqual(["claude"]);
    expect(tree[0].panes[0].tabs[1].agents).toEqual([]);
  });
});

describe("showsAgentTrackingHint", () => {
  it("shows the hint when tracking is off and the panel is open", () => {
    expect(showsAgentTrackingHint({ agentTracking: false, panelCollapsed: false })).toBe(true);
  });

  it("hides it once tracking is on", () => {
    expect(showsAgentTrackingHint({ agentTracking: true, panelCollapsed: false })).toBe(false);
  });

  it("shows nothing while the panel is collapsed", () => {
    expect(showsAgentTrackingHint({ agentTracking: false, panelCollapsed: true })).toBe(false);
    expect(showsAgentTrackingHint({ agentTracking: true, panelCollapsed: true })).toBe(false);
  });
});
