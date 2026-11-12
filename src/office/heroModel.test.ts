import { describe, expect, it } from "vitest";
import { LINEAR_LEVELS, TIER, TIER_LEVELS, TOKENS_PER_LEVEL, accountXp, levelOf, levelUps, nextTierLevel, tierOf } from "./heroModel";
import type { UsageRow } from "../tokenmonitor/model";

describe("levels", () => {
  it("counts one level per 100M tokens", () => {
    expect(levelOf(0)).toEqual({ level: 0, xp: 0, need: TOKENS_PER_LEVEL, progress: 0 });
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

describe("levels past the linear stretch", () => {
  const B = 1_000_000_000;

  it("is linear, unchanged, up to level 100 and continuous at the seam", () => {
    expect(levelOf(99 * TOKENS_PER_LEVEL).level).toBe(99);
    expect(levelOf(LINEAR_LEVELS * TOKENS_PER_LEVEL - 1).level).toBe(99);
    const l100 = levelOf(LINEAR_LEVELS * TOKENS_PER_LEVEL);
    expect(l100).toMatchObject({ level: 100, xp: 0, need: TOKENS_PER_LEVEL * 1.01 });
    // Level 101 costs 1.01 x TPL, level 102 1.02 x TPL ...
    expect(levelOf(10 * B + 1.01 * TOKENS_PER_LEVEL).level).toBe(101);
    expect(levelOf(10 * B + 1.01 * TOKENS_PER_LEVEL - 1).level).toBe(100);
    expect(levelOf(10 * B + 1.01 * TOKENS_PER_LEVEL + 1.02 * TOKENS_PER_LEVEL).level).toBe(102);
  });

  it("makes the maintainer's 65.8B a few hundred levels, not 658", () => {
    const l = levelOf(65.8 * B);
    expect(l.level).toBeGreaterThan(300);
    expect(l.level).toBeLessThan(400);
    expect(l.xp).toBeGreaterThanOrEqual(0);
    expect(l.xp).toBeLessThan(l.need);
    expect(l.progress).toBeGreaterThanOrEqual(0);
    expect(l.progress).toBeLessThan(1);
  });

  it("never decreases with more tokens, and progress stays in [0, 1)", () => {
    let prev = -1;
    for (let t = 9.5 * B; t < 200 * B; t += 0.37 * B + 12345) {
      const l = levelOf(t);
      expect(l.level).toBeGreaterThanOrEqual(prev);
      expect(l.progress).toBeGreaterThanOrEqual(0);
      expect(l.progress).toBeLessThan(1);
      prev = l.level;
    }
  });

  it("copes with absurd and invalid totals", () => {
    expect(Number.isFinite(levelOf(1e15).level)).toBe(true);
    expect(levelOf(Number.POSITIVE_INFINITY).level).toBe(0);
    expect(tierOf(levelOf(65.8 * B).level)).toBe(9);
    expect(nextTierLevel(levelOf(65.8 * B).level)).toBeNull();
  });
});

describe("the first run over a complete history", () => {
  it("announces one level-up, the biggest, however far the jump", () => {
    const now = new Map([
      ["a", levelOf(65.8e9).level],
      ["b", levelOf(1.4e9).level],
      ["c", 5],
    ]);
    const ups = levelUps({ a: 12, b: 3, c: 5 }, now);
    expect(ups.map((u) => u.account)).toEqual(["a", "b"]);
    expect(ups[0].level).toBe(now.get("a"));
  });

  it("keeps every tier at or above what it was", () => {
    for (const lv of [0, 1, 3, 12, 99, 100]) expect(tierOf(levelOf(lv * TOKENS_PER_LEVEL).level)).toBe(tierOf(lv));
  });
});
