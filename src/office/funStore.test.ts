// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { flushFunStore, funStore } from "./funStore";

describe("funStore flush", () => {
  it("writes a pending change at once instead of after the debounce", () => {
    localStorage.clear();
    funStore.markReport("2026-W99");
    expect(localStorage.length).toBe(0);
    flushFunStore();
    expect(localStorage.length).toBe(1);
    expect(localStorage.getItem(localStorage.key(0) as string)).toContain("2026-W99");
  });

  it("is a no-op with nothing pending", () => {
    localStorage.clear();
    flushFunStore();
    expect(localStorage.length).toBe(0);
  });

  it("flushes on pagehide", () => {
    localStorage.clear();
    funStore.markReport("2026-W98");
    window.dispatchEvent(new Event("pagehide"));
    expect(localStorage.getItem(localStorage.key(0) as string)).toContain("2026-W98");
  });
});
