// The office view: the active workspace drawn as an office floor. Each pane
// (or tab group) is a room laid out like the split view, each tab a desk,
// each agent a character at its desk, each subagent a helper who walks in,
// works, reports back to its lead and leaves. Work one session sends to
// another (`officeEvents`) is carried over by a character on foot.
//
// Rooms and desks are rebuilt on every update (they are cheap and static);
// characters are kept by key so they can walk between updates.

import type { Uuid } from "../types";
import { t } from "../i18n/i18n";
import { fill } from "../git/worktreeFlow";
import {
  MANAGER_HOME,
  besideSpot,
  buildOffice,
  diffChars,
  walkMs,
  type CharModel,
  type OfficeInput,
  type OfficeModel,
  type Point,
  type RoomModel,
} from "./officeModel";
import { characterSvg, deskSvg, iconSvg, lobbyProps, paletteOf, roomProps } from "./sprites";
import { kindName } from "./officeModel";
import { onOfficeMessage, type OfficeMessage } from "./officeEvents";

export interface OfficeSource {
  /// The active workspace, as the model wants it.
  input(): OfficeInput;
  workspaceName(): string;
  /// Leave the office and focus `paneId` in the split view.
  openPane(paneId: Uuid): void;
  close(): void;
  /// Call `cb` whenever the layout, agents or pane statuses may have changed.
  subscribe(cb: () => void): () => void;
}

interface Actor {
  key: string;
  el: HTMLElement;
  bubble: HTMLElement;
  /// Null for the user's own character.
  model: CharModel | null;
  pos: Point;
  /// Scripted errands run one after another on this chain.
  chain: Promise<void>;
  /// In the middle of an errand: model updates don't move it until done.
  busy: number;
  /// A helper that finished, reported and went out of the door.
  gone: boolean;
}

const BUBBLE_MS = 1600;

export class OfficeView {
  readonly element: HTMLElement;
  private readonly floor: HTMLElement;
  private readonly roomsLayer: HTMLElement;
  private readonly actorsLayer: HTMLElement;
  private readonly titleEl: HTMLElement;
  private readonly kindsEl: HTMLElement;
  private readonly emptyEl: HTMLElement;
  private readonly actors = new Map<string, Actor>();
  private readonly manager: Actor;
  private model: OfficeModel = { rooms: [], chars: [] };
  private prev: CharModel[] = [];
  private first = true;
  private disposed = false;
  private renderQueued = false;
  private readonly timers = new Set<ReturnType<typeof setTimeout>>();
  private readonly cleanups: (() => void)[] = [];
  private readonly reduced =
    typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  constructor(private readonly source: OfficeSource) {
    this.element = document.createElement("section");
    this.element.className = "office";
    this.element.tabIndex = -1;
    this.element.setAttribute("role", "region");

    const head = document.createElement("div");
    head.className = "office__head";
    this.titleEl = document.createElement("div");
    this.titleEl.className = "office__title";
    const legend = document.createElement("div");
    legend.className = "office__legend";
    for (const s of ["working", "waiting", "done", "idle"] as const) {
      const chip = document.createElement("span");
      chip.className = `office__chip office__chip--${s}`;
      chip.dataset.status = s;
      legend.appendChild(chip);
    }
    const back = document.createElement("button");
    back.type = "button";
    back.className = "office__back";
    back.addEventListener("click", () => this.source.close());
    this.kindsEl = document.createElement("div");
    this.kindsEl.className = "office__kinds";
    head.append(this.titleEl, legend, this.kindsEl, back);

    this.floor = document.createElement("div");
    this.floor.className = "office__floor";
    this.roomsLayer = document.createElement("div");
    this.roomsLayer.className = "office__rooms";
    this.actorsLayer = document.createElement("div");
    this.actorsLayer.className = "office__actors";
    const lobby = document.createElement("div");
    lobby.className = "office__lobby";
    const lobbyLabel = document.createElement("span");
    lobbyLabel.className = "office__lobby-label";
    const lobbyFurniture = document.createElement("div");
    lobbyFurniture.className = "office__lobby-props";
    lobbyFurniture.innerHTML = lobbyProps();
    lobby.append(lobbyFurniture, lobbyLabel);
    this.emptyEl = document.createElement("div");
    this.emptyEl.className = "office__empty";
    this.floor.append(this.roomsLayer, lobby, this.actorsLayer, this.emptyEl);
    this.element.append(head, this.floor);

    this.manager = this.makeActor("manager", null, characterSvg("", "manager"));
    this.manager.el.classList.add("office-char--manager", "office-char--relaxing");
    this.place(this.manager, MANAGER_HOME);
    this.manager.el.addEventListener("click", (ev) => ev.stopPropagation());

    this.element.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") {
        ev.preventDefault();
        this.source.close();
      }
    });

    this.cleanups.push(this.source.subscribe(() => this.scheduleRender()));
    // The sprite scale follows the floor's size, so a resize re-renders.
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(() => this.scheduleRender());
      ro.observe(this.floor);
      this.cleanups.push(() => ro.disconnect());
    }
    this.cleanups.push(onOfficeMessage((m) => this.deliver(m)));
    this.translate(back, lobbyLabel, legend);
    this.render();
  }

  /// Labels follow the language; called once (the overlay is short-lived and
  /// rebuilt on every open).
  private translate(back: HTMLElement, lobbyLabel: HTMLElement, legend: HTMLElement): void {
    back.textContent = t("office.back");
    back.title = t("office.backTitle");
    lobbyLabel.textContent = t("office.lobby");
    this.manager.el.title = t("office.you");
    this.element.setAttribute("aria-label", t("office.title"));
    for (const chip of legend.querySelectorAll<HTMLElement>(".office__chip")) {
      chip.textContent = t(`office.status.${chip.dataset.status}`);
    }
    this.emptyEl.textContent = t("office.empty");
  }

  focus(): void {
    this.element.focus({ preventScroll: true });
  }

  dispose(): void {
    this.disposed = true;
    for (const c of this.cleanups) c();
    for (const id of this.timers) clearTimeout(id);
    this.timers.clear();
    this.element.remove();
  }

  private scheduleRender(): void {
    if (this.renderQueued || this.disposed) return;
    this.renderQueued = true;
    requestAnimationFrame(() => {
      this.renderQueued = false;
      if (!this.disposed) this.render();
    });
  }

  // ── Rendering ─────────────────────────────────────────────────────────

  private render(): void {
    this.model = buildOffice(this.source.input());
    this.titleEl.textContent = fill(t("office.heading"), { ws: this.source.workspaceName() });
    this.applyScale();
    this.renderKinds();
    this.renderRooms();
    this.reconcile();
    this.emptyEl.hidden = this.model.chars.length > 0;
  }

  /// Sprites shrink with the smallest room so a character never outgrows
  /// its room (a character is 44px tall, a desk 34px, at scale 1).
  private applyScale(): void {
    const w = this.floor.clientWidth;
    const h = this.floor.clientHeight;
    if (w <= 0 || h <= 0) return;
    let fit = Number.POSITIVE_INFINITY;
    for (const r of this.model.rooms) {
      fit = Math.min(fit, (r.rect.h * h) / 190, (r.rect.w * w) / 150);
    }
    if (!Number.isFinite(fit)) fit = 1;
    this.floor.style.setProperty("--o-scale", String(Math.max(0.45, Math.min(1.6, fit))));
    // Rooms too small for name cards: tooltips and plates still say who is who.
    this.floor.classList.toggle("office__floor--tiny", fit < 0.6);
  }

  /// One colour chip per agent kind on the floor (who is who).
  private renderKinds(): void {
    const kinds = [...new Set(this.model.chars.filter((c) => c.role === "lead").map((c) => c.kind))];
    this.kindsEl.replaceChildren(
      ...kinds.map((k) => {
        const chip = document.createElement("span");
        chip.className = "office__kind";
        chip.style.setProperty("--kind", paletteOf(k).shirt);
        chip.textContent = kindName(k);
        return chip;
      }),
    );
  }

  private renderRooms(): void {
    const rooms: HTMLElement[] = [];
    for (const old of this.actorsLayer.querySelectorAll(".office-desk")) old.remove();
    for (const room of this.model.rooms) {
      rooms.push(this.roomEl(room));
      for (const desk of room.desks) {
        const d = document.createElement("button");
        d.type = "button";
        d.className = `office-desk office-desk--${desk.status}${desk.active ? "" : " office-desk--back"}`;
        d.innerHTML = deskSvg();
        d.title = desk.label;
        // Above characters at the same height: a seated lead sits behind it.
        this.position(d, desk.at, 20);
        d.addEventListener("click", (ev) => {
          ev.stopPropagation();
          this.source.openPane(desk.paneId);
        });
        // Tabs at the same desk row get a name card under their desk.
        if (room.desks.length > 1) {
          const card = document.createElement("span");
          card.className = "office-desk__name";
          card.textContent = desk.label;
          d.appendChild(card);
        }
        this.actorsLayer.appendChild(d);
      }
    }
    this.roomsLayer.replaceChildren(...rooms);
  }

  private roomEl(room: RoomModel): HTMLElement {
    const el = document.createElement("div");
    el.className = `office-room office-room--${room.kind} office-room--${room.status}${room.focused ? " office-room--focused" : ""}`;
    el.style.left = `${room.rect.x * 100}%`;
    el.style.top = `${room.rect.y * 100}%`;
    el.style.width = `${room.rect.w * 100}%`;
    el.style.height = `${room.rect.h * 100}%`;
    el.title = room.label;
    const inner = document.createElement("div");
    inner.className = "office-room__inner";
    const plate = document.createElement("div");
    plate.className = "office-room__plate";
    plate.textContent = room.label;
    // The back wall (3/4 view), with the room's furniture against it.
    const wall = document.createElement("div");
    wall.className = "office-room__wall";
    const props = document.createElement("div");
    props.className = "office-room__props";
    props.innerHTML = roomProps(room.kind);
    const door = document.createElement("div");
    door.className = "office-room__door";
    inner.append(wall, props, plate, door);
    el.appendChild(inner);
    const target = room.desks.find((d) => d.active)?.paneId ?? room.desks[0]?.paneId;
    el.addEventListener("click", () => {
      if (target) this.source.openPane(target);
    });
    return el;
  }

  private reconcile(): void {
    const changes = diffChars(this.prev, this.model.chars);
    const entered = new Set(changes.entered.map((c) => c.key));
    for (const c of this.model.chars) {
      let actor = this.actors.get(c.key);
      if (!actor) {
        const fresh = this.makeActor(c.key, c, characterSvg(c.kind, c.role));
        actor = fresh;
        this.actors.set(c.key, fresh);
        const room = this.roomOf(c);
        if (c.role === "sub" && c.status === "done") {
          // Its errand finished before we were looking: not drawn.
          fresh.gone = true;
          fresh.el.hidden = true;
          this.place(fresh, c.home);
        } else if (this.first || this.reduced || !room || !entered.has(c.key)) {
          this.place(fresh, c.home);
        } else {
          this.place(fresh, room.door);
          this.errand(fresh, () => this.walk(fresh, c.home));
        }
      } else {
        actor.model = c;
        // Busy again (the same id restarted): back on the floor.
        if (actor.gone && c.status !== "done") {
          actor.gone = false;
          actor.el.hidden = false;
          const room = this.roomOf(c);
          if (room) this.place(actor, room.door);
        }
        if (!actor.busy && !actor.gone) this.walk(actor, c.home);
      }
      this.look(actor);
    }
    for (const c of changes.finished) {
      const actor = this.actors.get(c.key);
      if (actor && !actor.gone) this.errand(actor, () => this.report(actor, c));
    }
    for (const key of changes.left) {
      const actor = this.actors.get(key);
      if (!actor) continue;
      this.actors.delete(key);
      this.errand(actor, async () => {
        const room = actor.model ? this.roomOf(actor.model) : undefined;
        if (!actor.gone && room && !this.reduced) {
          await this.walk(actor, room.door);
          await this.fadeOut(actor);
        }
        actor.gone = true;
        actor.el.remove();
      });
    }
    this.prev = this.model.chars;
    this.first = false;
  }

  private roomOf(c: CharModel): RoomModel | undefined {
    return this.model.rooms.find((r) => r.id === c.roomId);
  }

  // ── Characters ────────────────────────────────────────────────────────

  private makeActor(key: string, model: CharModel | null, svg: string): Actor {
    const el = document.createElement("div");
    el.className = "office-char";
    if (model?.role === "sub") el.classList.add("office-char--sub");
    el.innerHTML = svg;
    const bubble = document.createElement("span");
    bubble.className = "office-char__bubble";
    bubble.hidden = true;
    el.appendChild(bubble);
    if (model) {
      const tag = document.createElement("span");
      tag.className = "office-char__tag";
      el.appendChild(tag);
      el.addEventListener("click", (ev) => {
        ev.stopPropagation();
        if (actor.model) this.source.openPane(actor.model.paneId);
      });
    }
    this.actorsLayer.appendChild(el);
    const actor: Actor = { key, el, bubble, model, pos: MANAGER_HOME, chain: Promise.resolve(), busy: 0, gone: false };
    return actor;
  }

  /// Pose, bubble, name tag and tooltip from the actor's model.
  private look(actor: Actor): void {
    const c = actor.model;
    if (!c) return;
    const el = actor.el;
    for (const p of ["typing", "raising", "relaxing", "sleeping"]) {
      el.classList.toggle(`office-char--${p}`, c.pose === p);
    }
    el.classList.toggle("office-char--attention", c.status === "waiting");
    this.seat(actor);
    const tag = el.querySelector<HTMLElement>(".office-char__tag");
    if (tag) tag.textContent = c.label;
    const status = t(`office.status.${c.status === "waiting" ? "waiting" : c.status}`);
    el.title = [c.label, status, c.tool].filter(Boolean).join(" · ");
    if (!el.classList.contains("office-char--speaking")) {
      const b = this.bubbleFor(c);
      this.setBubble(actor, b.icon, b.text);
    }
  }

  /// A bubble is a pixel icon, a short text, or both; neither hides it.
  private setBubble(actor: Actor, icon: string | null, text = ""): void {
    actor.bubble.innerHTML = icon ? iconSvg(icon) : "";
    if (text) {
      const span = document.createElement("span");
      span.textContent = text;
      actor.bubble.appendChild(span);
    }
    actor.bubble.hidden = !icon && !text;
  }

  /// A lead at its desk sits behind it; on an errand it stands and walks.
  private seat(actor: Actor): void {
    const seated =
      actor.model?.role === "lead" &&
      actor.busy === 0 &&
      Math.abs(actor.pos.x - actor.model.home.x) < 0.002 &&
      Math.abs(actor.pos.y - actor.model.home.y) < 0.002;
    actor.el.classList.toggle("office-char--seated", seated);
  }

  private bubbleFor(c: CharModel): { icon: string | null; text: string } {
    switch (c.status) {
      case "working":
        return c.tool ? { icon: null, text: clip(c.tool, 14) } : { icon: "dots", text: "" };
      case "waiting":
        return { icon: "alert", text: "" };
      case "done":
        return { icon: c.role === "lead" ? "check" : null, text: "" };
      case "idle":
        return { icon: "zzz", text: "" };
    }
  }

  private position(el: HTMLElement, p: Point, lift = 0): void {
    el.style.left = `${p.x * 100}%`;
    el.style.top = `${p.y * 100}%`;
    el.style.zIndex = String(Math.round(p.y * 1000) + lift);
  }

  private place(actor: Actor, p: Point): void {
    actor.el.style.transition = "none";
    actor.pos = p;
    this.position(actor.el, p, 10);
    this.seat(actor);
  }

  /// Walk to `to` at a steady pace; resolves on arrival.
  private walk(actor: Actor, to: Point): Promise<void> {
    if (Math.abs(actor.pos.x - to.x) < 0.002 && Math.abs(actor.pos.y - to.y) < 0.002) {
      return Promise.resolve();
    }
    if (this.reduced) {
      this.place(actor, to);
      return Promise.resolve();
    }
    const ms = walkMs(actor.pos, to);
    actor.el.classList.remove("office-char--seated");
    actor.el.classList.toggle("office-char--face-left", to.x < actor.pos.x);
    actor.el.classList.add("office-char--walking");
    actor.el.style.transition = `left ${ms}ms linear, top ${ms}ms linear`;
    // Force the transition to start from the current spot.
    void actor.el.offsetWidth;
    actor.pos = to;
    this.position(actor.el, to, 10);
    return this.sleep(ms).then(() => {
      // Arrived: face the room again.
      actor.el.classList.remove("office-char--walking", "office-char--face-left");
      this.seat(actor);
    });
  }

  private sleep(ms: number): Promise<void> {
    return new Promise((resolve) => {
      if (this.disposed) return resolve();
      const id = setTimeout(() => {
        this.timers.delete(id);
        resolve();
      }, ms);
      this.timers.add(id);
    });
  }

  /// Run `steps` after the actor's earlier errands; while it runs, model
  /// updates leave the actor alone, and afterwards it goes back where the
  /// latest model wants it.
  private errand(actor: Actor, steps: () => Promise<void>): void {
    actor.busy++;
    actor.el.classList.remove("office-char--seated");
    actor.chain = actor.chain
      .then(() => (this.disposed ? undefined : steps()))
      .catch((e) => console.warn("office errand failed:", e))
      .then(async () => {
        actor.busy--;
        if (this.disposed || actor.busy > 0 || actor.gone) return;
        const home = actor.model ? actor.model.home : MANAGER_HOME;
        await this.walk(actor, home);
        this.seat(actor);
      });
  }

  /// Show pixel icon `icon` in a speech bubble for `ms`.
  private async say(actor: Actor, icon: string, ms = BUBBLE_MS): Promise<void> {
    actor.el.classList.add("office-char--speaking");
    this.setBubble(actor, icon);
    await this.sleep(ms);
    actor.el.classList.remove("office-char--speaking");
    if (actor.model) this.look(actor);
    else actor.bubble.hidden = true;
  }

  private async fadeOut(actor: Actor): Promise<void> {
    actor.el.classList.add("office-char--leaving");
    await this.sleep(400);
  }

  /// A finished helper walks to its lead, hands over its report, and leaves.
  private async report(actor: Actor, c: CharModel): Promise<void> {
    const room = this.roomOf(c);
    const lead = this.actors.get(`${c.paneId}:lead`);
    if (room && lead?.model && !this.reduced) {
      await this.walk(actor, besideSpot(lead.model.home, room.rect));
      await this.say(actor, "paper", 1200);
    }
    if (room && !this.reduced) {
      await this.walk(actor, room.door);
      await this.fadeOut(actor);
    }
    actor.gone = true;
    actor.el.hidden = true;
    actor.el.classList.remove("office-char--leaving");
  }

  /// Work one session sent to another: its lead (or, when the user sent it,
  /// the user's own character) walks over with it.
  private deliver(m: OfficeMessage): void {
    const targetRoom = this.model.rooms.find((r) => r.desks.some((d) => d.paneId === m.to));
    if (!targetRoom) return;
    const fromLead = m.from ? this.actors.get(`${m.from}:lead`) : undefined;
    const messenger = fromLead && !fromLead.gone && m.from !== m.to ? fromLead : this.manager;
    const targetLead = this.actors.get(`${m.to}:lead`);
    const icon = m.kind === "review" ? "review" : m.kind === "commit" ? "box" : "chat";
    this.errand(messenger, async () => {
      const spot = targetLead?.model
        ? besideSpot(targetLead.model.home, targetRoom.rect)
        : targetRoom.door;
      await this.walk(messenger, spot);
      const told = targetLead && !targetLead.busy ? this.say(targetLead, "alert", BUBBLE_MS) : null;
      await this.say(messenger, icon);
      await told;
    });
  }
}

function clip(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}
