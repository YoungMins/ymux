import { describe, expect, it } from "vitest";
import { handoffNote, quotaAlert, quotaPollWanted, quotaLevel, rankSwitchTargets, switchCandidates, usedPercent } from "./quotaModel";
import type { ProviderUsage } from "../tokenmonitor/model";

const NOW = 1_000_000;
const zero = { input: 0, output: 0, cache_read: 0, cache_write: 0, total: 0 };
const card = (
  id: string,
  account: string,
  windows: [number, number, number?][],
  also: string[] = [],
): ProviderUsage => ({
  id,
  account,
  also,
  auth_status: "credentials_present",
  five_hour: zero,
  seven_day: zero,
  all_time: zero,
  quotas: windows.map(([window_minutes, used_percent, resets_at]) => ({
    window_minutes,
    used_percent,
    resets_at: resets_at === undefined ? NOW + 3600 : resets_at,
    observed_at: NOW,
  })),
  files: 0,
  skipped: 0,
  warnings: [],
});

describe("quotaLevel", () => {
  it("is near from 90 and at limit from 100", () => {
    expect([null, 0, 89.9, 90, 99.9, 100, 140].map(quotaLevel)).toEqual(["ok", "ok", "ok", "near", "near", "limit", "limit"]);
    expect(quotaLevel(Number.NaN)).toBe("ok");
  });
});

describe("usedPercent", () => {
  it("takes the tightest live window of that CLI and account", () => {
    const ps = [card("claude", "a", [[300, 40], [10080, 95]]), card("codex", "a", [[300, 100]])];
    expect(usedPercent(ps, "a", "claude", NOW)).toBe(95);
    expect(usedPercent(ps, "a", "codex", NOW)).toBe(100);
    expect(usedPercent(ps, "b", "claude", NOW)).toBeNull();
  });
  it("ignores a window that has already reset", () => {
    const ps = [card("claude", "a", [[300, 100, NOW - 1], [10080, 20]])];
    expect(usedPercent(ps, "a", "claude", NOW)).toBe(20);
  });
  it("finds an account that shares the owner's folder", () => {
    const ps = [card("claude", "", [[300, 50]], ["none"])];
    expect(usedPercent(ps, "none", "claude", NOW)).toBe(50);
  });
});

describe("rankSwitchTargets", () => {
  const ps = [
    card("claude", "a", [[300, 95]]),
    card("claude", "b", [[300, 30]]),
    card("claude", "c", [[300, 70], [10080, 10]]),
    card("claude", "", [[300, 5]], ["none"]),
  ];
  it("lists best headroom first, unknown last, never the current or a folder-mate", () => {
    const r = rankSwitchTargets(ps, ["none", "a", "b", "c", "d"], "a", "claude", NOW);
    expect(r.map((o) => o.account)).toEqual(["none", "b", "c", "d"]);
    expect(r[0]).toMatchObject({ used: 5, headroom: 95, level: "ok" });
    expect(r[3]).toMatchObject({ used: null, headroom: null });
  });
  it("drops the account that shares the current one's quota", () => {
    expect(rankSwitchTargets(ps, ["none", "b"], "", "claude", NOW).map((o) => o.account)).toEqual(["b"]);
  });
});

describe("quotaAlert", () => {
  const cands = ["none", "a", "b"];
  it("is silent while the account has room", () => {
    expect(quotaAlert([card("claude", "a", [[300, 50]]), card("claude", "b", [[300, 10]])], cands, "a", "claude", NOW)).toBeNull();
  });
  it("warns near and at limit when another account has headroom", () => {
    const base = card("claude", "b", [[300, 10]]);
    expect(quotaAlert([card("claude", "a", [[300, 92]]), base], cands, "a", "claude", NOW)).toMatchObject({ level: "near", used: 92 });
    expect(quotaAlert([card("claude", "a", [[300, 100]]), base], cands, "a", "claude", NOW)?.level).toBe("limit");
  });
  it("is silent with no other account, or none with headroom", () => {
    expect(quotaAlert([card("claude", "a", [[300, 99]])], ["a"], "a", "claude", NOW)).toBeNull();
    expect(quotaAlert([card("claude", "a", [[300, 99]]), card("claude", "b", [[300, 95]])], cands, "a", "claude", NOW)).toBeNull();
    // An unknown quota is not headroom.
    expect(quotaAlert([card("claude", "a", [[300, 99]])], cands, "a", "claude", NOW)).toBeNull();
  });
  it("looks at the pane's own CLI only", () => {
    const ps = [card("claude", "a", [[300, 99]]), card("claude", "b", [[300, 5]]), card("codex", "a", [[300, 1]]), card("codex", "b", [[300, 99]])];
    expect(quotaAlert(ps, cands, "a", "codex", NOW)).toBeNull();
    expect(quotaAlert(ps, cands, "a", "claude", NOW)?.options[0].account).toBe("b");
  });
});

describe("switchCandidates / handoffNote", () => {
  it("offers the default login and every profile once", () => {
    expect(switchCandidates(["a", "", "none", "b"])).toEqual(["none", "a", "b"]);
  });
  it("builds a short note with what is known", () => {
    const full = handoffNote({ cwd: "D:\\Git\\x", branch: "feat/y" });
    expect(full).toContain("Working directory: D:\\Git\\x");
    expect(full).toContain("Branch: feat/y");
    expect(full).toContain("git status");
    expect(full).toContain("git diff");
    const bare = handoffNote({ cwd: null, branch: null });
    expect(bare).not.toContain("Working directory");
    expect(bare).not.toContain("Branch");
  });
});

describe("quotaPollWanted", () => {
  it("needs a profile and a live agent that uses accounts", () => {
    expect(quotaPollWanted(0, ["claude"])).toBe(false);
    expect(quotaPollWanted(2, [])).toBe(false);
    expect(quotaPollWanted(1, ["claude"])).toBe(true);
    expect(quotaPollWanted(1, ["codex", "opencode"])).toBe(true);
  });
});
