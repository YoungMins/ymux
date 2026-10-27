// The office view's peek: a window over a room with its terminal's screen,
// key buttons and a prompt box, so the user can read and answer an agent
// without leaving the office. The screen is polled while open (it is plain
// text read off the terminal's buffer; cheap). One peek at a time; a tab
// group's terminals get a tab each.

import type { Uuid } from "../types";
import type { PaneStatus } from "../terminal/paneStatus";
import { t } from "../i18n/i18n";
import { PEEK_KEYS, type PeekKey } from "./peekKeys";

export type PeekSendResult = "sent" | "blocked" | "multiline" | "failed" | "gone";

export interface PeekSource {
  /// The pane's screen as text, or null when it is not a terminal.
  screen(paneId: Uuid): string[] | null;
  send(paneId: Uuid, text: string): Promise<PeekSendResult>;
  key(paneId: Uuid, key: PeekKey): void;
  /// Leave the office for the pane itself.
  openPane(paneId: Uuid): void;
}

export interface PeekTab {
  paneId: Uuid;
  label: string;
  status: PaneStatus;
}

/// Where the peek sits: the room's box on the floor, in CSS pixels.
export interface PeekBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

const POLL_MS = 250;
const MIN_W = 380;
const MAX_W = 760;
const MIN_H = 280;
const MAX_H = 560;
const MARGIN = 8;

/// The peek's box: centred on the room, at least `MIN_*` and at most `MAX_*`
/// big, and moved (then shrunk) to stay inside the `floorW` x `floorH` floor.
export function peekBox(room: PeekBox, floorW: number, floorH: number): PeekBox {
  const fit = (want: number, min: number, max: number, room: number): number =>
    Math.max(0, Math.min(Math.max(want, min), max, room - 2 * MARGIN));
  const w = fit(room.w - 24, MIN_W, MAX_W, floorW);
  const h = fit(room.h - 24, MIN_H, MAX_H, floorH);
  const clamp = (v: number, size: number, total: number): number =>
    Math.min(Math.max(v, MARGIN), Math.max(MARGIN, total - size - MARGIN));
  return {
    x: clamp(room.x + room.w / 2 - w / 2, w, floorW),
    y: clamp(room.y + room.h / 2 - h / 2, h, floorH),
    w,
    h,
  };
}

export class OfficePeek {
  readonly element: HTMLElement;
  private readonly tabsEl: HTMLElement;
  private readonly titleEl: HTMLElement;
  private readonly screenEl: HTMLPreElement;
  private readonly noteEl: HTMLElement;
  private readonly input: HTMLTextAreaElement;
  private readonly sendBtn: HTMLButtonElement;
  private pane: Uuid | null = null;
  private tabs: PeekTab[] = [];
  private shown = "";
  private timer: ReturnType<typeof setInterval> | null = null;
  private sending = false;

  constructor(private readonly source: PeekSource) {
    this.element = document.createElement("div");
    this.element.className = "office-peek";
    this.element.hidden = true;
    this.element.setAttribute("role", "dialog");
    // Clicks inside never reach the floor (which would close it) or a room.
    this.element.addEventListener("click", (ev) => ev.stopPropagation());
    this.element.addEventListener("dblclick", (ev) => ev.stopPropagation());

    const head = document.createElement("div");
    head.className = "office-peek__head";
    this.titleEl = document.createElement("span");
    this.titleEl.className = "office-peek__title";
    this.tabsEl = document.createElement("div");
    this.tabsEl.className = "office-peek__tabs";
    const open = document.createElement("button");
    open.type = "button";
    open.className = "office-peek__btn";
    open.textContent = t("office.peek.open");
    open.addEventListener("click", () => {
      if (this.pane) this.source.openPane(this.pane);
    });
    const close = document.createElement("button");
    close.type = "button";
    close.className = "office-peek__btn office-peek__close";
    close.textContent = "✕";
    close.title = t("office.peek.close");
    close.setAttribute("aria-label", t("office.peek.close"));
    close.addEventListener("click", () => this.close());
    head.append(this.titleEl, this.tabsEl, open, close);

    this.screenEl = document.createElement("pre");
    this.screenEl.className = "office-peek__screen";

    const keys = document.createElement("div");
    keys.className = "office-peek__keys";
    keys.setAttribute("aria-label", t("office.peek.keys"));
    for (const k of PEEK_KEYS) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "office-peek__key";
      b.textContent = k.label;
      b.addEventListener("click", () => {
        if (!this.pane) return;
        this.source.key(this.pane, k.key);
        this.poll();
        this.input.focus();
      });
      keys.appendChild(b);
    }
    this.noteEl = document.createElement("span");
    this.noteEl.className = "office-peek__note";
    keys.appendChild(this.noteEl);

    const form = document.createElement("div");
    form.className = "office-peek__form";
    this.input = document.createElement("textarea");
    this.input.className = "office-peek__input";
    this.input.rows = 2;
    this.input.placeholder = t("office.peek.placeholder");
    this.input.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") {
        ev.preventDefault();
        ev.stopPropagation();
        this.close();
        return;
      }
      // Enter sends; Shift+Enter is a new line. Never mid-composition (IME).
      if (ev.key === "Enter" && !ev.shiftKey && !ev.isComposing && ev.keyCode !== 229) {
        ev.preventDefault();
        void this.submit();
      }
    });
    this.sendBtn = document.createElement("button");
    this.sendBtn.type = "button";
    this.sendBtn.className = "office-peek__send";
    this.sendBtn.textContent = t("office.peek.send");
    this.sendBtn.addEventListener("click", () => void this.submit());
    form.append(this.input, this.sendBtn);

    this.element.append(head, this.screenEl, keys, form);
  }

  get paneId(): Uuid | null {
    return this.pane;
  }

  /// Show `paneId`'s screen in `tabs` (its room's terminals), at `box`.
  open(paneId: Uuid, tabs: PeekTab[], box: PeekBox): void {
    const switching = this.pane !== paneId;
    this.pane = paneId;
    this.element.hidden = false;
    this.update(tabs, box);
    if (switching) {
      this.shown = "";
      this.noteEl.textContent = "";
      this.poll(true);
    }
    if (this.timer === null) this.timer = setInterval(() => this.poll(), POLL_MS);
    this.input.focus({ preventScroll: true });
  }

  /// The room moved or its desks changed.
  update(tabs: PeekTab[], box: PeekBox): void {
    this.tabs = tabs;
    const s = this.element.style;
    s.left = `${box.x}px`;
    s.top = `${box.y}px`;
    s.width = `${box.w}px`;
    s.height = `${box.h}px`;
    const current = tabs.find((tab) => tab.paneId === this.pane);
    this.titleEl.textContent = current?.label ?? "";
    this.element.dataset.status = current?.status ?? "idle";
    this.element.setAttribute("aria-label", current?.label ?? "");
    this.renderTabs();
  }

  close(): void {
    if (this.timer !== null) clearInterval(this.timer);
    this.timer = null;
    this.pane = null;
    this.element.hidden = true;
  }

  dispose(): void {
    this.close();
    this.element.remove();
  }

  private renderTabs(): void {
    this.tabsEl.hidden = this.tabs.length < 2;
    this.tabsEl.replaceChildren(
      ...this.tabs.map((tab) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = `office-peek__tab office-peek__tab--${tab.status}`;
        if (tab.paneId === this.pane) b.classList.add("office-peek__tab--on");
        b.textContent = tab.label;
        b.addEventListener("click", () => {
          if (tab.paneId === this.pane) return;
          this.pane = tab.paneId;
          this.shown = "";
          this.noteEl.textContent = "";
          this.update(this.tabs, this.boxNow());
          this.poll(true);
          this.input.focus();
        });
        return b;
      }),
    );
  }

  private boxNow(): PeekBox {
    const s = this.element.style;
    return { x: parseFloat(s.left), y: parseFloat(s.top), w: parseFloat(s.width), h: parseFloat(s.height) };
  }

  /// Re-read the screen; repaint only when it changed, and keep following
  /// the bottom unless the user scrolled up to read.
  private poll(force = false): void {
    if (!this.pane) return;
    const lines = this.source.screen(this.pane);
    if (lines === null) {
      this.close();
      return;
    }
    const text = lines.join("\n");
    if (!force && text === this.shown) return;
    const el = this.screenEl;
    const atBottom = force || el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    this.shown = text;
    el.textContent = text;
    if (atBottom) el.scrollTop = el.scrollHeight;
  }

  private async submit(): Promise<void> {
    const text = this.input.value.trim();
    if (!text || !this.pane || this.sending) return;
    this.sending = true;
    this.sendBtn.disabled = true;
    try {
      const res = await this.source.send(this.pane, text);
      if (res === "sent") {
        this.input.value = "";
        this.noteEl.textContent = "";
      } else if (res === "gone") {
        this.close();
      } else {
        this.noteEl.textContent = t(`office.peek.${res}`);
      }
    } finally {
      this.sending = false;
      this.sendBtn.disabled = false;
      this.poll();
    }
  }
}
