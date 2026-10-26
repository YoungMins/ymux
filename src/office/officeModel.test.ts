import { describe, expect, it } from "vitest";
import type { AgentSnapshot, LayoutNode } from "../types";
import { newPane, paneNode } from "../layout/LayoutTree";
import {
  LOBBY_H,
  buildOffice,
  deskSpots,
  diffChars,
  helperSpot,
  layoutLeaves,
  poseOf,
  walkMs,
  worstStatus,
  type OfficeInput,
} from "./officeModel";

const pane = (id: string): LayoutNode => {
  const n = paneNode(newPane("bash"));
  return { ...n, id } as LayoutNode;
};

const input = (root: LayoutNode, agents: AgentSnapshot = {}): OfficeInput => ({
  root,
  agents,
  statusOf: () => "idle",
  labelOf: (id) => `label-${id}`,
  focused: null,
});

describe("layoutLeaves", () => {
  it("splits side by side for horizontal and stacked for vertical, by ratio", () => {
    const root: LayoutNode = {
      kind: "split",
      direction: "horizontal",
      ratio: 0.25,
      a: pane("a"),
      b: { kind: "split", direction: "vertical", ratio: 0.5, a: pane("b"), b: pane("c") },
    };
    const leaves = layoutLeaves(root, { x: 0, y: 0, w: 1, h: 1 });
    const byId = Object.fromEntries(leaves.map((l) => [l.id, l.rect]));
    expect(byId.a).toEqual({ x: 0, y: 0, w: 0.25, h: 1 });
    expect(byId.b).toEqual({ x: 0.25, y: 0, w: 0.75, h: 0.5 });
    expect(byId.c).toEqual({ x: 0.25, y: 0.5, w: 0.75, h: 0.5 });
  });

  it("makes one room of a tab group with one desk per tab", () => {
    const root: LayoutNode = { kind: "tabs", id: "g", active: 1, children: [pane("a"), pane("b")] };
    const office = buildOffice(input(root));
    expect(office.rooms).toHaveLength(1);
    expect(office.rooms[0].id).toBe("g");
    expect(office.rooms[0].desks.map((d) => [d.paneId, d.active])).toEqual([
      ["a", false],
      ["b", true],
    ]);
  });

  it("keeps the lobby free", () => {
    const office = buildOffice(input(pane("a")));
    const r = office.rooms[0].rect;
    expect(r.y + r.h).toBeCloseTo(1 - LOBBY_H);
  });
});

describe("characters", () => {
  const agents: AgentSnapshot = {
    a: {
      lead: { kind: "claude", status: "working", source: "hook", tool: "Bash" },
      subagents: [
        { id: "s1", agent_type: "Explore", status: "working" },
        { id: "s2", agent_type: "reviewer", status: "done" },
      ],
    },
    b: { lead: { kind: "codex", status: "waiting", source: "process", tool: null }, subagents: [] },
  };
  const root: LayoutNode = { kind: "split", direction: "horizontal", ratio: 0.5, a: pane("a"), b: pane("b") };

  it("gives every lead and subagent a character in its room", () => {
    const { chars } = buildOffice(input(root, agents));
    expect(chars.map((c) => [c.key, c.role, c.label, c.pose])).toEqual([
      ["a:lead", "lead", "Claude Code", "typing"],
      ["a:sub:s1", "sub", "Explore", "typing"],
      ["a:sub:s2", "sub", "reviewer", "relaxing"],
      ["b:lead", "lead", "Codex", "raising"],
    ]);
    // Subagents wear their lead's colours and stay inside its room.
    const s1 = chars.find((c) => c.key === "a:sub:s1")!;
    expect(s1.kind).toBe("claude");
    expect(s1.home.x).toBeLessThan(0.5);
  });

  it("leaves panes without an agent empty", () => {
    expect(buildOffice(input(root, {})).chars).toEqual([]);
  });
});

describe("diffChars", () => {
  const base = buildOffice(
    input(pane("a"), {
      a: {
        lead: { kind: "claude", status: "working", source: "hook", tool: null },
        subagents: [{ id: "s1", agent_type: "Explore", status: "working" }],
      },
    }),
  ).chars;

  it("reports arrivals, finished errands and departures", () => {
    const next = buildOffice(
      input(pane("a"), {
        a: {
          lead: { kind: "claude", status: "working", source: "hook", tool: null },
          subagents: [
            { id: "s1", agent_type: "Explore", status: "done" },
            { id: "s2", agent_type: "Plan", status: "working" },
          ],
        },
      }),
    ).chars;
    const d = diffChars(base, next);
    expect(d.entered.map((c) => c.key)).toEqual(["a:sub:s2"]);
    expect(d.finished.map((c) => c.key)).toEqual(["a:sub:s1"]);
    expect(d.left).toEqual([]);
    expect(diffChars(next, []).left).toEqual(["a:lead", "a:sub:s1", "a:sub:s2"]);
  });

  it("does not report a subagent that was already done", () => {
    const done = buildOffice(
      input(pane("a"), {
        a: { lead: null, subagents: [{ id: "s1", agent_type: "Explore", status: "done" }] },
      }),
    ).chars;
    expect(diffChars(done, done).finished).toEqual([]);
  });
});

describe("geometry helpers", () => {
  it("puts up to three desks in a row and wraps the rest", () => {
    expect(deskSpots(1)).toEqual([{ x: 0.5, y: 0.42 }]);
    const four = deskSpots(4);
    expect(four.map((p) => p.x)).toEqual([1 / 6, 0.5, 5 / 6, 1 / 6]);
    expect(four[3].y).toBeGreaterThan(four[0].y);
  });

  it("keeps helpers inside the room", () => {
    for (let n = 1; n <= 12; n++) {
      for (let i = 0; i < n; i++) {
        const p = helperSpot(i, n);
        expect(p.x).toBeGreaterThan(0);
        expect(p.x).toBeLessThan(1);
        expect(p.y).toBeLessThanOrEqual(0.9);
      }
    }
  });

  it("clamps walk time", () => {
    expect(walkMs({ x: 0, y: 0 }, { x: 0, y: 0 })).toBe(350);
    expect(walkMs({ x: 0, y: 0 }, { x: 1, y: 1 })).toBe(2400);
  });

  it("maps statuses", () => {
    expect(worstStatus(["idle", "done", "running"])).toBe("done");
    expect(worstStatus([])).toBe("idle");
    expect(poseOf("idle")).toBe("sleeping");
  });
});
