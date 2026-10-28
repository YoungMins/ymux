import { describe, expect, it } from "vitest";
import { linkedGroup, snapTo, type GutterLine } from "./gutterSnap";

// A 2×2 grid: a top-left/top-right divider (0) above a bottom one (1), with
// the crossing divider between them at y 400..404; and a divider (2) far off
// in another column.
const grid = (bottomAt: number): GutterLine[] => [
  { at: 500, from: 0, to: 400 },
  { at: bottomAt, from: 404, to: 800 },
  { at: 1300, from: 0, to: 800 },
];

describe("gutterSnap", () => {
  it("snaps onto the neighbour's line within the threshold", () => {
    const lines = grid(500);
    expect(snapTo(506, lines, [1])).toEqual({ at: 500, snapped: 0 });
    expect(snapTo(494, lines, [1])).toEqual({ at: 500, snapped: 0 });
    expect(snapTo(510, lines, [1])).toEqual({ at: 510, snapped: null });
  });

  it("only snaps to dividers that meet the dragged one end to end", () => {
    // Two pieces on one column with a pane between them, not a crossing divider.
    const far: GutterLine[] = [{ at: 500, from: 0, to: 300 }, { at: 503, from: 404, to: 800 }];
    expect(snapTo(503, far, [1])).toEqual({ at: 503, snapped: null });
  });

  it("links dividers on one line that meet end to end", () => {
    expect(linkedGroup(grid(500), 1).sort()).toEqual([0, 1]);
    expect(linkedGroup(grid(520), 1)).toEqual([1]);
  });

  it("links through a chain of dividers", () => {
    const lines: GutterLine[] = [
      { at: 300, from: 0, to: 200 },
      { at: 300, from: 204, to: 400 },
      { at: 300, from: 404, to: 600 },
      { at: 300, from: 700, to: 900 },
    ];
    expect(linkedGroup(lines, 0).sort()).toEqual([0, 1, 2]);
  });

  it("does not snap a linked group onto itself", () => {
    const lines = grid(500);
    expect(snapTo(503, lines, [0, 1])).toEqual({ at: 503, snapped: null });
  });
});
