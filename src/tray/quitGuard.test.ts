import { describe, expect, it, vi } from "vitest";
import { runQuitGuard, type QuitGuardDeps } from "./quitGuard";

function deps(over: Partial<QuitGuardDeps> = {}): QuitGuardDeps {
  return {
    hasUnsavedEditors: () => false,
    showWindow: vi.fn(async () => {}),
    confirmCloseAll: vi.fn(async () => true),
    flushDrafts: vi.fn(async () => {}),
    flushLayout: vi.fn(async () => {}),
    ...over,
  };
}

describe("runQuitGuard", () => {
  it("quits without showing the window when nothing is unsaved", async () => {
    const d = deps();
    expect(await runQuitGuard(d)).toBe(true);
    expect(d.showWindow).not.toHaveBeenCalled();
    expect(d.flushLayout).toHaveBeenCalledOnce();
  });

  it("shows the window before prompting about unsaved editors", async () => {
    const order: string[] = [];
    const d = deps({
      hasUnsavedEditors: () => true,
      showWindow: async () => void order.push("show"),
      confirmCloseAll: async () => (order.push("confirm"), true),
    });
    expect(await runQuitGuard(d)).toBe(true);
    expect(order).toEqual(["show", "confirm"]);
  });

  it("a cancelled prompt keeps the app and skips the layout flush", async () => {
    const d = deps({ hasUnsavedEditors: () => true, confirmCloseAll: async () => false });
    expect(await runQuitGuard(d)).toBe(false);
    expect(d.flushLayout).not.toHaveBeenCalled();
  });

  it("a failing guard still quits, after flushing drafts", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const d = deps({
      confirmCloseAll: async () => {
        throw new Error("boom");
      },
    });
    expect(await runQuitGuard(d)).toBe(true);
    expect(d.flushDrafts).toHaveBeenCalledOnce();
    expect(d.flushLayout).toHaveBeenCalledOnce();
  });

  it("a hung layout flush cannot hold the quit", async () => {
    const d = deps({ flushLayout: () => new Promise<void>(() => {}) });
    expect(await runQuitGuard(d, 10)).toBe(true);
  });
});
