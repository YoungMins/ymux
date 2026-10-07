import { api } from "../ipc/bridge";
import { t, getLang, onLangChange } from "../i18n/i18n";
import type { Pane } from "../layout/Pane";
import type { Uuid } from "../types";
import { aggregateUsage, type TokenUsage, type UsageWindow, type TokenTotals } from "./model";
import "./tokenmonitor.css";

function el<K extends keyof HTMLElementTagNameMap>(tag: K, text = "", cls = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag); node.textContent = text; node.className = cls; return node;
}
function compact(value: number): string { return new Intl.NumberFormat(getLang(), { notation: "compact", maximumFractionDigits: 1 }).format(value); }
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
  private readonly refreshButton = el("button", "↻");
  private readonly cleanupLang: () => void;
  private timer: ReturnType<typeof setInterval> | null = null;
  private disposed = false;
  private busy = false;
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
    this.periodSelect.onchange = () => {
      switch (this.periodSelect.value) { case "five_hour": case "seven_day": case "all_time": this.period = this.periodSelect.value; break; }
      this.render();
    };
    this.groupSelect.onchange = () => {
      switch (this.groupSelect.value) { case "provider": case "model": case "project": this.group = this.groupSelect.value; break; }
      this.render();
    };
    this.refreshButton.type = "button"; this.refreshButton.onclick = () => { void this.refresh(); };
    this.controls.append(this.periodSelect, this.groupSelect, this.refreshButton);
    this.element.append(this.titleEl, this.controls, this.state, this.content);
    this.setOwnChrome(options.ownChrome ?? true);
    this.cleanupLang = onLangChange(() => this.translate()); this.translate();
  }
  setOwnChrome(enabled: boolean): void { this.titleEl.hidden = !enabled; }
  setTitle(title: string | null): void { this.title = title; this.titleEl.textContent = title || t("usage.title"); }
  focus(): void { this.element.focus(); this.options.onFocus?.(); }
  scheduleFit(): void { if (this.timer !== null && !this.busy && this.visible()) void this.refreshIfOld(); }
  private visible(): boolean { return !document.hidden && this.element.isConnected && !this.element.closest(".pane--tab-hidden") && this.element.getClientRects().length > 0; }
  private async refreshIfOld(): Promise<void> { if (!this.data || Date.now() / 1000 - this.data.collected_at >= 60) await this.refresh(); }
  async spawn(): Promise<void> {
    if (this.disposed || this.timer !== null) return;
    this.timer = setInterval(() => { if (this.visible()) void this.refresh(); }, 60_000);
    if (this.visible()) await this.refresh();
  }
  dispose(): void { this.disposed = true; if (this.timer !== null) clearInterval(this.timer); this.cleanupLang(); this.element.remove(); }
  private translate(): void {
    this.setTitle(this.title);
    this.periodSelect.setAttribute("aria-label", t("usage.title")); this.groupSelect.setAttribute("aria-label", t("usage.project"));
    this.refreshButton.title = t("usage.refresh"); this.refreshButton.setAttribute("aria-label", t("usage.refresh"));
    for (const option of [...this.periodSelect.options, ...this.groupSelect.options]) option.textContent = t(`usage.${option.value}`);
    this.render();
  }
  private renderState(): void {
    switch (this.phase) {
      case "loading": this.state.textContent = t("usage.loading"); break;
      case "error": this.state.textContent = t("usage.error"); break;
      case "ready": this.state.textContent = this.data ? `${t("usage.observed")}: ${new Date(this.data.collected_at * 1000).toLocaleTimeString()}` : ""; break;
    }
  }
  private render(): void {
    this.renderState(); this.content.replaceChildren(); if (!this.data) return;
    const cards = el("div", "", "usage-cards");
    for (const provider of this.data.providers) {
      const card = el("article", "", "usage-card");
      const heading = el("div", "", "usage-provider-heading");
      const name = el("span", provider.id === "claude" ? "Claude Code" : "Codex"); name.title = t(`usage.${provider.auth_status}`);
      const total = el("strong", compact(provider[this.period].total)); total.title = breakdown(provider[this.period]); heading.append(name, total); card.append(heading);
      const auth = el("span", t(`usage.${provider.auth_status}`), "usage-auth"); card.append(auth);
      const meters = el("div", "", "usage-meters");
      for (const [minutes, key] of [[300, "five_hour"], [10080, "seven_day"]] as const) {
        const quota = provider.quotas.find(item => item.window_minutes === minutes);
        const row = el("div", "", "usage-meter-row"); const label = el("span", t(`usage.${key}`));
        const percent = el("span", quota ? `${quota.used_percent.toFixed(0)}%` : "—");
        const meter = el("progress"); meter.max = 100;
        if (quota) {
          meter.value = Math.min(100, Math.max(0, quota.used_percent));
          const stale = (quota.resets_at !== null && quota.resets_at <= this.data.collected_at) || this.data.collected_at - quota.observed_at > 600;
          if (stale) { percent.textContent += " *"; row.classList.add("usage-stale"); }
          row.title = `${t("usage.quota")}: ${quota.used_percent.toFixed(1)}%\n${t("usage.observed")}: ${new Date(quota.observed_at * 1000).toLocaleString()}${quota.resets_at === null ? "" : `\n${t("usage.resets")}: ${new Date(quota.resets_at * 1000).toLocaleString()}`}${stale ? `\n${t("usage.stale")}` : ""}`;
        } else { meter.value = 0; row.title = `${t("usage.quota")}: —`; }
        meter.setAttribute("aria-label", `${t("usage.quota")} ${t(`usage.${key}`)} ${percent.textContent}`);
        row.append(label, meter, percent); meters.append(row);
      }
      card.append(meters); cards.append(card);
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
  private async refresh(): Promise<void> {
    if (this.busy || this.disposed) return; this.busy = true; this.refreshButton.disabled = true;
    this.phase = "loading"; this.renderState();
    try { const next = await api.getTokenUsage(); if (!this.disposed) { this.data = next; this.phase = "ready"; this.render(); } }
    catch (error: unknown) { if (!this.disposed) { this.phase = "error"; this.renderState(); } console.warn("Token usage read failed", error); }
    finally { this.busy = false; this.refreshButton.disabled = false; }
  }
}
