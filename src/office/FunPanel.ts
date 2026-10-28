// The office games' window: achievements (and the title worn), the shop,
// today's quests, the weekly newspaper and the on/off switches, one tab
// each. A pixel window like the hero roster; everything it shows comes from
// `funStore`.

import { getLang, t } from "../i18n/i18n";
import { fill } from "../git/worktreeFlow";
import {
  ACHIEVEMENTS,
  ACTIVITY_FAMILIES,
  FAMILIES,
  priceOf,
  weeklySpecial,
  QUEST_COINS,
  SHOP,
  achievementById,
  progressOf,
  questDef,
  questsFor,
  dayKey,
  roman,
  weekReport,
  type Achievement,
  type FunStats,
  type ItemPlace,
} from "./funModel";
import { kindName } from "./officeModel";
import { shortDuration } from "./officeQueue";
import { funProp, paintProps } from "./sprites";
import { petSprite } from "./sheet";
import type { FunSettings } from "./funStore";
import type { PetSpecies } from "./hiPets";

export type FunTab = "achievements" | "shop" | "quests" | "report" | "settings";
const TABS: readonly { id: FunTab; icon: string }[] = [
  { id: "achievements", icon: "🏆" },
  { id: "shop", icon: "🛒" },
  { id: "quests", icon: "📜" },
  { id: "report", icon: "📰" },
  { id: "settings", icon: "⚙" },
];

/// An achievement's name: its family's, with the rank as a numeral.
export function achievementName(a: Achievement): string {
  const name = t(`fun.ach.${a.family.id}`);
  return a.family.steps.length > 1 ? `${name} ${roman(a.rank + 1)}` : name;
}

function num(n: number): string {
  return new Intl.NumberFormat(getLang()).format(n);
}

export interface FunActions {
  buy(id: string): void;
  wear(id: string): void;
  setSettings(s: FunSettings): void;
  readReport(): void;
  /// Start moving the bought furniture around, or put it all back.
  decorate(): void;
  resetPlaces(): void;
  /// The record: copy it out, paste one in, or start over.
  exportRecord(): void;
  importRecord(): void;
  resetRecord(): void;
}

type AchFilter = "all" | "got" | "near" | "locked";

export class FunPanel {
  readonly element: HTMLElement;
  private readonly tabsEl: HTMLElement;
  private readonly body: HTMLElement;
  private tab: FunTab = "achievements";
  private stats: FunStats | null = null;
  private settings: FunSettings | null = null;
  private filter: AchFilter = "all";
  private query = "";

  constructor(private readonly actions: FunActions) {
    this.element = document.createElement("div");
    this.element.className = "office-heroes office-fun";
    this.element.hidden = true;
    this.element.setAttribute("role", "dialog");
    this.element.addEventListener("click", (ev) => ev.stopPropagation());
    const frame = document.createElement("div");
    frame.className = "office-peek__frame";
    const head = document.createElement("div");
    head.className = "office-peek__head";
    this.tabsEl = document.createElement("div");
    this.tabsEl.className = "office-fun__tabs";
    this.tabsEl.setAttribute("role", "tablist");
    const close = document.createElement("button");
    close.type = "button";
    close.className = "office-peek__btn office-peek__close";
    close.textContent = "✕";
    close.title = t("office.peek.close");
    close.setAttribute("aria-label", t("office.peek.close"));
    close.addEventListener("click", () => this.hide());
    head.append(this.tabsEl, close);
    this.body = document.createElement("div");
    this.body.className = "office-heroes__list office-fun__body";
    frame.append(head, this.body);
    this.element.appendChild(frame);
  }

  get open(): boolean {
    return !this.element.hidden;
  }

  get current(): FunTab {
    return this.tab;
  }

  show(tab: FunTab): void {
    this.tab = tab;
    this.element.hidden = false;
    this.paint();
    if (tab === "report") this.actions.readReport();
  }

  hide(): void {
    this.element.hidden = true;
  }

  render(stats: FunStats, settings: FunSettings): void {
    this.stats = stats;
    this.settings = settings;
    if (this.open) this.paint();
  }

  private paint(): void {
    const s = this.stats;
    if (!s || !this.settings) return;
    // A repaint (a keystroke, or a turn finishing meanwhile) keeps typing in
    // the search box going.
    const typing = document.activeElement instanceof HTMLInputElement && document.activeElement.classList.contains("office-fun__search") ? document.activeElement.selectionStart : null;
    this.paintTabs(s, this.settings);
    if (typing !== null) {
      const again = this.body.querySelector<HTMLInputElement>(".office-fun__search");
      again?.focus();
      again?.setSelectionRange(typing, typing);
    }
  }

  private paintTabs(s: FunStats, settings: FunSettings): void {
    this.tabsEl.replaceChildren(
      ...TABS.map((x) => {
        const b = document.createElement("button");
        b.type = "button";
        b.setAttribute("role", "tab");
        b.className = `office-fun__tab${x.id === this.tab ? " office-fun__tab--on" : ""}`;
        b.setAttribute("aria-selected", String(x.id === this.tab));
        b.textContent = `${x.icon} ${t(`fun.${x.id}`)}`;
        b.addEventListener("click", () => this.show(x.id));
        return b;
      }),
    );
    const scroll = this.body.scrollTop;
    switch (this.tab) {
      case "achievements":
        this.body.replaceChildren(...this.achievements(s));
        break;
      case "shop":
        this.body.replaceChildren(...this.shop(s));
        break;
      case "quests":
        this.body.replaceChildren(...this.quests(s));
        break;
      case "report":
        this.body.replaceChildren(this.report(s));
        break;
      case "settings":
        this.body.replaceChildren(this.settingsView(settings));
        break;
    }
    paintProps(this.body);
    this.body.scrollTop = scroll;
  }

  private note(text: string): HTMLElement {
    const n = document.createElement("div");
    n.className = "office-heroes__note";
    n.textContent = text;
    return n;
  }

  private achievements(s: FunStats): HTMLElement[] {
    const now = Date.now();
    const count = Object.keys(s.unlocked).length;
    const head = this.note(fill(t("fun.achievementsCount"), { n: count, total: ACHIEVEMENTS.length }));
    if (s.title) {
      const a = achievementById(s.title);
      if (a) head.textContent += ` · ${t("fun.wearing")}: ${a.family.icon} ${achievementName(a)}`;
    }
    // Filters and a search box.
    const tools = document.createElement("div");
    tools.className = "office-fun__tools";
    for (const f of ["all", "got", "near", "locked"] as const) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = `office-fun__tab${this.filter === f ? " office-fun__tab--on" : ""}`;
      b.textContent = t(`fun.filter.${f}`);
      b.addEventListener("click", () => {
        this.filter = f;
        this.paint();
      });
      tools.appendChild(b);
    }
    const search = document.createElement("input");
    search.type = "search";
    search.className = "office-fun__search";
    search.placeholder = t("fun.search");
    search.value = this.query;
    search.addEventListener("keydown", (ev) => ev.stopPropagation());
    search.addEventListener("input", () => {
      this.query = search.value;
      this.paint();
    });
    tools.appendChild(search);
    const grid = document.createElement("div");
    grid.className = "office-fun__grid";
    const q = this.query.trim().toLowerCase();
    const shown = ACHIEVEMENTS.filter((a) => {
      const got = s.unlocked[a.id] !== undefined;
      if (this.filter === "got" && !got) return false;
      if (this.filter === "locked" && got) return false;
      if (this.filter === "near" && (got || progressOf(s, a, now).share < 0.5)) return false;
      if (!q) return true;
      return `${achievementName(a)} ${t(`fun.achd.${a.family.id}`)}`.toLowerCase().includes(q);
    });
    // Unlocked first, then the closest to unlocking.
    const list = [...shown].sort((a, b) => {
      const ua = s.unlocked[a.id] !== undefined;
      const ub = s.unlocked[b.id] !== undefined;
      if (ua !== ub) return ua ? -1 : 1;
      if (ua) return (s.unlocked[b.id] ?? 0) - (s.unlocked[a.id] ?? 0);
      return progressOf(s, b, now).share - progressOf(s, a, now).share;
    });
    for (const a of list) {
      const got = s.unlocked[a.id] !== undefined;
      const card = document.createElement("div");
      card.className = `office-fun__ach${got ? " office-fun__ach--got" : ""}${s.title === a.id ? " office-fun__ach--worn" : ""}`;
      const icon = document.createElement("span");
      icon.className = "office-fun__ach-icon";
      icon.textContent = a.family.icon;
      const text = document.createElement("div");
      text.className = "office-fun__ach-text";
      const name = document.createElement("b");
      name.textContent = achievementName(a);
      const desc = document.createElement("span");
      desc.textContent = fill(t(`fun.achd.${a.family.id}`), { n: num(a.target) });
      text.append(name, desc);
      if (!got) {
        const p = progressOf(s, a, now);
        const bar = document.createElement("span");
        bar.className = "office-fun__bar";
        const f = document.createElement("i");
        f.style.width = `${Math.round(p.share * 100)}%`;
        bar.appendChild(f);
        bar.title = `${num(Math.min(p.value, a.target))} / ${num(a.target)}`;
        text.appendChild(bar);
        // Counted from the activity log: say so while it is off.
        if (!s.activity && ACTIVITY_FAMILIES.has(a.family.id)) {
          const note = document.createElement("span");
          note.className = "office-fun__needs";
          note.textContent = `📋 ${t("fun.needsLog")}`;
          text.appendChild(note);
        }
      }
      const side = document.createElement("div");
      side.className = "office-fun__ach-side";
      side.textContent = got ? fill(t("fun.reward"), { n: a.reward }) : t("fun.locked");
      if (got && s.title !== a.id) {
        const wear = document.createElement("button");
        wear.type = "button";
        wear.className = "office-fun__btn";
        wear.textContent = t("fun.wear");
        wear.addEventListener("click", () => this.actions.wear(a.id));
        side.appendChild(wear);
      }
      card.append(icon, text, side);
      grid.appendChild(card);
    }
    return [head, tools, grid];
  }

  private shop(s: FunStats): HTMLElement[] {
    const head = this.note(`🪙 ${fill(t("fun.coins"), { n: num(s.coins) })}`);
    if (s.owned.some((id) => SHOP.find((i) => i.id === id)?.place !== "pet")) {
      const deco = document.createElement("button");
      deco.type = "button";
      deco.className = "office-fun__btn office-fun__deco";
      deco.textContent = `🖌 ${t("fun.decorate")}`;
      deco.addEventListener("click", () => this.actions.decorate());
      const reset = document.createElement("button");
      reset.type = "button";
      reset.className = "office-fun__btn office-fun__deco";
      reset.textContent = t("fun.resetPlaces");
      reset.disabled = Object.keys(s.places).length === 0;
      reset.addEventListener("click", () => this.actions.resetPlaces());
      head.append(deco, reset);
    }
    const out: HTMLElement[] = [head];
    const now = Date.now();
    const special = weeklySpecial(now);
    for (const place of ["use", "lobby", "room", "wall", "pet"] as ItemPlace[]) {
      const h = document.createElement("div");
      h.className = "office-fun__section";
      h.textContent = t(`fun.place.${place}`);
      const grid = document.createElement("div");
      grid.className = "office-fun__shop";
      for (const item of SHOP.filter((i) => i.place === place)) {
        const owned = s.owned.includes(item.id);
        const card = document.createElement("div");
        card.className = `office-fun__item${owned ? " office-fun__item--owned" : ""}`;
        const art = document.createElement("div");
        art.className = "office-fun__item-art";
        if (place === "pet") art.appendChild(petSprite(item.id as PetSpecies));
        else if (place === "use") art.textContent = item.id === "snacks" ? "🍩" : item.id === "party" ? "🎉" : "🎆";
        else art.innerHTML = funProp(item.id, "");
        const price = priceOf(item, now);
        if (item.id === special && !owned) {
          const tag = document.createElement("span");
          tag.className = "office-fun__special";
          tag.textContent = t("fun.special");
          card.appendChild(tag);
        }
        const name = document.createElement("span");
        name.className = "office-fun__item-name";
        name.textContent = t(`fun.item.${item.id}`);
        const b = document.createElement("button");
        b.type = "button";
        b.className = "office-fun__btn";
        b.disabled = owned || s.coins < price;
        b.textContent = owned ? t("fun.owned") : `${t("fun.buy")} · 🪙${num(price)}`;
        b.addEventListener("click", () => this.actions.buy(item.id));
        card.append(art, name, b);
        grid.appendChild(card);
      }
      out.push(h, grid);
    }
    return out;
  }

  private quests(s: FunStats): HTMLElement[] {
    const today = dayKey(Date.now());
    const ids = s.quests.day === today ? s.quests.ids : questsFor(today);
    const done = s.quests.day === today ? s.quests.done : [];
    const day = s.days[today];
    const list = ids.map((id) => {
      const q = questDef(id);
      const card = document.createElement("div");
      card.className = `office-fun__ach${done.includes(id) ? " office-fun__ach--got" : ""}`;
      const icon = document.createElement("span");
      icon.className = "office-fun__ach-icon";
      icon.textContent = done.includes(id) ? "✅" : "📜";
      const text = document.createElement("div");
      text.className = "office-fun__ach-text";
      const name = document.createElement("b");
      name.textContent = q ? fill(t(`fun.quest.${id}`), { n: q.target }) : id;
      text.appendChild(name);
      if (q) {
        const have = Math.min(q.target, day?.c[q.key] ?? 0);
        const bar = document.createElement("span");
        bar.className = "office-fun__bar";
        const f = document.createElement("i");
        f.style.width = `${Math.round((have / q.target) * 100)}%`;
        bar.appendChild(f);
        const n = document.createElement("span");
        n.textContent = `${have} / ${q.target}`;
        text.append(bar, n);
      }
      const side = document.createElement("div");
      side.className = "office-fun__ach-side";
      side.textContent = fill(t("fun.reward"), { n: QUEST_COINS });
      card.append(icon, text, side);
      return card;
    });
    return [this.note(fill(t("fun.questsNote"), { n: QUEST_COINS })), ...list];
  }

  private report(s: FunStats): HTMLElement {
    const r = weekReport(s, Date.now());
    const paper = document.createElement("div");
    paper.className = "office-fun__paper";
    const title = document.createElement("h3");
    title.textContent = t("fun.report.paper");
    const date = (k: string) => {
      const [y, m, d] = k.split("-").map(Number);
      return new Intl.DateTimeFormat(getLang(), { month: "short", day: "numeric", weekday: "short" }).format(new Date(y, m - 1, d));
    };
    const sub = document.createElement("div");
    sub.className = "office-fun__paper-sub";
    sub.textContent = fill(t("fun.report.week"), { day: date(r.week) });
    paper.append(title, sub);
    if (r.turns === 0) {
      const p = document.createElement("p");
      p.textContent = t("fun.report.quiet");
      paper.appendChild(p);
      return paper;
    }
    const lead = document.createElement("p");
    lead.className = "office-fun__paper-lead";
    lead.textContent = r.mvp ? fill(t("fun.report.mvp"), { agent: kindName(r.mvp.kind), n: r.mvp.turns }) : "";
    const facts = document.createElement("dl");
    for (const [k, v] of [
      ["turns", num(r.turns)],
      ["lines", `+${num(r.add)} / −${num(r.del)}`],
      ["reviews", num(r.reviews)],
      ["quick", num(r.quick)],
      ["time", shortDuration(r.workMs, getLang())],
      ["coins", `🪙 ${num(r.coins)}`],
    ] as const) {
      const dt = document.createElement("dt");
      dt.textContent = t(`fun.report.${k}`);
      const dd = document.createElement("dd");
      dd.textContent = v;
      facts.append(dt, dd);
    }
    paper.append(lead, facts);
    if (r.busiest) {
      const p = document.createElement("p");
      p.textContent = fill(t("fun.report.busiest"), { day: date(r.busiest.day), n: r.busiest.turns });
      paper.appendChild(p);
    }
    if (r.unlocked.length > 0) {
      const h = document.createElement("h4");
      h.textContent = t("fun.report.unlocked");
      const ul = document.createElement("ul");
      for (const id of r.unlocked) {
        const a = achievementById(id);
        if (!a) continue;
        const li = document.createElement("li");
        li.textContent = `${a.family.icon} ${achievementName(a)}`;
        ul.appendChild(li);
      }
      paper.append(h, ul);
    }
    return paper;
  }

  private settingsView(settings: FunSettings): HTMLElement {
    const box = document.createElement("div");
    box.className = "office-fun__settings";
    for (const key of ["life", "ambience", "banners", "sound"] as const) {
      const label = document.createElement("label");
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = settings[key];
      cb.addEventListener("change", () => this.actions.setSettings({ ...settings, [key]: cb.checked }));
      label.append(cb, ` ${t(`fun.settings.${key}`)}`);
      box.appendChild(label);
    }
    const row = document.createElement("div");
    row.className = "office-fun__record";
    for (const [k, act] of [
      ["export", () => this.actions.exportRecord()],
      ["import", () => this.actions.importRecord()],
      ["reset", () => this.actions.resetRecord()],
    ] as const) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "office-fun__btn";
      b.textContent = t(`fun.${k}`);
      b.addEventListener("click", act);
      row.appendChild(b);
    }
    box.appendChild(row);
    return box;
  }
}

/// How many families there are (for the docs and the panel's header).
export const FAMILY_COUNT = FAMILIES.length;
