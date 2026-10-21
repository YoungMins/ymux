// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { aggregateUsage, sumTotals, dailyTotals, groupProvidersByAccount, heatmapGrid, type ProviderUsage, type TokenUsage, type UsageRow } from "./model";
import { TokenMonitorPane } from "./TokenMonitor";
import { api } from "../ipc/bridge";
import { setLang, translationGaps } from "../i18n/i18n";
const totals = (total: number) => ({ input: total - 2, output: 2, cache_read: 0, cache_write: 0, total });
const row = (provider: string, model: string, project: string, total: number, account = ""): UsageRow => ({ account, provider, model, project, five_hour: totals(total), seven_day: totals(total * 2), all_time: totals(total * 3) });
const data: TokenUsage = {
  collected_at: 1700000000,
  providers: [{ id: "claude", account: "", also: [], warnings: [], auth_status: "credentials_present", five_hour: totals(20), seven_day: totals(40), all_time: totals(60), quotas: [], files: 1, skipped: 0 }],
  rows: [row("claude", "opus", "<script>unsafe</script>", 20)], hourly: [], warnings: [],
};
let active: TokenMonitorPane | null = null;
function mount(): void {
  active = new TokenMonitorPane({ id: "usage-test" });
  vi.spyOn(active.element, "getClientRects").mockReturnValue({ length: 1, item: () => null, [Symbol.iterator]: () => [new DOMRect()].values() });
  document.body.append(active.element); void active.spawn();
}
afterEach(() => {
  active?.dispose(); active = null;
  document.body.replaceChildren(); localStorage.clear(); vi.restoreAllMocks(); vi.useRealTimers();
  setLang("en");
});
describe("token usage aggregation", () => {
  it("sums shared projects across providers in the selected window", () => {
    const rows = [row("claude", "opus", "/app", 20), row("codex", "gpt", "/app", 10)];
    const result = aggregateUsage(rows, "project", "seven_day");
    expect(result).toEqual([{ label: "/app", tokens: { input: 56, output: 4, cache_read: 0, cache_write: 0, total: 60 } }]);
  });
  it("keeps identically named models separate across providers", () => {
    const result = aggregateUsage([row("claude", "same", "/a", 10), row("codex", "same", "/b", 20)], "model", "five_hour");
    expect(result.map(item => item.label)).toEqual(["codex / same", "claude / same"]);
  });
  it("totals every row of the selected window", () => {
    const rows = aggregateUsage([row("claude", "opus", "/a", 20), row("codex", "gpt", "/b", 10)], "project", "all_time");
    expect(sumTotals(rows)).toEqual({ input: 86, output: 4, cache_read: 0, cache_write: 0, total: 90 });
    expect(sumTotals([]).total).toBe(0);
  });
  it("shows the window's token total above the table", async () => {
    vi.spyOn(api, "getTokenUsage").mockResolvedValue({ ...data, rows: [row("claude", "opus", "/a", 20), row("codex", "gpt", "/b", 10)] });
    mount(); await vi.waitFor(() => expect(active!.element.querySelector(".usage-sum")).not.toBeNull());
    const select = active!.element.querySelector<HTMLSelectElement>(".usage-controls select")!;
    select.value = "all_time"; select.dispatchEvent(new Event("change"));
    const sum = active!.element.querySelector(".usage-sum")!;
    expect(sum.querySelector("strong")!.textContent).toBe("90 tokens");
    expect(sum.textContent).toContain("All recorded usage");
  });
  it("translates every token panel key into all 13 languages", () => {
    expect(translationGaps("usage.")).toEqual([]);
  });
});
describe("groupProvidersByAccount", () => {
  const card = (id: string, account: string, also: string[] = []): ProviderUsage => ({ ...data.providers[0]!, id, account, also });
  it("keeps one group, in order, for a single account", () => {
    const groups = groupProvidersByAccount([card("claude", ""), card("codex", "")]);
    expect(groups.map(g => [g.account, g.providers.map(p => p.id)])).toEqual([["", ["claude", "codex"]]]);
  });
  it("groups cards by account in order of first appearance and merges shared-folder accounts", () => {
    const groups = groupProvidersByAccount([card("claude", "", ["work"]), card("claude", "work"), card("codex", "", ["work", "x"]), card("codex", "work")]);
    expect(groups.map(g => g.account)).toEqual(["", "work"]);
    expect(groups[0]?.also).toEqual(["work", "x"]);
    expect(groups[1]?.providers.map(p => p.id)).toEqual(["claude", "codex"]);
  });
  it("sums rows across accounts when grouped by project", () => {
    const rows = [row("claude", "m", "/a", 10, ""), row("claude", "m", "/a", 5, "work")];
    expect(aggregateUsage(rows, "project", "five_hour")[0]?.tokens.total).toBe(15);
  });
});
describe("token monitor panel", () => {
  it("shows a short countdown per cycle with the absolute reset time in the tooltip and updates it every ten seconds", async () => {
    // Given a current snapshot with different future reset times.
    vi.useFakeTimers(); vi.setSystemTime(new Date("2026-10-09T00:00:00Z"));
    const now = Date.now() / 1000;
    const provider = data.providers[0];
    if (!provider) throw new Error("Provider fixture missing");
    vi.spyOn(api, "getTokenUsage").mockResolvedValue({ ...data, providers: [{ ...provider, quotas: [
      { window_minutes: 300, used_percent: 75, resets_at: now + 3661, observed_at: now },
      { window_minutes: 10080, used_percent: 25, resets_at: now + 86461, observed_at: now },
    ] }] });
    // When the visible panel renders and advances one polling interval.
    mount(); await Promise.resolve();
    const times = [...document.querySelectorAll<HTMLTimeElement>("time.usage-reset")];
    expect(times.map(time => time.dateTime)).toEqual(["2026-10-09T01:01:01.000Z", "2026-10-10T00:01:01.000Z"]);
    expect(times[0]?.title).toContain(new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "medium" }).format(new Date((now + 3661) * 1000)));
    expect(times.map(time => time.textContent)).toEqual(["1h 2m", "1d"]);
    const before = times[0]?.textContent;
    await vi.advanceTimersByTimeAsync(10_000);
    // Then the remaining duration changes without hiding the reset date.
    expect(document.querySelector(".usage-reset")?.textContent).not.toBe(before);
    expect(document.querySelector(".usage-reset")?.getAttribute("aria-label")).toContain("Current 5-hour cycle");
    const display = document.querySelector<HTMLSelectElement>(".usage-display-select");
    if (!display) throw new Error("Display selector missing");
    display.value = "used"; display.dispatchEvent(new Event("change"));
    expect(document.querySelector(".usage-card strong")?.textContent).toBe("75%");
    expect(document.querySelector<HTMLTimeElement>("time.usage-reset")?.dateTime).toBe(times[0]?.dateTime);
    setLang("ko");
    expect(document.querySelector(".usage-reset")?.getAttribute("title")).toContain("초기화 시각");
  });
  it("shows unknown reset times for missing or undated cycles", async () => {
    const provider = data.providers[0];
    if (!provider) throw new Error("Provider fixture missing");
    vi.spyOn(api, "getTokenUsage").mockResolvedValue({ ...data, providers: [{ ...provider, quotas: [{ window_minutes: 300, used_percent: 75, resets_at: null, observed_at: Date.now() / 1000 }] }] });
    mount();
    await vi.waitFor(() => expect(document.querySelectorAll(".usage-reset")).toHaveLength(2));
    expect([...document.querySelectorAll(".usage-reset")].map(item => item.textContent)).toEqual(["Unknown", "Unknown"]);
  });
  it("labels expired reset snapshots without claiming the next cycle is available", async () => {
    const provider = data.providers[0];
    if (!provider) throw new Error("Provider fixture missing");
    const past = Date.now() / 1000 - 10;
    vi.spyOn(api, "getTokenUsage").mockResolvedValue({ ...data, providers: [{ ...provider, quotas: [300, 10080].map(window_minutes => ({ window_minutes, used_percent: 75, resets_at: past, observed_at: past - 100 })) }] });
    mount();
    await vi.waitFor(() => expect(document.querySelectorAll(".usage-reset")).toHaveLength(2));
    expect([...document.querySelectorAll(".usage-reset")].every(item => item.textContent === "Recorded reset time has passed")).toBe(true);
    expect(document.querySelector(".usage-card strong")?.textContent).toBe("—");
    expect([...document.querySelectorAll(".usage-meter-row [role=progressbar]")].every(item => item.getAttribute("aria-label")?.includes("—"))).toBe(true);
  });
  it("changes quota headline, meters and labels together and remembers the selection", async () => {
    const provider = data.providers[0];
    if (!provider) throw new Error("Provider fixture missing");
    vi.spyOn(api, "getTokenUsage").mockResolvedValue({ ...data, providers: [{ ...provider, quotas: [{ window_minutes: 300, used_percent: 75, resets_at: null, observed_at: Date.now() / 1000 }] }] });
    mount();
    await vi.waitFor(() => expect(document.querySelector(".usage-card strong")?.textContent).toBe("25%"));
    const select = document.querySelector<HTMLSelectElement>(".usage-display-select");
    if (!select) throw new Error("Quota display selector missing");
    select.value = "used"; select.dispatchEvent(new Event("change"));
    expect(document.querySelector(".usage-card strong")?.textContent).toBe("75%");
    expect(document.querySelector("[role=progressbar]")?.getAttribute("aria-valuenow")).toBe("75");
    expect(document.querySelector("[role=progressbar]")?.getAttribute("aria-label")).toContain("Used quota");
    active?.dispose(); mount();
    expect(document.querySelector<HTMLSelectElement>(".usage-display-select")?.value).toBe("used");
  });
  it("does not read usage when spawned as a hidden tab", async () => {
    const read = vi.spyOn(api, "getTokenUsage").mockResolvedValue(data);
    active = new TokenMonitorPane({ id: "hidden" });
    document.body.append(active.element); active.element.classList.add("pane--tab-hidden");
    await active.spawn();
    expect(read).not.toHaveBeenCalled();
  });
  it("spawns once and stops polling a hidden tab", async () => {
    vi.useFakeTimers();
    const read = vi.spyOn(api, "getTokenUsage").mockResolvedValue(data);
    mount(); await Promise.resolve();
    await active?.spawn();
    active?.element.classList.add("pane--tab-hidden");
    await vi.advanceTimersByTimeAsync(120000);
    expect(read).toHaveBeenCalledTimes(1);
  });
  it("hands its title chrome to a tab group and restores it", () => {
    const pane = new TokenMonitorPane({ id: "chrome", title: "Custom", ownChrome: false });
    active = pane; document.body.append(pane.element);
    expect(pane.element.querySelector<HTMLElement>(".pane-title")?.hidden).toBe(true);
    pane.setOwnChrome(true);
    expect(pane.element.querySelector<HTMLElement>(".pane-title")?.textContent).toBe("Custom");
    expect(pane.element.querySelector<HTMLElement>(".pane-title")?.hidden).toBe(false);
  });
  it("keeps a refresh error visible and translated after switching language", async () => {
    const read = vi.spyOn(api, "getTokenUsage").mockResolvedValueOnce(data).mockRejectedValueOnce(new Error("unavailable"));
    vi.spyOn(console, "warn").mockImplementation(() => undefined);
    mount();
    await vi.waitFor(() => expect(document.querySelector("tbody")).not.toBeNull());
    const refresh = [...document.querySelectorAll("button")].find(button => button.getAttribute("aria-label") === "Refresh");
    refresh?.click();
    await vi.waitFor(() => expect(document.querySelector('[role="status"]')?.textContent).toContain("Could not read"));
    setLang("ko");
    expect(document.querySelector('[role="status"]')?.textContent).toContain("토큰 기록을 읽지 못했습니다");
    expect(document.querySelector("select")?.getAttribute("aria-label")).toBe("AI 토큰 사용량");
    expect(read).toHaveBeenCalledTimes(2);
  });
  it("changes the selected window without another IPC read", async () => {
    const read = vi.spyOn(api, "getTokenUsage").mockResolvedValue(data);
    mount();
    await vi.waitFor(() => expect(document.querySelector(".usage-local-tokens")?.textContent).toContain("20"));
    const select = document.querySelector("select");
    if (!select) throw new Error("Window selector missing");
    select.value = "seven_day"; select.dispatchEvent(new Event("change"));
    expect(document.querySelector(".usage-local-tokens")?.textContent).toContain("40");
    expect(document.querySelector(".usage-card strong")?.textContent).toBe("—");
    expect(read).toHaveBeenCalledTimes(1);
  });
  it("marks an expired quota snapshot instead of implying a current cycle", async () => {
    const provider = data.providers[0];
    if (!provider) throw new Error("Provider fixture missing");
    vi.spyOn(api, "getTokenUsage").mockResolvedValue({ ...data, providers: [{ ...provider, quotas: [{ window_minutes: 300, used_percent: 75, resets_at: data.collected_at - 1, observed_at: data.collected_at }] }] });
    mount();
    await vi.waitFor(() => expect(document.querySelector(".usage-stale")?.getAttribute("title")).toContain("Expired or stale snapshot"));
  });
  it("renders safe text and stays mounted on Escape", async () => {
    vi.spyOn(api, "getTokenUsage").mockResolvedValue(data);
    const launcher = document.createElement("button"); document.body.append(launcher); launcher.focus();
    mount();
    await vi.waitFor(() => expect(document.querySelector("tbody")?.textContent).toContain("<script>unsafe</script>"));
    expect(document.querySelector(".token-monitor-pane script")).toBeNull();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.querySelector(".token-monitor-pane")).not.toBeNull();
  });
  it("stops refreshing after closing the panel", async () => {
    vi.useFakeTimers();
    const read = vi.spyOn(api, "getTokenUsage").mockResolvedValue(data);
    mount(); await Promise.resolve();
    active?.dispose();
    await vi.advanceTimersByTimeAsync(120000);
    expect(read).toHaveBeenCalledTimes(1);
  });
  it("shows a recoverable message after an IPC read failure", async () => {
    vi.spyOn(api, "getTokenUsage").mockRejectedValue(new Error("<unsafe>"));
    vi.spyOn(console, "warn").mockImplementation(() => undefined);
    mount();
    await vi.waitFor(() => expect(document.querySelector('[role="status"]')?.textContent).toContain("Could not read"));
    expect(document.querySelector('[role="status"]')?.textContent).not.toContain("unsafe");
  });
});
describe("daily activity heatmap", () => {
  const hourAt = (y: number, m: number, d: number, h: number): number => Math.floor(new Date(y, m - 1, d, h).getTime() / 3600_000);
  it("groups hour buckets into local calendar days and filters by provider", () => {
    const hourly = [
      { hour: hourAt(2026, 3, 8, 0), account: "", provider: "claude", total: 5 },
      { hour: hourAt(2026, 3, 8, 23), account: "", provider: "codex", total: 7 },
      { hour: hourAt(2026, 3, 9, 0), account: "", provider: "claude", total: 11 },
      { hour: hourAt(2026, 11, 1, 12), account: "", provider: "claude", total: 1 },
    ];
    expect([...dailyTotals(hourly, null)]).toEqual([["2026-03-08", 12], ["2026-03-09", 11], ["2026-11-01", 1]]);
    expect([...dailyTotals(hourly, "codex")]).toEqual([["2026-03-08", 7]]);
  });
  it("lays out Sunday-first week columns, omits future days and ends on today", () => {
    const grid = heatmapGrid(new Map(), new Date(2026, 9, 14), 3); // Wednesday
    expect(grid.map(c => c.length)).toEqual([7, 7, 4]);
    expect(grid[0]?.[0]?.date).toBe("2026-09-27");
    expect(grid[2]?.[3]?.date).toBe("2026-10-14");
    expect(grid.flat().every(cell => cell.level === 0)).toBe(true);
  });
  it("assigns levels by quartiles of non-zero days", () => {
    const daily = new Map([["2026-10-11", 10], ["2026-10-12", 20], ["2026-10-13", 30], ["2026-10-14", 40], ["2026-10-15", 1000]]);
    const levels = new Map(heatmapGrid(daily, new Date(2026, 9, 17), 1).flat().map(cell => [cell.date, cell.level]));
    expect([11, 12, 13, 14, 15, 16].map(d => levels.get(`2026-10-${d}`))).toEqual([1, 1, 2, 3, 4, 0]);
  });
});
describe("activity section", () => {
  it("renders a labelled heatmap with localized tooltips and legend", async () => {
    const hour = Math.floor(Date.now() / 3600_000);
    vi.spyOn(api, "getTokenUsage").mockResolvedValue({ ...data, hourly: [{ hour, account: "", provider: "claude", total: 1500 }, { hour, account: "", provider: "codex", total: 500 }] });
    mount(); await Promise.resolve(); await Promise.resolve();
    expect(document.querySelector(".usage-activity")?.getAttribute("aria-label")).toContain("Daily activity: 2K");
    const cell = [...document.querySelectorAll<HTMLElement>(".usage-heat-cell")].find(c => c.title.includes("Codex"));
    expect(cell?.title).toContain("Claude Code: 1.5K");
    expect(document.querySelector(".usage-heat-legend")?.textContent).toBe("LessMore");
  });
});
describe("accounts on the panel", () => {
  const twoAccounts = (): TokenUsage => {
    const provider = data.providers[0];
    if (!provider) throw new Error("Provider fixture missing");
    return { ...data, providers: [provider, { ...provider, account: "work", warnings: ["quota_unavailable"] }] };
  };
  it("renders no account headings for a single account", async () => {
    vi.spyOn(api, "getTokenUsage").mockResolvedValue(data);
    mount();
    await vi.waitFor(() => expect(document.querySelector(".usage-card")).not.toBeNull());
    expect(document.querySelector(".usage-account-heading")).toBeNull();
  });
  it("renders a heading per account and reads quota warnings per card", async () => {
    vi.spyOn(api, "getTokenUsage").mockResolvedValue(twoAccounts());
    mount();
    await vi.waitFor(() => expect(document.querySelectorAll(".usage-account-heading").length).toBe(2));
    expect(document.querySelectorAll(".usage-account-heading")[0]?.textContent).toBe("Default account");
    const cards = [...document.querySelectorAll(".usage-card")];
    expect(cards.length).toBe(2);
    expect(cards[0]?.querySelector(".usage-stale")).toBeNull();
    expect(cards[1]?.querySelector(".usage-stale")).not.toBeNull();
  });
});
