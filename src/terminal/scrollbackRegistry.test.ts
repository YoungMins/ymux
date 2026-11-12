import { describe, expect, it, vi } from "vitest";
import { flushAllScrollback, registerScrollbackFlusher } from "./scrollbackRegistry";

describe("scrollbackRegistry", () => {
  it("runs every flusher, survives a failing or throwing one, and unregisters", async () => {
    const ok = vi.fn(async () => {});
    const bad = vi.fn(async () => {
      throw new Error("disk");
    });
    const sync = vi.fn((): Promise<void> => {
      throw new Error("sync");
    });
    const off = [ok, bad, sync].map((f) => registerScrollbackFlusher(f));
    await expect(flushAllScrollback()).resolves.toBeUndefined();
    expect(ok).toHaveBeenCalledOnce();
    expect(bad).toHaveBeenCalledOnce();
    expect(sync).toHaveBeenCalledOnce();
    off.forEach((f) => f());
    await flushAllScrollback();
    expect(ok).toHaveBeenCalledOnce();
  });
});
