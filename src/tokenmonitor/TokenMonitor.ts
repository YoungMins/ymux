import { api } from "../ipc/bridge";
import { t, getLang, onLangChange } from "../i18n/i18n";
import type { Pane } from "../layout/Pane";
import type { Uuid } from "../types";
import { aggregateUsage, remainingQuota, type TokenUsage, type UsageWindow, type TokenTotals } from "./model";
import "./tokenmonitor.css";

function el<K extends keyof HTMLElementTagNameMap>(tag: K, text = "", cls = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag); node.textContent = text; node.className = cls; return node;
}
function compact(value: number): string { return new Intl.NumberFormat(getLang(), { notation: "compact", maximumFractionDigits: 1 }).format(value); }
function resetInfo(resetsAt: number | null | undefined, now: number, cycle: string): HTMLElement {
  const info = el("div", "", "usage-reset");
  info.append(el("span", `${t("usage.resets")}: `));
  if (resetsAt === null || resetsAt === undefined) {
    info.append(el("span", t("usage.unknown")));
  } else {
    const date = new Date(resetsAt * 1000);
    const time = el("time", new Intl.DateTimeFormat(getLang(), { dateStyle: "medium", timeStyle: "medium" }).format(date));
    time.dateTime = date.toISOString(); info.append(time);
    const seconds = Math.max(0, Math.ceil(resetsAt - now));
    const units = [[Math.floor(seconds / 3600), "hour"], [Math.floor(seconds % 3600 / 60), "minute"], [seconds % 60, "second"]] as const;
    const duration = units.map(([value, unit]) => new Intl.NumberFormat(getLang(), { style: "unit", unit, unitDisplay: "short" }).format(value)).join(" ");
    info.append(el("span", seconds === 0 ? t("usage.reset_passed") : `${t("usage.reset_in")}: ${duration}`, "usage-reset-countdown"));
  }
  info.setAttribute("aria-label", `${cycle} · ${info.textContent}`);
  return info;
}
function breakdown(tokens: TokenTotals): string {
  return (["total", "input", "output", "cache_read", "cache_write"] as const).map(key => `${t(`usage.${key}`)}: ${tokens[key].toLocaleString()}`).join("\n");
}
export interface TokenMonitorOptions {
  readonly id: Uuid;
  readonly title?: string | null;
  readonly ownChrome?: boolean;
  readonly onFocus?: () => void;
}

export class TokenMonitorPane implements Pane {
  readonly id: Uuid;
  readonly element = el("section", "", "pane token-monitor-pane");
  private readonly titleEl = el("div", "", "pane-title");
  private readonly controls = el("div", "", "usage-controls");
  private readonly state = el("div", "", "usage-state");
  private readonly content = el("div", "", "usage-content");
  private readonly periodSelect = el("select");
  private readonly groupSelect = el("select");
  private readonly displaySelect = el("select", "", "usage-display-select");
  private display: "used" | "remaining" = "remaining";
  private readonly refreshButton = el("button", "↻");
  private readonly cleanupLang: () => void;
  private timer: ReturnType<typeof setInterval> | null = null;
  private disposed = false;
  private busy = false;
  private lastRefresh = Number.NEGATIVE_INFINITY;
  private lastManualRefresh = Number.NEGATIVE_INFINITY;
  private refreshFailed = false;
  private readonly regainVisibility = (): void => { if (this.visible()) { this.render(); void this.refreshIfOld(); } };
  private data: TokenUsage | null = null;
  private phase: "loading" | "error" | "ready" = "loading";
  private period: UsageWindow = "five_hour";
  private group: "provider" | "model" | "project" = "project";
  private title: string | null;

  constructor(private readonly options: TokenMonitorOptions) {
    this.id = options.id; this.title = options.title ?? null;
    this.element.dataset.paneId = this.id; this.element.tabIndex = -1;
    this.element.addEventListener("focusin", () => options.onFocus?.());
    this.element.addEventListener("pointerdown", () => options.onFocus?.());
    this.state.setAttribute("role", "status");
    for (const key of ["five_hour", "seven_day", "all_time"] as const) { const option = el("option"); option.value = key; this.periodSelect.append(option); }
    for (const key of ["provider", "model", "project"] as const) { const option = el("option"); option.value = key; this.groupSelect.append(option); }
    this.groupSelect.value = this.group;
    try { if (localStorage.getItem("ymux.tokenUsage.display") === "used") this.display = "used"; }
    catch (error: unknown) { console.warn("Token display preference unavailable", error); }
    for (const key of ["remaining", "used"] as const) { const option = el("option"); option.value = key; this.displaySelect.append(option); }
    this.displaySelect.value = this.display;
    this.displaySelect.onchange = () => {
      if (this.displaySelect.value !== "used" && this.displaySelect.value !== "remaining") return;
      this.display = this.displaySelect.value;
      try { localStorage.setItem("ymux.tokenUsage.display", this.display); }
      catch (error: unknown) { console.warn("Token display preference could not be saved", error); }
      this.render();
    };
    this.periodSelect.onchange = () => {
      switch (this.periodSelect.value) { case "five_hour": case "seven_day": case "all_time": this.period = this.periodSelect.value; break; }
      this.render();
    };
    this.groupSelect.onchange = () => {
      switch (this.groupSelect.value) { case "provider": case "model": case "project": this.group = this.groupSelect.value; break; }
      this.render();
    };
    this.refreshButton.type = "button"; this.refreshButton.onclick = () => { void this.refresh(true); };
    document.addEventListener("visibilitychange", this.regainVisibility);
    window.addEventListener("focus", this.regainVisibility);
    this.controls.append(this.periodSelect, this.groupSelect, this.displaySelect, this.refreshButton);
    this.element.append(this.titleEl, this.controls, this.state, this.content);
    this.setOwnChrome(options.ownChrome ?? true);
    this.cleanupLang = onLangChange(() => this.translate()); this.translate();
  }
  setOwnChrome(enabled: boolean): void { this.titleEl.hidden = !enabled; }
  setTitle(title: string | null): void { this.title = title; this.titleEl.textContent = title || t("usage.title"); }
  focus(): void { this.element.focus(); this.options.onFocus?.(); }
  scheduleFit(): void { if (this.timer !== null && this.visible()) { this.render(); void this.refreshIfOld(); } }
  private visible(): boolean { return !document.hidden && this.element.isConnected && !this.element.closest(".pane--tab-hidden") && this.element.getClientRects().length > 0; }
  private async refreshIfOld(): Promise<void> { if (performance.now() - this.lastRefresh >= 10_000) await this.refresh(); }
  async spawn(): Promise<void> {
    if (this.disposed || this.timer !== null) return;
    this.timer = setInterval(() => { if (this.visible()) { this.render(); void this.refresh(); } }, 10_000);
    if (this.visible()) await this.refresh();
  }
  dispose(): void {
    this.disposed = true; if (this.timer !== null) clearInterval(this.timer);
    document.removeEventListener("visibilitychange", this.regainVisibility);
    window.removeEventListener("focus", this.regainVisibility);
    this.cleanupLang(); this.element.remove();
  }
  private translate(): void {
    this.setTitle(this.title);
    this.periodSelect.setAttribute("aria-label", t("usage.title")); this.groupSelect.setAttribute("aria-label", t("usage.project"));
    this.refreshButton.title = t("usage.refresh"); this.refreshButton.setAttribute("aria-label", t("usage.refresh"));
    this.displaySelect.setAttribute("aria-label", t("usage.quota_display"));
    for (const option of [...this.periodSelect.options, ...this.groupSelect.options, ...this.displaySelect.options]) option.textContent = t(`usage.${option.value}`);
    this.render();
  }
  private renderState(): void {
    switch (this.phase) {
      case "loading": this.state.textContent = t("usage.loading"); break;
      case "error": this.state.textContent = t("usage.error"); break;
      case "ready": this.state.textContent = t("usage.auto_refresh"); break;
    }
  }
  private render(): void {
    this.renderState(); this.content.replaceChildren(); if (!this.data) return;
    const cards = el("div", "", "usage-cards");
    const now = Date.now() / 1000;
    const quotaLabel = t(this.display === "used" ? "usage.used_quota" : "usage.remaining_quota");
    const displayed = (remaining: number | null): number | null => remaining === null ? null : this.display === "used" ? 100 - remaining : remaining;
    for (const provider of this.data.providers) {
      const card = el("article", "", "usage-card");
      const heading = el("div", "", "usage-provider-heading");
      const name = el("span", provider.id === "claude" ? "Claude Code" : "Codex"); name.title = t(`usage.${provider.auth_status}`);
      const selected = provider.quotas.find(item => item.window_minutes === (this.period === "seven_day" ? 10080 : 300));
      const remaining = displayed(remainingQuota(selected, now));
      const failed = this.refreshFailed || this.data.warnings.some(warning => warning.startsWith(`${provider.id}_quota_`));
      const stale = failed || (selected !== undefined && (remaining === null || now - selected.observed_at > 120));
      const total = el("strong", remaining === null ? "—" : `${remaining.toFixed(0)}%`); total.title = `${quotaLabel}${stale ? ` · ${t("usage.stale")}` : ""}`;
      if (stale) total.classList.add("usage-stale");
      heading.append(name, total); card.append(heading);
      const auth = el("span", t(`usage.${provider.auth_status}`), "usage-auth"); card.append(auth);
      card.append(el("span", `${quotaLabel} · ${t(this.period === "seven_day" ? "usage.cycle_week" : "usage.cycle_five_hour")}${stale ? ` · ${t("usage.stale")}` : ""}`, "usage-quota-caption"));
      const meters = el("div", "", "usage-meters");
      for (const [minutes, key] of [[300, "cycle_five_hour"], [10080, "cycle_week"]] as const) {
        const quota = provider.quotas.find(item => item.window_minutes === minutes);
        const row = el("div", "", "usage-meter-row"); const label = el("span", t(`usage.${key}`));
        const available = displayed(remainingQuota(quota, now));
        const percent = el("span", available === null ? "—" : `${available.toFixed(0)}%`);
        const meter = el("progress"); meter.max = 100;
        if (available === null) meter.style.visibility = "hidden";
        if (quota) {
          meter.value = available ?? 0;
          const stale = failed || available === null || now - quota.observed_at > 120;
          if (stale) { if (available !== null) percent.textContent += " *"; row.classList.add("usage-stale"); }
          row.title = `${quotaLabel}: ${available === null ? "—" : `${available.toFixed(1)}%`}\n${t("usage.observed")}: ${new Date(quota.observed_at * 1000).toLocaleString()}${quota.resets_at === null ? "" : `\n${t("usage.resets")}: ${new Date(quota.resets_at * 1000).toLocaleString()}`}${stale ? `\n${t("usage.stale")}` : ""}`;
        } else { meter.value = 0; row.title = `${quotaLabel}: —`; }
        meter.setAttribute("aria-label", `${quotaLabel} ${t(`usage.${key}`)} ${percent.textContent}`);
        row.append(label, meter, percent);
        const cycle = el("div", "", "usage-cycle");
        cycle.append(row, resetInfo(quota?.resets_at, now, t(`usage.${key}`))); meters.append(cycle);
      }
      card.append(meters);
      const observed = provider.quotas.reduce((latest, quota) => Math.max(latest, quota.observed_at), 0);
      card.append(el("span", `${t("usage.observed")}: ${observed ? new Date(observed * 1000).toLocaleTimeString() : "—"}`, "usage-auth"));
      const local = el("span", `${t("usage.local_tokens")}: ${compact(provider[this.period].total)}`, "usage-local-tokens");
      local.title = breakdown(provider[this.period]); card.append(local); cards.append(card);
    }
    this.content.append(cards);
    const rows = aggregateUsage(this.data.rows, this.group, this.period).filter(row => row.tokens.total > 0);
    const table = el("table"); const head = el("thead"); const heading = el("tr"); heading.append(el("th", t(`usage.${this.group}`)), el("th", t("usage.total"))); head.append(heading); table.append(head);
    const body = el("tbody");
    for (const row of rows) {
      const tr = el("tr"); const label = this.group === "provider" ? (row.label === "claude" ? "Claude Code" : "Codex") : row.label;
      const cell = el("td", label === "unknown" ? t("usage.unknown") : label); cell.title = label;
      const total = el("td", compact(row.tokens.total)); total.title = breakdown(row.tokens); tr.append(cell, total); body.append(tr);
    }
    table.append(body); this.content.append(rows.length ? table : el("p", t("usage.empty"), "usage-empty"));
    const details = el("details", "", "usage-details"); details.append(el("summary", this.data.warnings.length ? "⚠ ⓘ" : "ⓘ")); details.querySelector("summary")?.setAttribute("aria-label", t("usage.limitations"));
    details.append(el("p", t("usage.limitations")), el("p", t("usage.history")));
    for (const warning of this.data.warnings) details.append(el("p", t(`usage.${warning}`)));
    this.content.append(details);
  }
  private async refresh(refreshQuota = false): Promise<void> {
    if (this.busy || this.disposed || (refreshQuota && performance.now() - this.lastManualRefresh < 10_000)) return;
    this.lastRefresh = performance.now(); if (refreshQuota) this.lastManualRefresh = this.lastRefresh;
    this.busy = true; this.refreshButton.disabled = true;
    this.phase = "loading"; this.renderState();
    try { const next = await api.getTokenUsage(refreshQuota); if (!this.disposed) { this.data = next; this.refreshFailed = false; this.phase = "ready"; this.render(); } }
    catch (error: unknown) { if (!this.disposed) { this.refreshFailed = true; this.phase = "error"; this.render(); } console.warn("Token usage read failed", error); }
    finally { this.busy = false; this.refreshButton.disabled = false; }
  }
}
