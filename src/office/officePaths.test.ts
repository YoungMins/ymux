import { describe, expect, it } from "vitest";
import type { LayoutNode } from "../types";
import { newPane, paneNode } from "../layout/LayoutTree";
import { corridorGraph, route, segmentsOf, simplify } from "./officePaths";
import { LOBBY_H, MANAGER_HOME, buildOffice, planPath } from "./officeModel";

const pane = (id: string): LayoutNode => ({ ...paneNode(newPane("bash")), id }) as LayoutNode;

describe("corridorGraph", () => {
  it("joins shared edges and splits them at T-junctions", () => {
    // Left cell full height; right column split in two.
    const g = corridorGraph([
      { x: 0, y: 0, w: 0.5, h: 1 },
      { x: 0.5, y: 0, w: 0.5, h: 0.5 },
      { x: 0.5, y: 0.5, w: 0.5, h: 0.5 },
    ]);
    // The middle line x=0.5 is split at y=0.5 (the T-junction).
    const mid = segmentsOf(g).filter((s) => s.a.x === 0.5 && s.b.x === 0.5);
    expect(mid).toHaveLength(2);
    // No edge is listed twice.
    const keys = segmentsOf(g).map((s) => `${s.a.x},${s.a.y}-${s.b.x},${s.b.y}`);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("routes along corridors, never through a cell", () => {
    const g = corridorGraph([
      { x: 0, y: 0, w: 0.5, h: 1 },
      { x: 0.5, y: 0, w: 0.5, h: 1 },
    ], [{ x: 0.25, y: 1 }, { x: 0.75, y: 0 }]);
    const from = g.nodes.findIndex((p) => p.x === 0.25 && p.y === 1);
    const to = g.nodes.findIndex((p) => p.x === 0.75 && p.y === 0);
    const path = simplify(route(g, from, to));
    // Every step is horizontal or vertical along a cell edge.
    for (let i = 1; i < path.length; i++) {
      const a = path[i - 1];
      const b = path[i];
      expect(a.x === b.x || a.y === b.y).toBe(true);
    }
    // Shortest: via the middle line (x = 0.5), not around the outside.
    expect(path.some((p) => p.x === 0.5)).toBe(true);
    expect(path[0]).toEqual({ x: 0.25, y: 1 });
    expect(path[path.length - 1]).toEqual({ x: 0.75, y: 0 });
  });
});

describe("planPath", () => {
  const root: LayoutNode = {
    kind: "split",
    direction: "horizontal",
    ratio: 0.5,
    a: pane("a"),
    b: { kind: "split", direction: "vertical", ratio: 0.5, a: pane("b"), b: pane("c") },
  };
  const office = buildOffice({
    root,
    agents: {},
    statusOf: () => "idle",
    labelOf: (id) => id,
    focused: null,
    corridor: { x: 0.04, y: 0.05 },
  });
  const room = (id: string) => office.rooms.find((r) => r.id === id)!;

  it("leaves gaps between rooms for the corridors", () => {
    const a = room("a").rect;
    const b = room("b").rect;
    const c = room("c").rect;
    expect(b.x - (a.x + a.w)).toBeCloseTo(0.04);
    expect(c.y - (b.y + b.h)).toBeCloseTo(0.05);
    expect(c.y + c.h).toBeCloseTo(1 - LOBBY_H - 0.05);
  });

  it("goes out of one door, along the corridors, in at the other", () => {
    const from = { at: room("c").desks[0].at, room: "c" };
    const to = { at: room("b").desks[0].at, room: "b" };
    const path = planPath(office, from, to);
    // Straight down to the door (merged with the exit when they line up).
    expect(path[0].x).toBeCloseTo(room("c").door.x);
    expect(path).toContainEqual(room("c").exit);
    expect(path).toContainEqual(room("b").exit);
    expect(path[path.length - 1]).toEqual(to.at);
    // Between the two exits it never enters a room.
    const i = path.findIndex((p) => p === room("c").exit || (p.x === room("c").exit.x && p.y === room("c").exit.y));
    const j = path.findIndex((p) => p.x === room("b").exit.x && p.y === room("b").exit.y);
    for (const p of path.slice(i + 1, j)) {
      for (const r of office.rooms) {
        const inside = p.x > r.rect.x + 1e-9 && p.x < r.rect.x + r.rect.w - 1e-9 && p.y > r.rect.y + 1e-9 && p.y < r.rect.y + r.rect.h - 1e-9;
        expect(inside, `${p.x},${p.y} in ${r.id}`).toBe(false);
      }
    }
  });

  it("brings the user up from the lobby and back", () => {
    const axisAligned = (pts: { x: number; y: number }[]) => {
      for (let i = 1; i < pts.length; i++) {
        const a = pts[i - 1];
        const b = pts[i];
        expect(Math.abs(a.x - b.x) < 1e-9 || Math.abs(a.y - b.y) < 1e-9).toBe(true);
      }
    };
    const there = planPath(office, { at: MANAGER_HOME, room: null }, { at: room("b").door, room: "b" });
    axisAligned([MANAGER_HOME, ...there]);
    expect(there).toContainEqual(room("b").exit);
    expect(there[there.length - 1]).toEqual(room("b").door);
    const back = planPath(office, { at: room("b").door, room: "b" }, { at: MANAGER_HOME, room: null });
    axisAligned([room("b").door, ...back]);
    expect(back[back.length - 1]).toEqual(MANAGER_HOME);
  });

  it("walks straight within a room", () => {
    const d = room("a").desks[0].at;
    expect(planPath(office, { at: room("a").door, room: "a" }, { at: d, room: "a" })).toEqual([d]);
  });
});
