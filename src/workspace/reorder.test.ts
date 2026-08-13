import { describe, it, expect } from "vitest";
import { moveItem, insertIndexFromMidpoints } from "./reorder";

describe("moveItem", () => {
  it("moves an item down, before the given original index", () => {
    expect(moveItem(["a", "b", "c"], 0, 2)).toEqual(["b", "a", "c"]);
    expect(moveItem(["a", "b", "c"], 0, 3)).toEqual(["b", "c", "a"]);
  });

  it("moves an item up", () => {
    expect(moveItem(["a", "b", "c"], 2, 0)).toEqual(["c", "a", "b"]);
    expect(moveItem(["a", "b", "c"], 2, 1)).toEqual(["a", "c", "b"]);
  });

  it("returns null for no-op drops just above or below itself", () => {
    expect(moveItem(["a", "b", "c"], 1, 1)).toBeNull();
    expect(moveItem(["a", "b", "c"], 1, 2)).toBeNull();
  });

  it("returns null for an out-of-range source", () => {
    expect(moveItem(["a", "b"], -1, 0)).toBeNull();
    expect(moveItem(["a", "b"], 2, 0)).toBeNull();
    expect(moveItem([], 0, 0)).toBeNull();
  });

  it("clamps an out-of-range target instead of failing", () => {
    expect(moveItem(["a", "b", "c"], 0, 99)).toEqual(["b", "c", "a"]);
    expect(moveItem(["a", "b", "c"], 2, -5)).toEqual(["c", "a", "b"]);
  });

  it("leaves the input untouched", () => {
    const input = ["a", "b", "c"];
    moveItem(input, 0, 3);
    expect(input).toEqual(["a", "b", "c"]);
  });
});

describe("insertIndexFromMidpoints", () => {
  it("returns 0 above the first row's midpoint", () => {
    expect(insertIndexFromMidpoints([10, 30, 50], 5)).toBe(0);
    expect(insertIndexFromMidpoints([10, 30, 50], 10)).toBe(0);
  });

  it("returns the index of the first row whose midpoint is below y", () => {
    expect(insertIndexFromMidpoints([10, 30, 50], 11)).toBe(1);
    expect(insertIndexFromMidpoints([10, 30, 50], 31)).toBe(2);
  });

  it("returns the list length past the last row", () => {
    expect(insertIndexFromMidpoints([10, 30, 50], 60)).toBe(3);
  });

  it("returns 0 for an empty list", () => {
    expect(insertIndexFromMidpoints([], 42)).toBe(0);
  });
});
