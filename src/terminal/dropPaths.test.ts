import { describe, it, expect } from "vitest";
import { formatDroppedPaths } from "./dropPaths";

describe("formatDroppedPaths", () => {
  it("double-quotes a single path so spaces survive as one argument", () => {
    expect(formatDroppedPaths(["C:\\Users\\a b\\notes.txt"])).toBe(
      '"C:\\Users\\a b\\notes.txt"',
    );
  });

  it("joins multiple paths with a space, each quoted", () => {
    expect(formatDroppedPaths(["C:\\a.txt", "D:\\b c.png"])).toBe(
      '"C:\\a.txt" "D:\\b c.png"',
    );
  });

  it("returns an empty string for no paths", () => {
    expect(formatDroppedPaths([])).toBe("");
  });

  it("skips empty entries rather than emitting bare quotes", () => {
    expect(formatDroppedPaths(["", "C:\\a.txt", ""])).toBe('"C:\\a.txt"');
  });
});
