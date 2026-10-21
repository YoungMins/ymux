import { api } from "../ipc/bridge";
import { t, getLang, onLangChange } from "../i18n/i18n";
import type { Pane } from "../layout/Pane";
import type { Uuid } from "../types";
import { getAccounts, normalizeColor, ACCOUNT_NONE } from "../accounts/accountModel";
import { aggregateUsage, sumTotals, dailyTotals, groupProvidersByAccount, heatmapGrid, remainingQuota, type HourBucket, type ProviderUsage, type TokenUsage, type UsageWindow, type TokenTotals } from "./model";
import "./tokenmonitor.css";

function el<K extends keyof HTMLElementTagNameMap>(tag: K, text = "", cls = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag); node.textContent = text; node.className = cls; return node;
}
function compact(value: number): string { return new Intl.NumberFormat(getLang(), { notation: "compact", maximumFractionDigits: 1 }).format(value); }
function resetInfo(resetsAt: number | null | undefined, now: number, cycle: string): HTMLElement {
  const info = el("time", "", "usage-reset");
  if (resetsAt === null || resetsAt === undefined) { info.textContent = t("usage.unknown"); }
  else {
    const date = new Date(resetsAt * 1000);
    info.dateTime = date.toISOString(); info.title = `${t("usage.resets")}: ${new Intl.DateTimeFormat(getLang(), { dateStyle: "medium", timeStyle: "medium" }).format(date)}`;
    const minutes = Math.ceil(Math.max(0, resetsAt - now) / 60);
    const units = [[Math.floor(minutes / 1440), "day"], [Math.floor(minutes % 1440 / 60), "hour"], [minutes % 60, "minute"]] as const;
    const first = Math.max(0, units.findIndex(([value]) => value > 0));
    const parts = units.slice(first, first + 2).filter(([value], index) => index === 0 || value > 0);
    info.textContent = minutes === 0 ? t("usage.reset_passed") : parts.map(([value, unit]) => new Intl.NumberFormat(getLang(), { style: "unit", unit, unitDisplay: "narrow" }).format(value)).join(" ");
  }
  info.setAttribute("aria-label", [cycle, info.textContent, info.title].filter(Boolean).join(" · "));
  return info;
}
function breakdown(tokens: TokenTotals): string {
  return (["total", "input", "output", "cache_read", "cache_write"] as const).map(key => `${t(`usage.${key}`)}: ${tokens[key].toLocaleString()}`).join("\n");
}
const HEAT_CELL = 9, HEAT_GAP = 2, HEAT_LABEL_W = 16, HEAT_MAX_WEEKS = 53;
/** Trailing weeks that fit in `width` px (content box); an unmeasured pane gets a modest default. */
function heatWeeks(width: number): number {
  if (width <= 0) return 20;
  return Math.max(4, Math.min(HEAT_MAX_WEEKS, Math.floor((width - HEAT_LABEL_W + HEAT_GAP) / (HEAT_CELL + HEAT_GAP))));
}
function heatmap(hourly: readonly HourBucket[], weeks: number): HTMLElement {
  const lang = getLang();
  const columns = heatmapGrid(dailyTotals(hourly, null), new Date(), weeks);
  const providers = (["claude", "codex"] as const).map(id => [id === "claude" ? "Claude Code" : "Codex", dailyTotals(hourly, id)] as const).filter(([, days]) => days.size > 0);
  const dateFormat = new Intl.DateTimeFormat(lang, { dateStyle: "medium" });
  const monthFormat = new Intl.DateTimeFormat(lang, { month: "short" });
  const weekdayFormat = new Intl.DateTimeFormat(lang, { weekday: "narrow" });
  const section = el("section", "", "usage-activity"); section.style.setProperty("--cols", String(columns.length));
  const months = el("div", "", "usage-heat-months"); const days = el("div", "", "usage-heat-days"); const grid = el("div", "", "usage-heat-grid");
  for (let day = 0; day < 7; day++) days.append(el("span", day % 2 === 1 ? weekdayFormat.format(new Date(2023, 0, 1 + day)) : ""));
  const cells = document.createDocumentFragment(); const labels = document.createDocumentFragment();
  let lastMonth = -1, lastLabel = -3, sum = 0, active = 0;
  columns.forEach((column, index) => {
    const start = column[0]; if (!start) return;
    const month = Number(start.date.slice(5, 7));
    if (month !== lastMonth) {
      lastMonth = month;
      if (index - lastLabel >= 3 && columns.length - index >= 2) {
        lastLabel = index;
        const label = el("span", monthFormat.format(new Date(Number(start.date.slice(0, 4)), month - 1, 1)));
        label.style.gridColumn = `${index + 1} / span ${Math.min(3, columns.length - index)}`; labels.append(label);
      }
    }
    for (const cell of column) {
      const node = el("i", "", "usage-heat-cell"); node.dataset.level = String(cell.level);
      const [y = 0, m = 1, d = 1] = cell.date.split("-").map(Number);
      const lines = [`${dateFormat.format(new Date(y, m - 1, d))}: ${compact(cell.total)}`];
      if (cell.total > 0) {
        sum += cell.total; active++;
        const parts = providers.filter(([, map]) => map.has(cell.date));
        if (parts.length > 1) for (const [name, map] of parts) lines.push(`${name}: ${compact(map.get(cell.date) ?? 0)}`);
      }
      node.title = lines.join("\n"); cells.append(node);
    }
  });
  months.append(labels); grid.append(cells);
  const legend = el("div", "", "usage-heat-legend");
  legend.append(el("span", t("usage.less")));
  for (let level = 0; level < 5; level++) { const node = el("i", "", "usage-heat-cell"); node.dataset.level = String(level); legend.append(node); }
  legend.append(el("span", t("usage.more")));
  const body = el("div", "", "usage-heat-body"); body.append(months, days, grid);
  section.append(el("h3", t("usage.activity"), "usage-activity-title"), body, legend);
  section.setAttribute("role", "img"); section.setAttribute("aria-label", `${t("usage.activity")}: ${compact(sum)} (${active})`);
  return section;
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
  private heatWeeksShown = 0;
  private resizeObserver: ResizeObserver | null = null;
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
    if (typeof ResizeObserver !== "undefined") {
      this.resizeObserver = new ResizeObserver(() => { if (this.data && this.data.hourly.length > 0 && heatWeeks(this.contentWidth()) !== this.heatWeeksShown && this.visible()) this.render(); });
      this.resizeObserver.observe(this.content);
    }
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
    this.resizeObserver?.disconnect(); this.cleanupLang(); this.element.remove();
  }
  private translate(): void {
    this.setTitle(this.title);
    this.periodSelect.setAttribute("aria-label", t("usage.title")); this.groupSelect.setAttribute("aria-label", t("usage.project"));
    this.refreshButton.title = t("usage.refresh"); this.refreshButton.setAttribute("aria-label", t("usage.refresh"));
    this.displaySelect.setAttribute("aria-label", t("usage.quota_display"));
    for (const option of [...this.periodSelect.options, ...this.groupSelect.options, ...this.displaySelect.options]) option.textContent = t(`usage.${option.value}`);
    this.render();
  }
  private contentWidth(): number { return this.content.clientWidth - 16; }
  private renderState(): void {
    switch (this.phase) {
      case "loading": this.state.textContent = t("usage.loading"); break;
      case "error": this.state.textContent = t("usage.error"); break;
      case "ready": this.state.textContent = ""; break;
    }
    this.state.hidden = this.state.textContent === ""; this.state.classList.toggle("usage-state--error", this.phase === "error");
  }
  private providerCard(provider: ProviderUsage, now: number): HTMLElement {
    const quotaLabel = t(this.display === "used" ? "usage.used_quota" : "usage.remaining_quota");
    const displayed = (remaining: number | null): number | null => remaining === null ? null : this.display === "used" ? 100 - remaining : remaining;
      const card = el("article", "", "usage-card");
      const heading = el("div", "", "usage-provider-heading");
      const name = el("span", provider.id === "claude" ? "Claude Code" : "Codex"); name.title = t(`usage.${provider.auth_status}`);
      const selected = provider.quotas.find(item => item.window_minutes === (this.period === "seven_day" ? 10080 : 300));
      const remaining = displayed(remainingQuota(selected, now));
      const failed = this.refreshFailed || provider.warnings.some(warning => warning.startsWith("quota_"));
      const stale = failed || (selected !== undefined && (remaining === null || now - selected.observed_at > 120));
      const total = el("strong", remaining === null ? "—" : `${remaining.toFixed(0)}%`); total.title = `${quotaLabel}${stale ? ` · ${t("usage.stale")}` : ""}`;
      if (stale) total.classList.add("usage-stale");
      heading.append(name, total); card.append(heading);
      const observed = provider.quotas.reduce((latest, quota) => Math.max(latest, quota.observed_at), 0);
      total.title += `\n${t("usage.observed")}: ${observed ? new Date(observed * 1000).toLocaleTimeString() : "—"}`;
      if (provider.auth_status !== "credentials_present") card.append(el("span", t(`usage.${provider.auth_status}`), "usage-auth"));
      const meters = el("div", "", "usage-meters");
      for (const [minutes, key] of [[300, "cycle_five_hour"], [10080, "cycle_week"]] as const) {
        const quota = provider.quotas.find(item => item.window_minutes === minutes);
        const row = el("div", "", "usage-meter-row"); const label = el("span", t(`usage.${key}`));
        const available = displayed(remainingQuota(quota, now));
        const percent = el("span", available === null ? "—" : `${available.toFixed(0)}%`);
        const fill = el("div", "", "usage-meter-fill"); const meter = el("div", "", "usage-meter"); meter.append(fill);
        meter.setAttribute("role", "progressbar"); meter.setAttribute("aria-valuemin", "0"); meter.setAttribute("aria-valuemax", "100");
        if (available === null) meter.style.visibility = "hidden";
        const left = available === null ? null : this.display === "used" ? 100 - available : available;
        fill.style.width = `${available ?? 0}%`; meter.setAttribute("aria-valuenow", String(available ?? 0));
        if (left !== null) fill.dataset.level = left <= 10 ? "critical" : left <= 25 ? "warn" : "ok";
        if (quota) {
          const stale = failed || available === null || now - quota.observed_at > 120;
          if (stale) { if (available !== null) percent.textContent += " *"; row.classList.add("usage-stale"); }
          row.title = `${quotaLabel}: ${available === null ? "—" : `${available.toFixed(1)}%`}\n${t("usage.observed")}: ${new Date(quota.observed_at * 1000).toLocaleString()}${quota.resets_at === null ? "" : `\n${t("usage.resets")}: ${new Date(quota.resets_at * 1000).toLocaleString()}`}${stale ? `\n${t("usage.stale")}` : ""}`;
        } else { row.title = `${quotaLabel}: —`; }
        meter.setAttribute("aria-label", `${quotaLabel} ${t(`usage.${key}`)} ${percent.textContent}`);
        row.append(label, meter, percent, resetInfo(quota?.resets_at, now, t(`usage.${key}`))); meters.append(row);
      }
      card.append(meters);
      const local = el("span", `${t("usage.local_tokens")}: ${compact(provider[this.period].total)}`, "usage-local-tokens");
      local.title = breakdown(provider[this.period]); card.append(local);
    return card;
  }
  private accountHeading(account: string, also: readonly string[]): HTMLElement {
    const profiles = getAccounts();
    const label = (id: string): string => id === "" ? t("usage.default_account") : id === ACCOUNT_NONE ? t("usage.home_account") : profiles.find(p => p.id === id)?.name || id;
    const heading = el("h3", "", "usage-account-heading");
    const dot = el("span", "", "account-badge__dot");
    const color = normalizeColor(profiles.find(p => p.id === account)?.color ?? "");
    if (color) dot.style.background = color;
    heading.append(dot, el("span", label(account)));
    if (also.length > 0) {
      const more = el("span", `+${also.length}`, "usage-account-also");
      more.title = `${t("usage.also_used_by")}: ${also.map(label).join(", ")}`;
      heading.append(more);
    }
    return heading;
  }
  private render(): void {
    this.renderState(); this.content.replaceChildren(); if (!this.data) return;
    const cards = el("div", "", "usage-cards");
    const now = Date.now() / 1000;
    const groups = groupProvidersByAccount(this.data.providers);
    for (const group of groups) {
      // One account renders exactly as before; headings appear once there is more than one.
      if (groups.length > 1) cards.append(this.accountHeading(group.account, group.also));
      for (const provider of group.providers) cards.append(this.providerCard(provider, now));
    }
    this.content.append(cards);
    let heat: HTMLElement | null = null;
    if (this.data.hourly.length > 0) { this.heatWeeksShown = heatWeeks(this.contentWidth()); heat = heatmap(this.data.hourly, this.heatWeeksShown); this.content.append(heat); }
    const rows = aggregateUsage(this.data.rows, this.group, this.period).filter(row => row.tokens.total > 0);
    if (rows.length > 0) {
      const sum = sumTotals(rows);
      const line = el("div", "", "usage-sum"); line.title = breakdown(sum);
      const value = el("strong", `${sum.total.toLocaleString(getLang())} ${t("usage.tokens_unit")}`);
      line.append(el("span", `${t("usage.sum")} · ${t(`usage.${this.period}`)}`), value);
      if (sum.total >= 10_000) line.append(el("span", `(${compact(sum.total)})`, "usage-sum-compact"));
      this.content.append(line);
    }
    const table = el("table"); const head = el("thead"); const heading = el("tr"); heading.append(el("th", t(`usage.${this.group}`)), el("th", t("usage.total"))); head.append(heading); table.append(head);
    const body = el("tbody");
    for (const row of rows) {
      const tr = el("tr"); const label = this.group === "provider" ? (row.label === "claude" ? "Claude Code" : "Codex") : row.label;
      const cell = el("td", label === "unknown" ? t("usage.unknown") : label); cell.title = label;
      const total = el("td", compact(row.tokens.total)); total.title = breakdown(row.tokens); tr.append(cell, total); body.append(tr);
    }
    table.append(body); this.content.append(rows.length ? table : el("p", t("usage.empty"), "usage-empty"));
    const details = el("details", "", "usage-details"); details.append(el("summary", this.data.warnings.length || this.data.providers.some(p => p.warnings.length) ? "⚠ ⓘ" : "ⓘ")); details.querySelector("summary")?.setAttribute("aria-label", t("usage.limitations"));
    details.append(el("p", t("usage.limitations")), el("p", t("usage.history")));
    const notices = new Set(this.data.warnings);
    for (const provider of this.data.providers) for (const warning of provider.warnings) notices.add(`${provider.id}_${warning}`);
    for (const warning of notices) details.append(el("p", t(`usage.${warning}`)));
    this.content.append(details);
    // Measured again now that everything is in: a scrollbar that appeared
    // with the rest of the content (macOS with "always show scrollbars")
    // narrows the pane, and the newest weeks on the right would be clipped.
    // Only ever shrink, so the next resize check settles on this count.
    if (heat) {
      const fit = heatWeeks(this.contentWidth());
      if (fit < this.heatWeeksShown) { this.heatWeeksShown = fit; const next = heatmap(this.data.hourly, fit); heat.replaceWith(next); }
    }
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
