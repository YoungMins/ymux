import { describe, expect, it } from "vitest";
import {
  LONG_TOOL_MS,
  SILENT_MS,
  approveOption,
  errandIcon,
  errandSpot,
  nextWaiting,
  quotaMeters,
  shortDuration,
  stuckReason,
  waitQueue,
  wanderSpot,
  type Waiting,
} from "./officeQueue";
import { OfficeLog } from "./officeLog";
import { TASK_XP, accountXp } from "./heroModel";
import type { ProviderUsage } from "../tokenmonitor/model";

const w = (paneId: string, since: number | null): Waiting => ({ paneId, ws: 1, label: paneId, since });

describe("wait queue", () => {
  it("puts the longest wait first and unknown starts last", () => {
    expect(waitQueue([w("a", 300), w("b", null), w("c", 100), w("d", 200)]).map((x) => x.paneId)).toEqual(["c", "d", "a", "b"]);
  });

  it("cycles to the next waiting pane", () => {
    const q = waitQueue([w("a", 1), w("b", 2), w("c", 3)]);
    expect(nextWaiting(q, null)?.paneId).toBe("a");
    expect(nextWaiting(q, "a")?.paneId).toBe("b");
    expect(nextWaiting(q, "c")?.paneId).toBe("a");
    expect(nextWaiting(q, "gone")?.paneId).toBe("a");
    expect(nextWaiting([], null)).toBeNull();
  });

  it("formats short durations", () => {
    expect(shortDuration(45_000)).toBe("45s");
    expect(shortDuration(12 * 60_000 + 5_000)).toBe("12m");
    expect(shortDuration(65 * 60_000)).toBe("1h 5m");
    expect(shortDuration(120 * 60_000)).toBe("2h");
  });
});

describe("approve all", () => {
  it("finds a plain yes as the first choice", () => {
    expect(approveOption(["Run this command?", "❯ 1. Yes", "  2. Yes, and don't ask again", "  3. No, and tell Claude what to do differently (esc)"])?.n).toBe("1");
    expect(approveOption(["› 1. 예", "  2. 아니요"])?.n).toBe("1");
  });

  it("leaves other menus and plain screens alone", () => {
    expect(approveOption(["❯ 1. No", "  2. Yes"])).toBeNull();
    expect(approveOption(["❯ 1. Type something", "  2. Yes"])).toBeNull();
    expect(approveOption(["Steps:", "1. Yes do it", "2. More"])).toBeNull();
    expect(approveOption(null)).toBeNull();
  });
});

describe("stuck agents", () => {
  const now = 10_000_000;
  it("notices silence and a long tool call while working", () => {
    expect(stuckReason({ working: true, tool: null, outputAt: now - SILENT_MS, toolSince: null, now })).toBe("silent");
    expect(stuckReason({ working: true, tool: "Bash", outputAt: now - 1000, toolSince: now - LONG_TOOL_MS, now })).toBe("tool");
    expect(stuckReason({ working: true, tool: "Bash", outputAt: now - 1000, toolSince: now - 1000, now })).toBeNull();
    expect(stuckReason({ working: false, tool: null, outputAt: 1, toolSince: null, now })).toBeNull();
    expect(stuckReason({ working: true, tool: null, outputAt: 0, toolSince: null, now })).toBeNull();
  });
});

describe("quota meters", () => {
  const card = (id: string, account: string, quotas: [number, number][]): ProviderUsage =>
    ({
      id,
      account,
      also: [],
      auth_status: "credentials_present",
      five_hour: {} as never,
      seven_day: {} as never,
      all_time: {} as never,
      quotas: quotas.map(([window_minutes, used_percent]) => ({ window_minutes, used_percent, resets_at: null, observed_at: 0 })),
      files: 0,
      skipped: 0,
      warnings: [],
    }) as ProviderUsage;

  it("shows the tightest quota per provider and window", () => {
    const m = quotaMeters([card("claude", "", [[10080, 20], [300, 40]]), card("claude", "work", [[300, 90]]), card("codex", "", [[300, 5]])], 0);
    expect(m).toEqual([
      { provider: "claude", window: "5h", left: 10 },
      { provider: "claude", window: "7d", left: 80 },
      { provider: "codex", window: "5h", left: 95 },
    ]);
  });
});

describe("busy errands", () => {
  it("sends an agent to the shelf, the machines or the window by tool", () => {
    expect(errandIcon("Read")).toBe("book");
    expect(errandIcon("Grep")).toBe("book");
    expect(errandIcon("Bash")).toBe("gear");
    expect(errandIcon("WebFetch")).toBe("globe");
    expect(errandIcon("WebSearch")).toBe("globe");
    expect(errandIcon("TodoWrite")).toBe("paper");
    expect(errandIcon("Task")).toBe("chat");
    expect(errandIcon("Edit")).toBe("pencil");
    expect(errandIcon(null)).toBe("pencil");
  });

  it("keeps errand and wander spots inside the room", () => {
    const room = { x: 0.1, y: 0.1, w: 0.4, h: 0.5 };
    for (const r of [0, 0.5, 0.999]) {
      const p = errandSpot(room, 1000, 800, () => r);
      expect(p.x).toBeGreaterThan(room.x);
      expect(p.x).toBeLessThan(room.x + room.w);
      expect(p.y).toBeGreaterThan(room.y);
      expect(p.y).toBeLessThanOrEqual(room.y + room.h * 0.55);
      const q = wanderSpot({ x: 0.3, y: 0.5 }, room, 1000, 800, () => r);
      expect(q.x).toBeGreaterThanOrEqual(room.x);
      expect(q.y).toBeLessThanOrEqual(room.y + room.h);
    }
  });
});

describe("office log timing", () => {
  it("remembers since when an agent is in its status and on its tool", () => {
    let now = 1000;
    const log = new OfficeLog(() => now);
    log.track("p", "working", "Read");
    now = 2000;
    log.track("p", "working", "Bash");
    expect(log.statusSince("p")).toBe(1000);
    expect(log.toolSince("p")).toBe(2000);
    log.track("p", null, null);
    expect(log.statusSince("p")).toBeNull();
  });

  it("describes the newest finished turn of a pane", () => {
    const log = new OfficeLog(() => 5);
    log.done("p", "api", "claude", 65_000);
    log.describeTurn("p", { headline: "Changed 2 files (+10/-3)", files: 2, add: 10, del: 3 });
    const e = log.list()[0];
    expect(e.turn).toEqual({ durationMs: 65_000, headline: "Changed 2 files (+10/-3)", files: 2, add: 10, del: 3 });
  });
});

describe("task experience", () => {
  it("adds finished tasks to an account's experience", () => {
    const xp = accountXp([], { work: 3 });
    expect(xp.get("work")).toBe(3 * TASK_XP);
  });
});
