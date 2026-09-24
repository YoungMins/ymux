import { describe, it, expect } from "vitest";
import { isTopLevelWindow } from "./frameGuard";

// ymux's document must never boot inside a frame: framed, it still has the
// `main` label and a local Origin, so every `guard_local` command would pass.
describe("isTopLevelWindow", () => {
  it("accepts the top-level window", () => {
    const win: { top: unknown } = { top: null };
    win.top = win;
    expect(isTopLevelWindow(win)).toBe(true);
  });

  it("refuses a framed window", () => {
    expect(isTopLevelWindow({ top: {} })).toBe(false);
  });

  it("refuses when top is missing (detached or unreadable)", () => {
    expect(isTopLevelWindow({ top: null })).toBe(false);
    expect(isTopLevelWindow({ top: undefined })).toBe(false);
  });

  it("refuses when reading top throws", () => {
    const win = {
      get top(): unknown {
        throw new Error("blocked");
      },
    };
    expect(isTopLevelWindow(win)).toBe(false);
  });
});
