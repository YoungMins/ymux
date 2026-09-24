import { describe, expect, it, vi } from "vitest";

import type { PathCandidate } from "./pathMatch";
import {
  PROBE_CACHE_LIMIT,
  PathProbeCache,
  isProbeable,
  resolveOverlaps,
  type ProbeResult,
} from "./pathProbe";

function cand(text: string, start = 0, end = text.length): PathCandidate {
  return { text, start, end };
}

/// A probe that says "exists" for anything in `present`, and counts calls.
function fakeProbe(present: readonly string[]) {
  const calls: string[][] = [];
  const fn = async (
    texts: readonly string[],
    cwd: string | null,
  ): Promise<ProbeResult[]> => {
    calls.push([...texts]);
    return texts.map((t) =>
      present.includes(t) ? { absolute: `${cwd ?? ""}/${t}`, is_dir: false } : null,
    );
  };
  return { fn, calls };
}

describe("isProbeable", () => {
  it("accepts an ordinary path", () => {
    expect(isProbeable("src/main.ts")).toBe(true);
  });

  it("rejects the empty string", () => {
    expect(isProbeable("")).toBe(false);
  });

  it("rejects control characters", () => {
    expect(isProbeable("src/\u0007a.ts")).toBe(false);
    expect(isProbeable("src/\u001ba.ts")).toBe(false);
    expect(isProbeable("src/\u007fa.ts")).toBe(false);
  });

  it("rejects an embedded newline", () => {
    expect(isProbeable("src/a\nrm -rf /")).toBe(false);
  });

  it("rejects an absurdly long string", () => {
    expect(isProbeable("a/".repeat(4096))).toBe(false);
  });
});

describe("resolveOverlaps", () => {
  const exists = (c: { text: string }): boolean => c.text !== "missing";

  it("keeps a non-overlapping set untouched", () => {
    const cs = [cand("a/1", 0, 3), cand("b/2", 4, 7)];
    expect(resolveOverlaps(cs, exists).map((c) => c.text)).toEqual(["a/1", "b/2"]);
  });

  it("prefers the longest of two overlapping candidates", () => {
    const whole = cand("C:\\Program Files\\Git", 1, 21);
    const prefix = cand("C:\\Program", 1, 11);
    expect(resolveOverlaps([prefix, whole], exists).map((c) => c.text)).toEqual([
      "C:\\Program Files\\Git",
    ]);
  });

  it("falls back to the shorter candidate when the longer does not exist", () => {
    const whole = { ...cand("missing", 1, 21), text: "missing" };
    const prefix = cand("C:\\Program", 1, 11);
    expect(resolveOverlaps([whole, prefix], exists).map((c) => c.text)).toEqual([
      "C:\\Program",
    ]);
  });

  it("drops every candidate that does not exist", () => {
    expect(resolveOverlaps([cand("missing", 0, 7)], exists)).toEqual([]);
  });

  it("returns the kept candidates in source order", () => {
    const cs = [cand("z/9", 10, 13), cand("a/1", 0, 3)];
    expect(resolveOverlaps(cs, exists).map((c) => c.start)).toEqual([0, 10]);
  });
});

describe("PathProbeCache", () => {
  it("resolves candidates through the probe", async () => {
    const { fn } = fakeProbe(["src/main.ts"]);
    const cache = new PathProbeCache(fn);
    cache.setCwd("/repo");
    const got = await cache.resolve([cand("src/main.ts"), cand("and/or")]);
    expect(got.get("src/main.ts")).toEqual({ absolute: "/repo/src/main.ts", is_dir: false });
    expect(got.has("and/or")).toBe(false);
  });

  it("batches every candidate of a row into one call", async () => {
    const { fn, calls } = fakeProbe([]);
    const cache = new PathProbeCache(fn);
    await cache.resolve([cand("a/1"), cand("b/2"), cand("c/3")]);
    expect(calls).toEqual([["a/1", "b/2", "c/3"]]);
  });

  it("does not re-probe a cached hit", async () => {
    const { fn, calls } = fakeProbe(["a/1"]);
    const cache = new PathProbeCache(fn);
    await cache.resolve([cand("a/1")]);
    await cache.resolve([cand("a/1")]);
    expect(calls).toHaveLength(1);
  });

  it("caches negative answers too", async () => {
    const { fn, calls } = fakeProbe([]);
    const cache = new PathProbeCache(fn);
    await cache.resolve([cand("and/or")]);
    await cache.resolve([cand("and/or")]);
    expect(calls).toHaveLength(1);
    expect(cache.peek("and/or")).toBeNull();
  });

  it("deduplicates repeats within one row", async () => {
    const { fn, calls } = fakeProbe([]);
    const cache = new PathProbeCache(fn);
    await cache.resolve([cand("a/1", 0, 3), cand("a/1", 8, 11)]);
    expect(calls).toEqual([["a/1"]]);
  });

  it("shares one request between concurrent hovers", async () => {
    const { fn, calls } = fakeProbe(["a/1"]);
    const cache = new PathProbeCache(fn);
    const [first, second] = await Promise.all([
      cache.resolve([cand("a/1")]),
      cache.resolve([cand("a/1")]),
    ]);
    expect(calls).toHaveLength(1);
    expect(first.get("a/1")).toEqual(second.get("a/1"));
  });

  it("skips unprobeable candidates entirely", async () => {
    const { fn, calls } = fakeProbe([]);
    const cache = new PathProbeCache(fn);
    const got = await cache.resolve([cand("src/\u0007a.ts")]);
    expect(calls).toEqual([]);
    expect(got.size).toBe(0);
  });

  it("reports an all-cached row so the caller can answer synchronously", async () => {
    const { fn } = fakeProbe(["a/1"]);
    const cache = new PathProbeCache(fn);
    expect(cache.allCached([cand("a/1")])).toBe(false);
    await cache.resolve([cand("a/1")]);
    expect(cache.allCached([cand("a/1")])).toBe(true);
  });

  it("treats a row of only unprobeable candidates as cached", () => {
    const cache = new PathProbeCache(fakeProbe([]).fn);
    expect(cache.allCached([cand("src/\u0007a.ts")])).toBe(true);
  });

  it("invalidates everything when the cwd changes", async () => {
    const { fn, calls } = fakeProbe(["src/main.ts"]);
    const cache = new PathProbeCache(fn);
    cache.setCwd("/a");
    await cache.resolve([cand("src/main.ts")]);
    cache.setCwd("/b");
    expect(cache.size).toBe(0);
    const got = await cache.resolve([cand("src/main.ts")]);
    expect(calls).toHaveLength(2);
    expect(got.get("src/main.ts")?.absolute).toBe("/b/src/main.ts");
  });

  it("does not invalidate when the cwd is set to the same value", async () => {
    const { fn, calls } = fakeProbe(["a/1"]);
    const cache = new PathProbeCache(fn);
    cache.setCwd("/a");
    await cache.resolve([cand("a/1")]);
    cache.setCwd("/a");
    await cache.resolve([cand("a/1")]);
    expect(calls).toHaveLength(1);
  });

  it("does not cache an answer computed against a cwd it has since left", async () => {
    let release!: (r: ProbeResult[]) => void;
    const fn = vi.fn(
      () => new Promise<ProbeResult[]>((res) => (release = res)),
    );
    const cache = new PathProbeCache(fn);
    cache.setCwd("/a");
    const pending = cache.resolve([cand("a/1")]);
    cache.setCwd("/b");
    release([{ absolute: "/a/a/1", is_dir: false }]);
    await pending;
    expect(cache.peek("a/1")).toBeUndefined();
  });

  it("treats a failed probe as 'no link' rather than throwing", async () => {
    const cache = new PathProbeCache(async () => {
      throw new Error("ipc down");
    });
    await expect(cache.resolve([cand("a/1")])).resolves.toEqual(new Map());
  });

  it("evicts least-recently-used entries past the limit", async () => {
    const { fn } = fakeProbe([]);
    const cache = new PathProbeCache(fn, 3);
    await cache.resolve([cand("a/1"), cand("b/2"), cand("c/3")]);
    // Touch a/1 so b/2 becomes the least recently used.
    expect(cache.peek("a/1")).toBeNull();
    await cache.resolve([cand("d/4")]);
    expect(cache.size).toBe(3);
    expect(cache.peek("b/2")).toBeUndefined();
    expect(cache.peek("a/1")).toBeNull();
    expect(cache.peek("d/4")).toBeNull();
  });

  it("defaults to a bounded limit", async () => {
    const { fn } = fakeProbe([]);
    const cache = new PathProbeCache(fn);
    for (let i = 0; i < PROBE_CACHE_LIMIT + 50; i++) {
      await cache.resolve([cand(`d${i}/f`)]);
    }
    expect(cache.size).toBe(PROBE_CACHE_LIMIT);
  });
});
