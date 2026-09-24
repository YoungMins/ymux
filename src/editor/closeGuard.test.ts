import { describe, expect, it } from "vitest";
import { closeDecision, closePlan, closeResult } from "./closeGuard";

describe("closeDecision — dirty/clean × titled/untitled", () => {
  it("a clean pane closes without asking, titled or not", () => {
    expect(closeDecision(false, true)).toBeNull();
    expect(closeDecision(false, false)).toBeNull();
  });

  it("a dirty file offers save / discard / cancel", () => {
    expect(closeDecision(true, true)).toEqual(["save", "discard", "cancel"]);
  });

  it("a dirty untitled buffer cannot be saved from the prompt", () => {
    expect(closeDecision(true, false)).toEqual(["discard", "cancel"]);
  });
});

describe("closePlan — one prompt for many panes", () => {
  it("closes when nothing is dirty", () => {
    expect(closePlan([])).toEqual({ kind: "close" });
    expect(closePlan([{ name: "a.rs", dirty: false, hasPath: true }])).toEqual({ kind: "close" });
  });

  it("lists every dirty file, and only those", () => {
    const plan = closePlan([
      { name: "a.rs", dirty: true, hasPath: true },
      { name: "b.rs", dirty: false, hasPath: true },
      { name: "c.md", dirty: true, hasPath: true },
    ]);
    expect(plan).toEqual({
      kind: "ask",
      names: ["a.rs", "c.md"],
      choices: ["save", "discard", "cancel"],
    });
  });

  it("drops Save when any dirty buffer has nowhere to go", () => {
    const plan = closePlan([
      { name: "a.rs", dirty: true, hasPath: true },
      { name: "untitled", dirty: true, hasPath: false },
    ]);
    expect(plan.kind === "ask" && plan.choices).toEqual(["discard", "cancel"]);
  });
});

describe("closeResult — only Discard or a completed Save closes", () => {
  it("Esc / click-outside and Cancel keep the pane", () => {
    expect(closeResult(null)).toBe(false);
    expect(closeResult("cancel")).toBe(false);
  });

  it("Discard closes", () => {
    expect(closeResult("discard")).toBe(true);
  });

  it("Save closes only when every save completed", () => {
    expect(closeResult("save", [true])).toBe(true);
    expect(closeResult("save", [true, true])).toBe(true);
    expect(closeResult("save", [true, false])).toBe(false);
    // A conflict the user then cancelled reports false, like any failure.
    expect(closeResult("save", [false])).toBe(false);
    // Save with nothing saved is not a close.
    expect(closeResult("save", [])).toBe(false);
  });
});
