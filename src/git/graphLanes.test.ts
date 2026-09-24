import { describe, expect, it } from "vitest";
import { assignLanes, laneColor, LANE_COLORS, type Edge } from "./graphLanes";

const c = (hash: string, ...parents: string[]) => ({ hash, parents });

/// Edges as sortable strings, so a test reads as a picture of the row.
const show = (edges: Edge[]) => edges.map((e) => `${e.from}>${e.to}`).sort();

describe("assignLanes", () => {
  it("lays out an empty log as nothing", () => {
    expect(assignLanes([])).toEqual({ rows: [], width: 0 });
  });

  it("keeps a linear history in one lane", () => {
    const { rows, width } = assignLanes([c("C", "B"), c("B", "A"), c("A")]);
    expect(width).toBe(1);
    expect(rows.map((r) => r.lane)).toEqual([0, 0, 0]);
    // The tip has nothing above it, the root nothing below.
    expect(show(rows[0].up)).toEqual([]);
    expect(show(rows[0].down)).toEqual(["0>0"]);
    expect(show(rows[1].up)).toEqual(["0>0"]);
    expect(show(rows[1].down)).toEqual(["0>0"]);
    expect(show(rows[2].up)).toEqual(["0>0"]);
    expect(show(rows[2].down)).toEqual([]);
  });

  it("opens a lane for a merge's second parent and closes it where they meet", () => {
    // M merges B into A; both came from R.
    const { rows, width } = assignLanes([c("M", "A", "B"), c("A", "R"), c("B", "R"), c("R")]);
    expect(width).toBe(2);
    const [m, a, b, r] = rows;
    expect(m.lane).toBe(0);
    expect(show(m.down)).toEqual(["0>0", "0>1"]);
    expect(a.lane).toBe(0);
    expect(show(a.up)).toEqual(["0>0", "1>1"]);
    expect(b.lane).toBe(1);
    expect(show(b.down)).toEqual(["0>0", "1>1"]);
    // Both lanes wait for R: they converge into its node.
    expect(r.lane).toBe(0);
    expect(show(r.up)).toEqual(["0>0", "1>0"]);
    expect(show(r.down)).toEqual([]);
  });

  it("gives every parent of an octopus merge its own lane", () => {
    const { rows, width } = assignLanes([c("O", "A", "B", "C"), c("A"), c("B"), c("C")]);
    expect(width).toBe(3);
    expect(show(rows[0].down)).toEqual(["0>0", "0>1", "0>2"]);
    expect(rows.slice(1).map((r) => r.lane)).toEqual([0, 1, 2]);
  });

  it("reuses a freed lane for a branch that reappears after a gap", () => {
    // Two tips; lane 0's history ends at a1, then an unrelated tip C shows
    // up further down (its child is not in the log) and takes lane 0 again.
    const { rows } = assignLanes([
      c("A", "a1"),
      c("B", "b1"),
      c("a1"),
      c("C", "c1"),
      c("b1"),
      c("c1"),
    ]);
    const byHash = (i: number) => rows[i];
    expect(byHash(0).lane).toBe(0);
    expect(byHash(1).lane).toBe(1);
    expect(byHash(2).lane).toBe(0);
    expect(show(byHash(2).down)).toEqual(["1>1"]);
    const tipC = byHash(3);
    expect(tipC.lane).toBe(0);
    // Nothing enters C from above in its own lane: that is the gap.
    expect(show(tipC.up)).toEqual(["1>1"]);
    expect(show(tipC.down)).toEqual(["0>0", "1>1"]);
    expect(byHash(4).lane).toBe(1);
    expect(byHash(5).lane).toBe(0);
  });

  it("keeps a lane open for a parent the loaded page does not contain", () => {
    // History truncated by paging: the line runs off the bottom.
    const { rows } = assignLanes([c("B", "A")]);
    expect(show(rows[0].down)).toEqual(["0>0"]);
  });

  it("colours by lane index and cycles every six lanes", () => {
    expect(LANE_COLORS).toBe(6);
    expect([0, 1, 5, 6, 7, 13].map(laneColor)).toEqual([0, 1, 5, 0, 1, 1]);
    const parents = ["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7"];
    const { rows } = assignLanes([c("O", ...parents), ...parents.map((p) => c(p))]);
    const toSix = rows[0].down.find((e) => e.to === 6)!;
    const toSeven = rows[0].down.find((e) => e.to === 7)!;
    expect(toSix.color).toBe(0);
    expect(toSeven.color).toBe(1);
    expect(rows[8].color).toBe(1);
  });
});
