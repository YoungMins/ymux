import { describe, expect, it } from "vitest";
import {
  ACHIEVEMENTS,
  BOSS_MS,
  FAMILIES,
  QUEST_POOL,
  SHOP,
  applyEvent,
  buildingOf,
  buy,
  dayKey,
  dayPart,
  emptyStats,
  normalizeStats,
  pizzaTime,
  priceOf,
  questDef,
  questsFor,
  reportDue,
  roman,
  seasonOf,
  streak,
  toolKind,
  weekKey,
  weekReport,
  weeklySpecial,
} from "./funModel";

const at = (y: number, m: number, d: number, h = 12) => new Date(y, m - 1, d, h).getTime();

describe("achievements", () => {
  it("has more than a hundred, with unique ids and rising steps", () => {
    expect(ACHIEVEMENTS.length).toBeGreaterThanOrEqual(100);
    expect(new Set(ACHIEVEMENTS.map((a) => a.id)).size).toBe(ACHIEVEMENTS.length);
    for (const f of FAMILIES) for (let i = 1; i < f.steps.length; i++) expect(f.steps[i], f.id).toBeGreaterThan(f.steps[i - 1]);
  });

  it("unlocks the first turn, pays coins and remembers the day", () => {
    const s = emptyStats();
    const now = at(2026, 10, 28);
    const r = applyEvent(s, { t: "turn", kind: "claude", ms: 60_000, at: now }, now);
    expect(r.unlocked.map((a) => a.id)).toContain("turns:1");
    expect(s.unlocked["turns:1"]).toBe(now);
    expect(s.coins).toBeGreaterThan(5);
    expect(s.days[dayKey(now)].unlocked).toContain("turns:1");
    // Not twice.
    expect(applyEvent(s, { t: "count", what: "pet" }, now).unlocked.map((a) => a.id)).not.toContain("turns:1");
  });

  it("counts night, weekend, marathons, bosses and speed runs", () => {
    const s = emptyStats();
    const sat3am = at(2026, 10, 31, 3);
    const r = applyEvent(s, { t: "turn", kind: "codex", ms: BOSS_MS + 1, at: sat3am }, sat3am);
    expect(r.boss).toBe(true);
    expect(s.counters.night).toBe(1);
    expect(s.counters.weekend).toBe(1);
    expect(s.counters.bosses).toBe(1);
    applyEvent(s, { t: "turn", kind: "codex", ms: 10_000, at: sat3am }, sat3am);
    expect(s.counters.speed).toBe(1);
  });

  it("hunts a bug when a failing test is made to pass", () => {
    const s = emptyStats();
    const now = at(2026, 10, 28);
    applyEvent(s, { t: "facts", pane: "p", files: 1, add: 2, del: 0, test: false }, now);
    applyEvent(s, { t: "facts", pane: "p", files: 1, add: 1, del: 1, test: true }, now);
    expect(s.counters.bughunt).toBe(1);
    expect(s.counters.tiny).toBe(2);
    applyEvent(s, { t: "facts", pane: "q", files: 9, add: 10, del: 200, test: null }, now);
    expect(s.counters.cleanup).toBe(1);
  });

  it("maps tools to their families", () => {
    expect(toolKind("Read")).toBe("read");
    expect(toolKind("Grep")).toBe("read");
    expect(toolKind("Bash")).toBe("bash");
    expect(toolKind("MultiEdit")).toBe("edit");
    expect(toolKind("WebFetch")).toBe("web");
    expect(toolKind("TodoWrite")).toBe("edit");
  });

  it("writes ranks as roman numerals", () => {
    expect([1, 2, 4, 5, 9].map(roman)).toEqual(["I", "II", "IV", "V", "IX"]);
  });
});

describe("days, streaks and seasons", () => {
  it("counts a streak ending today or yesterday", () => {
    const s = emptyStats();
    for (const d of [24, 25, 26, 27]) applyEvent(s, { t: "turn", kind: "claude", ms: null, at: at(2026, 10, d) }, at(2026, 10, d));
    expect(streak(s.days, at(2026, 10, 28))).toBe(4);
    expect(streak(s.days, at(2026, 10, 27))).toBe(4);
    expect(streak(s.days, at(2026, 10, 30))).toBe(0);
  });

  it("knows the week, the season and the time of day", () => {
    expect(weekKey(at(2026, 10, 28))).toBe("2026-10-26");
    expect(seasonOf(at(2026, 12, 24))).toBe("xmas");
    expect(seasonOf(at(2026, 10, 31))).toBe("halloween");
    expect(seasonOf(at(2027, 1, 2))).toBe("newyear");
    expect(seasonOf(at(2027, 4, 1))).toBe("spring");
    expect(seasonOf(at(2027, 7, 1))).toBeNull();
    expect(dayPart(at(2026, 10, 28, 22))).toBe("night");
    expect(dayPart(at(2026, 10, 28, 18))).toBe("evening");
    expect(pizzaTime(at(2026, 10, 30, 16))).toBe(true);
    expect(pizzaTime(at(2026, 10, 29, 16))).toBe(false);
  });
});

describe("coins, shop and building", () => {
  it("buys what it can afford, once", () => {
    const s = emptyStats();
    s.coins = 100;
    expect(buy(s, "coffee")).toBe(true);
    expect(s.coins).toBe(40);
    expect(buy(s, "coffee")).toBe(false);
    expect(buy(s, "statue")).toBe(false);
    expect(buy(s, "nope")).toBe(false);
    expect(new Set(SHOP.map((i) => i.id)).size).toBe(SHOP.length);
  });

  it("grows the building with turns", () => {
    expect(buildingOf(0).name).toBe("garage");
    expect(buildingOf(10).name).toBe("startup");
    expect(buildingOf(2500)).toEqual({ step: 6, name: "empire", next: null });
  });
});

describe("quests and the weekly report", () => {
  it("picks three different quests a day, the same all day", () => {
    const q = questsFor("2026-10-28");
    expect(q).toHaveLength(3);
    expect(new Set(q).size).toBe(3);
    expect(questsFor("2026-10-28")).toEqual(q);
    for (const id of q) expect(QUEST_POOL.some((p) => p.id === id)).toBe(true);
  });

  it("pays a quest once its counter is reached", () => {
    const s = emptyStats();
    const now = at(2026, 10, 28);
    s.quests = { day: dayKey(now), ids: ["pet3"], done: [] };
    for (let i = 0; i < 2; i++) applyEvent(s, { t: "count", what: "pet" }, now);
    const r = applyEvent(s, { t: "count", what: "pet" }, now);
    expect(r.quests).toEqual(["pet3"]);
    expect(s.counters.quests).toBe(1);
  });

  it("sums the week and finds its MVP and busiest day", () => {
    const s = emptyStats();
    applyEvent(s, { t: "turn", kind: "claude", ms: 1000, at: at(2026, 10, 26) }, at(2026, 10, 26));
    for (let i = 0; i < 3; i++) applyEvent(s, { t: "turn", kind: "codex", ms: 1000, at: at(2026, 10, 27) }, at(2026, 10, 27));
    const r = weekReport(s, at(2026, 10, 30));
    expect(r.turns).toBe(4);
    expect(r.mvp).toEqual({ kind: "codex", turns: 3 });
    expect(r.busiest?.day).toBe("2026-10-27");
    expect(reportDue(s, at(2026, 10, 30, 17))).toBe("2026-10-26");
    expect(reportDue(s, at(2026, 10, 29, 17))).toBeNull();
  });

  it("survives whatever is in storage", () => {
    expect(normalizeStats(null).coins).toBe(0);
    expect(normalizeStats({ coins: -5, counters: { a: "x", b: 2 } }).counters).toEqual({ b: 2 });
  });
});

describe("consumables and the weekly special", () => {
  it("spends coins on a consumable without owning it", () => {
    const s = emptyStats();
    s.coins = 100;
    const now = at(2026, 10, 28);
    expect(buy(s, "snacks", now)).toBe(true);
    expect(buy(s, "snacks", now)).toBe(true);
    expect(s.owned).not.toContain("snacks");
    expect(s.counters["used:snacks"]).toBe(2);
    expect(s.coins).toBe(20);
  });

  it("sells one piece of furniture 30% off each week, the same all week", () => {
    const mon = at(2026, 10, 26);
    const id = weeklySpecial(mon);
    expect(weeklySpecial(at(2026, 10, 30))).toBe(id);
    const item = SHOP.find((i) => i.id === id)!;
    expect(item.place).not.toBe("use");
    expect(priceOf(item, mon)).toBe(Math.round(item.price * 0.7));
    const other = SHOP.find((i) => i.id !== id)!;
    expect(priceOf(other, mon)).toBe(other.price);
  });

  it("skips quests only the activity log could count while it is off", () => {
    for (let d = 1; d <= 28; d++) {
      const day = `2026-10-${String(d).padStart(2, "0")}`;
      const keys = questsFor(day, false).map((id) => questDef(id)!.key);
      expect(keys).not.toContain("add");
      expect(keys).not.toContain("testPass");
    }
  });
});
