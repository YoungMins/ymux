import { describe, expect, it } from "vitest";
import { TIER, TIER_LEVELS, TOKENS_PER_LEVEL, accountXp, levelOf, levelUps, nextTierLevel, tierOf } from "./heroModel";
import type { UsageRow } from "../tokenmonitor/model";

describe("levels", () => {
  it("counts one level per 100M tokens", () => {
    expect(levelOf(0)).toEqual({ level: 0, xp: 0, progress: 0 });
    expect(levelOf(TOKENS_PER_LEVEL - 1).level).toBe(0);
    expect(levelOf(TOKENS_PER_LEVEL).level).toBe(1);
    const l = levelOf(1_250_000_000);
    expect(l.level).toBe(12);
    expect(l.xp).toBe(50_000_000);
    expect(l.progress).toBeCloseTo(0.5);
    expect(levelOf(Number.NaN).level).toBe(0);
    expect(levelOf(-5).level).toBe(0);
  });

  it("changes the look at the tier levels", () => {
    expect([0, 1, 2, 3, 5, 9, 10, 20, 30, 49, 50, 75, 99, 100, 999].map(tierOf)).toEqual([
      0, 1, 1, 2, 3, 3, 4, 5, 6, 6, 7, 8, 8, 9, 9,
    ]);
    expect(nextTierLevel(0)).toBe(1);
    expect(nextTierLevel(7)).toBe(10);
    expect(nextTierLevel(60)).toBe(75);
    expect(nextTierLevel(100)).toBeNull();
  });

  it("starts the tiers at increasing levels, one per step", () => {
    expect(TIER_LEVELS).toHaveLength(Object.keys(TIER).length + 1);
    for (let i = 1; i < TIER_LEVELS.length; i++) expect(TIER_LEVELS[i]).toBeGreaterThan(TIER_LEVELS[i - 1]);
    for (const [name, tier] of Object.entries(TIER)) expect(tierOf(TIER_LEVELS[tier]), name).toBe(tier);
  });
});

describe("accountXp", () => {
  const row = (account: string, total: number): UsageRow => {
    const t = { input: 0, output: 0, cache_read: 0, cache_write: 0, total };
    return { account, provider: "claude", model: "m", project: "p", five_hour: t, seven_day: t, all_time: t };
  };

  it("adds every row of an account, both CLIs and all projects", () => {
    const xp = accountXp([row("", 10), row("a", 5), row("", 7), row("none", 1)]);
    expect(Object.fromEntries(xp)).toEqual({ "": 17, a: 5, none: 1 });
  });
});

describe("levelUps", () => {
  it("reports only accounts seen before at a lower level", () => {
    const now = new Map([
      ["", 3],
      ["a", 2],
      ["b", 1],
    ]);
    expect(levelUps({ "": 2, a: 2 }, now)).toEqual([{ account: "", level: 3 }]);
  });
});
