// The office's hero roster ("adventurers' guild"): every account as a hero
// with its level, experience bar and class. Picking a class here dresses
// that account's agents on the floor. A pixel window in the theme's peek
// style; the numbers come from the Usage view's collector.

import { getLang, t } from "../i18n/i18n";
import { fill } from "../git/worktreeFlow";
import { HERO_CLASSES, TOKENS_PER_LEVEL, levelOf, nextTierLevel, tierOf, type HeroClass } from "./heroModel";
import { characterSvg } from "./sprites";

export interface HeroRow {
  account: string;
  name: string;
  /// "#rrggbb" or "".
  color: string;
  tokens: number;
  cls: HeroClass;
}

function compact(value: number): string {
  return new Intl.NumberFormat(getLang(), { notation: "compact", maximumFractionDigits: 1 }).format(value);
}

/// A still slime in `cls`'s gear for `tier`.
export function heroAvatar(cls: HeroClass, tier: number): string {
  return characterSvg("claude", "lead", { cls, tier });
}

export class HeroPanel {
  readonly element: HTMLElement;
  private readonly list: HTMLElement;

  constructor(private readonly onPick: (account: string, cls: HeroClass) => void) {
    this.element = document.createElement("div");
    this.element.className = "office-heroes";
    this.element.hidden = true;
    this.element.setAttribute("role", "dialog");
    this.element.setAttribute("aria-label", t("office.heroes.title"));
    this.element.addEventListener("click", (ev) => ev.stopPropagation());
    const frame = document.createElement("div");
    frame.className = "office-peek__frame";
    const head = document.createElement("div");
    head.className = "office-peek__head";
    const title = document.createElement("span");
    title.className = "office-peek__title";
    title.textContent = `⚔ ${t("office.heroes.title")}`;
    const close = document.createElement("button");
    close.type = "button";
    close.className = "office-peek__btn office-peek__close";
    close.textContent = "✕";
    close.title = t("office.peek.close");
    close.setAttribute("aria-label", t("office.peek.close"));
    close.addEventListener("click", () => this.hide());
    head.append(title, close);
    this.list = document.createElement("div");
    this.list.className = "office-heroes__list";
    const note = document.createElement("div");
    note.className = "office-heroes__note";
    note.textContent = fill(t("office.heroes.note"), { n: compact(TOKENS_PER_LEVEL) });
    frame.append(head, this.list, note);
    this.element.appendChild(frame);
  }

  get open(): boolean {
    return !this.element.hidden;
  }

  show(): void {
    this.element.hidden = false;
  }

  hide(): void {
    this.element.hidden = true;
  }

  /// `rows` null: the numbers are still loading.
  render(rows: readonly HeroRow[] | null): void {
    if (rows === null) {
      this.list.replaceChildren(this.message(t("office.heroes.loading")));
      return;
    }
    if (rows.length === 0) {
      this.list.replaceChildren(this.message(t("office.heroes.empty")));
      return;
    }
    const sorted = [...rows].sort((a, b) => b.tokens - a.tokens);
    this.list.replaceChildren(...sorted.map((r) => this.row(r)));
  }

  private message(text: string): HTMLElement {
    const p = document.createElement("div");
    p.className = "office-heroes__empty";
    p.textContent = text;
    return p;
  }

  private row(r: HeroRow): HTMLElement {
    const lv = levelOf(r.tokens);
    const tier = tierOf(lv.level);
    const el = document.createElement("div");
    el.className = `office-hero${tier >= 5 ? " office-hero--aura" : ""}`;

    const avatar = document.createElement("div");
    avatar.className = "office-hero__avatar";
    avatar.innerHTML = heroAvatar(r.cls, tier);

    const info = document.createElement("div");
    info.className = "office-hero__info";
    const name = document.createElement("div");
    name.className = "office-hero__name";
    if (r.color) {
      const dot = document.createElement("span");
      dot.className = "account-badge__dot";
      dot.style.background = r.color;
      name.appendChild(dot);
    }
    name.append(r.name);
    const level = document.createElement("div");
    level.className = "office-hero__level";
    level.textContent = `${fill(t("office.heroes.level"), { n: String(lv.level) })} · ${t(`office.class.${r.cls}`)}`;
    const bar = document.createElement("div");
    bar.className = "office-hero__bar";
    bar.setAttribute("role", "progressbar");
    bar.setAttribute("aria-valuemin", "0");
    bar.setAttribute("aria-valuemax", "100");
    bar.setAttribute("aria-valuenow", String(Math.floor(lv.progress * 100)));
    const fillEl = document.createElement("span");
    fillEl.style.width = `${(lv.progress * 100).toFixed(1)}%`;
    bar.appendChild(fillEl);
    const xp = document.createElement("div");
    xp.className = "office-hero__xp";
    const next = nextTierLevel(lv.level);
    xp.textContent = [
      `${compact(lv.xp)} / ${compact(TOKENS_PER_LEVEL)} XP`,
      fill(t("office.heroes.total"), { n: compact(r.tokens) }),
      next === null ? t("office.heroes.max") : fill(t("office.heroes.next"), { n: String(next) }),
    ].join(" · ");
    info.append(name, level, bar, xp);

    // The class picker: each class shown in the gear this hero would wear.
    const pick = document.createElement("div");
    pick.className = "office-hero__classes";
    pick.setAttribute("role", "radiogroup");
    pick.setAttribute("aria-label", t("office.heroes.class"));
    for (const cls of HERO_CLASSES) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "office-hero__class";
      b.setAttribute("role", "radio");
      b.setAttribute("aria-checked", String(cls === r.cls));
      b.title = t(`office.class.${cls}`);
      b.innerHTML = heroAvatar(cls, Math.max(tier, 2));
      b.addEventListener("click", () => this.onPick(r.account, cls));
      pick.appendChild(b);
    }

    el.append(avatar, info, pick);
    return el;
  }
}
