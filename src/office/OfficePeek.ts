// The office view's peek: a window over a room with its terminal's screen,
// key buttons and a prompt box, so the user can read and answer an agent
// without leaving the office. The screen is polled while open (it is plain
// text read off the terminal's buffer; cheap). One peek at a time; a tab
// group's terminals get a tab each.

import type { Uuid } from "../types";
import type { PaneStatus } from "../terminal/paneStatus";
import { t } from "../i18n/i18n";
import { PEEK_KEYS, directKey, menuOptions, type MenuOption, type PeekKey } from "./peekKeys";

export type PeekSendResult = "sent" | "blocked" | "multiline" | "failed" | "gone";

export interface PeekSource {
  /// The pane's screen as text, or null when it is not a terminal.
  screen(paneId: Uuid): string[] | null;
  send(paneId: Uuid, text: string): Promise<PeekSendResult>;
  key(paneId: Uuid, key: PeekKey): void;
  /// Write raw bytes to the pane, as typing would.
  type(paneId: Uuid, data: string): void;
  /// The pane's app wants application cursor keys (DECCKM).
  cursorKeys(paneId: Uuid): boolean;
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
/// Keys the prompt box passes to the terminal while it is empty.
const FORWARD_WHEN_EMPTY = new Set(["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Tab", "Enter"]);
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
  private readonly optionsEl: HTMLElement;
  private shownOptions = "";
  /// The text field under a choice that takes typed text, and that choice.
  private readonly answerEl: HTMLElement;
  private readonly answerInput: HTMLInputElement;
  private answerFor: MenuOption | null = null;
  private readonly noteEl: HTMLElement;
  private readonly input: HTMLTextAreaElement;
  private readonly sendBtn: HTMLButtonElement;
  private pane: Uuid | null = null;
  private tabs: PeekTab[] = [];
  private shown = "";
  private timer: ReturnType<typeof setInterval> | null = null;
  private sending = false;
  private options: readonly MenuOption[] = [];
  private readonly waitEl: HTMLElement;
  private readonly nextBtn: HTMLButtonElement;
  /// A menu choice was answered in `paneId` (the queue moves on).
  onAnswered: ((paneId: Uuid) => void) | null = null;
  /// The "next waiting" button was pressed.
  onNext: (() => void) | null = null;

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
    // A status lamp: blue working, red needs you, green done.
    const lamp = document.createElement("span");
    lamp.className = "office-peek__lamp";
    // How long it has been waiting, and the way to the next one waiting.
    this.waitEl = document.createElement("span");
    this.waitEl.className = "office-peek__wait";
    this.nextBtn = document.createElement("button");
    this.nextBtn.type = "button";
    this.nextBtn.className = "office-peek__btn office-peek__next";
    this.nextBtn.hidden = true;
    this.nextBtn.addEventListener("click", () => this.onNext?.());
    head.append(lamp, this.titleEl, this.waitEl, this.tabsEl, this.nextBtn, open, close);

    this.screenEl = document.createElement("pre");
    this.screenEl.className = "office-peek__screen";
    // Click the screen and type: keys go straight to the terminal, like the
    // terminal itself (arrows, Enter, Esc, Ctrl+C, …). Composed text (IME)
    // can't be typed here; the prompt box below takes it.
    this.screenEl.tabIndex = 0;
    this.screenEl.title = t("office.peek.direct");
    this.screenEl.addEventListener("keydown", (ev) => {
      if (!this.pane) return;
      const bytes = directKey(ev, this.source.cursorKeys(this.pane));
      ev.stopPropagation();
      if (bytes === null) return;
      ev.preventDefault();
      this.send(bytes);
    });
    this.screenEl.addEventListener("paste", (ev) => {
      const text = ev.clipboardData?.getData("text/plain");
      if (!text) return;
      ev.preventDefault();
      this.send(text);
    });

    // The choices of a menu on screen, one click each.
    this.optionsEl = document.createElement("div");
    this.optionsEl.className = "office-peek__options";
    this.optionsEl.setAttribute("role", "group");
    this.optionsEl.setAttribute("aria-label", t("office.peek.options"));
    this.optionsEl.hidden = true;

    // A choice that takes typed text ("tell Claude what to do differently",
    // "Type something") opens this field instead of sending its number at
    // once; Enter sends the number, the text and Enter together. Its own
    // text box, so Korean and other IME input work.
    this.answerEl = document.createElement("div");
    this.answerEl.className = "office-peek__answer";
    this.answerEl.hidden = true;
    const answerLabel = document.createElement("span");
    answerLabel.className = "office-peek__answer-label";
    this.answerInput = document.createElement("input");
    this.answerInput.type = "text";
    this.answerInput.className = "office-peek__answer-input";
    this.answerInput.placeholder = t("office.peek.answerPlaceholder");
    const answerSend = document.createElement("button");
    answerSend.type = "button";
    answerSend.className = "office-peek__send";
    answerSend.textContent = t("office.peek.send");
    answerSend.addEventListener("click", () => this.submitAnswer());
    this.answerInput.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.isComposing || ev.keyCode === 229) return;
      if (ev.key === "Enter") {
        ev.preventDefault();
        this.submitAnswer();
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        this.closeAnswer();
      }
    });
    this.answerEl.append(answerLabel, this.answerInput, answerSend);

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
      const composing = ev.isComposing || ev.keyCode === 229;
      // Alt+N: on to the next agent waiting.
      if (ev.altKey && ev.code === "KeyN" && this.onNext && !this.nextBtn.hidden) {
        ev.preventDefault();
        ev.stopPropagation();
        this.onNext();
        return;
      }
      // With the box empty and a menu up, a digit picks that choice.
      if (!composing && this.input.value === "" && /^[1-9]$/.test(ev.key) && !ev.ctrlKey && !ev.metaKey && !ev.altKey) {
        const o = this.options.find((x) => x.n === ev.key);
        if (o) {
          ev.preventDefault();
          ev.stopPropagation();
          this.pick(o);
          return;
        }
      }
      // With the box empty, arrows, Tab and Enter drive the terminal (move
      // through a menu, pick, cycle modes) instead of the empty box.
      if (!composing && this.input.value === "" && this.pane && FORWARD_WHEN_EMPTY.has(ev.key) && !(ev.key === "Enter" && ev.shiftKey)) {
        const bytes = directKey(ev, this.source.cursorKeys(this.pane));
        if (bytes !== null) {
          ev.preventDefault();
          this.send(bytes);
          return;
        }
      }
      // Enter sends; Shift+Enter is a new line. Never mid-composition (IME).
      if (ev.key === "Enter" && !ev.shiftKey && !composing) {
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

    // The frame is clipped to pixel-stepped corners; the outer box carries
    // the drop shadow, which a clipped box would cut off.
    const frame = document.createElement("div");
    frame.className = "office-peek__frame";
    frame.append(head, this.screenEl, this.optionsEl, this.answerEl, keys, form);
    this.element.appendChild(frame);
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

  /// The wait shown by the title ("2m"), or "" for none.
  setWait(text: string): void {
    this.waitEl.textContent = text;
    this.waitEl.hidden = text === "";
  }

  /// The next agent waiting after this one (its label), or null.
  setNext(label: string | null): void {
    this.nextBtn.hidden = label === null;
    if (label !== null) {
      this.nextBtn.textContent = `${t("office.queue.next")} ▶`;
      this.nextBtn.title = `${label} (Alt+N)`;
    }
  }

  /// Put `text` in the prompt box, for the user to edit and send.
  prefill(text: string): void {
    this.input.value = text;
    this.input.focus();
    this.input.setSelectionRange(text.length, text.length);
  }

  /// Answer the menu with `o` (a choice that takes text asks for it first).
  private pick(o: MenuOption): void {
    if (o.input) {
      this.openAnswer(o);
      return;
    }
    const pane = this.pane;
    this.send(o.n);
    this.input.focus();
    if (pane) this.onAnswered?.(pane);
  }

  close(): void {
    this.answerFor = null;
    this.answerEl.hidden = true;
    this.answerInput.value = "";
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
          this.answerFor = null;
          this.answerEl.hidden = true;
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

  /// Raw bytes to the terminal, then a quick look at what changed.
  private send(bytes: string): void {
    if (!this.pane) return;
    this.source.type(this.pane, bytes);
    setTimeout(() => this.poll(), 60);
  }

  private openAnswer(o: MenuOption): void {
    this.answerFor = o;
    const label = this.answerEl.querySelector<HTMLElement>(".office-peek__answer-label");
    if (label) label.textContent = `${o.n}.`;
    this.answerEl.title = o.label;
    this.answerEl.hidden = false;
    this.shownOptions = "";
    this.poll(true);
    this.answerInput.focus();
  }

  private closeAnswer(): void {
    this.answerFor = null;
    this.answerEl.hidden = true;
    this.answerInput.value = "";
    this.shownOptions = "";
    this.poll(true);
    this.input.focus();
  }

  /// Pick the choice, give the agent's field a moment to open, then type the
  /// text and press Enter.
  private submitAnswer(): void {
    const o = this.answerFor;
    const text = this.answerInput.value.replace(/\r?\n/g, " ").trim();
    if (!o || !text || !this.pane) return;
    const pane = this.pane;
    this.source.type(pane, o.n);
    setTimeout(() => {
      this.source.type(pane, text);
      setTimeout(() => {
        this.source.type(pane, "\r");
        this.poll();
        this.onAnswered?.(pane);
      }, 120);
    }, 250);
    this.closeAnswer();
  }

  private renderOptions(options: readonly MenuOption[]): void {
    const key = `${this.answerFor?.n ?? ""}|` + options.map((o) => `${o.n}${o.selected ? "*" : ""}${o.label}`).join("\n");
    if (key === this.shownOptions) return;
    // The menu went away (answered in the terminal, or timed out): so does
    // the field for one of its choices.
    if (this.answerFor && !options.some((o) => o.n === this.answerFor?.n && o.label === this.answerFor.label)) {
      this.answerFor = null;
      this.answerEl.hidden = true;
    }
    this.shownOptions = key;
    this.optionsEl.hidden = options.length === 0;
    this.optionsEl.replaceChildren(
      ...options.map((o) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = `office-peek__option${o.selected ? " office-peek__option--on" : ""}`;
        b.textContent = `${o.n}. ${o.label}`;
        b.title = o.label;
        if (o.input) b.classList.add("office-peek__option--input");
        if (this.answerFor?.n === o.n) b.classList.add("office-peek__option--answering");
        // Agents take the number as "pick this one"; a choice that wants
        // text asks for it first.
        b.addEventListener("click", () => this.pick(o));
        return b;
      }),
    );
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
    this.options = menuOptions(lines);
    this.renderOptions(this.options);
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
