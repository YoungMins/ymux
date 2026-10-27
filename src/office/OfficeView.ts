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
  LOBBY_H,
  MANAGER_HOME,
  besideSpot,
  buildOffice,
  diffChars,
  planPath,
  type Place,
  type CharModel,
  type OfficeInput,
  type OfficeModel,
  type Point,
  type RoomModel,
} from "./officeModel";
import { characterSvg, corkSvg, deskSvg, flagSvg, iconSvg, lobbyProps, mailboxSvg, paletteOf, petSvg, roomProps } from "./sprites";
import { petErrand, petsFor, type PetLobby, type PetSpot } from "./petModel";
import type { PetSpecies } from "./petArt";
import type { LogEntry } from "./officeLog";
import { getLang } from "../i18n/i18n";
import { kindName } from "./officeModel";
import { onOfficeMessage, type OfficeMessage } from "./officeEvents";
import { THEME_IDS, loadTheme, saveTheme, type ThemeId } from "./themes";
import { OfficePeek, peekBox, type PeekSource, type PeekTab } from "./OfficePeek";
import { HeroPanel, type HeroRow } from "./HeroPanel";
import {
  accountXp,
  levelOf,
  levelUps,
  loadHeroClass,
  loadHeroForm,
  loadSeenLevels,
  saveHeroClass,
  saveHeroForm,
  saveSeenLevels,
  tierOf,
  TIER,
  type HeroClass,
  type HeroForm,
} from "./heroModel";
import type { Hero } from "./officeModel";
import type { TokenUsage } from "../tokenmonitor/model";
import type { AccountProfile } from "../types";
import { ACCOUNT_NONE } from "../accounts/accountModel";

/// Corridor width, in CSS pixels (wide enough for a character).
const CORRIDOR_PX = 40;
/// Walking pace, in CSS pixels per second, so a walk takes as long as its
/// on-screen distance whatever the window size.
const WALK_PX_PER_S = 240;

export interface OfficeSource extends PeekSource {
  /// The active workspace, as the model wants it.
  input(): OfficeInput;
  workspaceName(): string;
  /// Leave the office and focus `paneId` in the split view.
  openPane(paneId: Uuid): void;
  close(): void;
  /// Call `cb` whenever the layout, agents, pane statuses or the office log
  /// may have changed.
  subscribe(cb: () => void): () => void;
  /// The lobby board's notes, newest first (`officeLog`).
  log(): readonly LogEntry[];
  /// The account a pane runs under: "" default, "none" home, else a profile id.
  accountOf(paneId: Uuid): string;
  accounts(): readonly AccountProfile[];
  /// The Usage view's numbers (all-time tokens per account are experience).
  tokenUsage(): Promise<TokenUsage>;
}

/// How often the heroes' experience is re-read while the office is open.
const XP_REFRESH_MS = 5 * 60 * 1000;
const LEVEL_UP_MS = 4000;

/// One of the office pets, wandering on its own (`petModel`).
interface Pet {
  species: PetSpecies;
  el: HTMLElement;
  bubble: HTMLElement;
  spot: PetSpot;
  /// Cleared when the office has fewer rooms and this pet goes home.
  alive: boolean;
}

/// Pets amble: slower than the agents.
const PET_PX_PER_S = 70;

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
  /// The room it is in, or null for the lobby.
  room: string | null;
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
  private model: OfficeModel = { rooms: [], chars: [], graph: { nodes: [], edges: [] }, lobbyGate: MANAGER_HOME };
  private prev: CharModel[] = [];
  private first = true;
  private disposed = false;
  private renderQueued = false;
  private readonly timers = new Set<ReturnType<typeof setTimeout>>();
  private readonly cleanups: (() => void)[] = [];
  private readonly fixturesLayer: HTMLElement;
  private readonly board: HTMLButtonElement;
  private readonly boardNotes: HTMLElement;
  private readonly boardPanel: HTMLElement;
  private readonly lobbyFurniture: HTMLElement;
  private readonly lobbyLabel: HTMLElement;
  private readonly themeSelect: HTMLSelectElement;
  private readonly peek: OfficePeek;
  private readonly heroes: HeroPanel;
  /// All-time tokens per account; null until the first read.
  private xp: Map<string, number> | null = null;
  private readonly classes = new Map<string, HeroClass>();
  private readonly forms = new Map<string, HeroForm>();
  private readonly levelUpEl: HTMLElement;
  private readonly pets: Pet[] = [];
  private theme: ThemeId = loadTheme();
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
    // The theme: the same floor plan as an office, a cave, a castle, …
    this.themeSelect = document.createElement("select");
    this.themeSelect.className = "office__theme";
    for (const id of THEME_IDS) {
      const opt = document.createElement("option");
      opt.value = id;
      this.themeSelect.appendChild(opt);
    }
    this.themeSelect.value = this.theme;
    this.themeSelect.addEventListener("change", () => this.setTheme(this.themeSelect.value as ThemeId));
    // Arrow keys and Escape belong to the select while it has focus.
    this.themeSelect.addEventListener("keydown", (ev) => ev.stopPropagation());
    const heroBtn = document.createElement("button");
    heroBtn.type = "button";
    heroBtn.className = "office__back office__heroes-btn";
    heroBtn.textContent = `⚔ ${t("office.heroes")}`;
    heroBtn.title = t("office.heroes.title");
    heroBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      this.toggleHeroes();
    });
    head.append(this.titleEl, legend, this.kindsEl, heroBtn, this.themeSelect, back);

    this.floor = document.createElement("div");
    this.floor.className = "office__floor";
    this.roomsLayer = document.createElement("div");
    this.roomsLayer.className = "office__rooms";
    this.actorsLayer = document.createElement("div");
    this.actorsLayer.className = "office__actors";
    const lobby = document.createElement("div");
    lobby.className = "office__lobby";
    this.lobbyLabel = document.createElement("span");
    this.lobbyLabel.className = "office__lobby-label";
    this.lobbyFurniture = document.createElement("div");
    this.lobbyFurniture.className = "office__lobby-props";
    // The board: notes pinned for each agent finishing or work handed over;
    // a click opens the full list.
    this.board = document.createElement("button");
    this.board.type = "button";
    this.board.className = "office-board";
    this.board.innerHTML = corkSvg(36, 20);
    this.boardNotes = document.createElement("span");
    this.boardNotes.className = "office-board__notes";
    this.board.appendChild(this.boardNotes);
    this.board.addEventListener("click", (ev) => {
      ev.stopPropagation();
      this.toggleBoardPanel();
    });
    this.boardPanel = document.createElement("div");
    this.boardPanel.className = "office-board__panel";
    this.boardPanel.hidden = true;
    lobby.append(this.lobbyFurniture, this.board, this.lobbyLabel);
    this.emptyEl = document.createElement("div");
    this.emptyEl.className = "office__empty";
    this.fixturesLayer = document.createElement("div");
    this.fixturesLayer.className = "office__fixtures";
    this.peek = new OfficePeek(source);
    this.heroes = new HeroPanel(
      (account, cls) => this.pickClass(account, cls),
      (account, form) => this.pickForm(account, form),
    );
    this.levelUpEl = document.createElement("div");
    this.levelUpEl.className = "office-levelup";
    this.levelUpEl.hidden = true;
    this.levelUpEl.setAttribute("role", "status");
    this.floor.append(
      this.roomsLayer,
      lobby,
      this.fixturesLayer,
      this.actorsLayer,
      this.emptyEl,
      this.boardPanel,
      this.peek.element,
      this.heroes.element,
      this.levelUpEl,
    );
    this.floor.addEventListener("click", () => {
      this.boardPanel.hidden = true;
      this.peek.close();
      this.heroes.hide();
    });
    this.element.append(head, this.floor);

    this.manager = this.makeActor("manager", null, characterSvg("", "manager"));
    this.manager.el.classList.add("office-char--manager", "office-char--relaxing");
    this.place(this.manager, MANAGER_HOME);
    this.manager.el.addEventListener("click", (ev) => ev.stopPropagation());

    this.element.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") {
        ev.preventDefault();
        if (this.peek.paneId) this.peek.close();
        else if (this.heroes.open) this.heroes.hide();
        else if (!this.boardPanel.hidden) this.boardPanel.hidden = true;
        else this.source.close();
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
    void this.loadXp();
    const xpTimer = setInterval(() => void this.loadXp(), XP_REFRESH_MS);
    this.cleanups.push(() => clearInterval(xpTimer));
    this.translate(back, legend);
    this.applyTheme();
    this.render();
  }

  /// Labels follow the language; called once (the overlay is short-lived and
  /// rebuilt on every open).
  private translate(back: HTMLElement, legend: HTMLElement): void {
    back.textContent = t("office.back");
    back.title = t("office.backTitle");
    this.themeSelect.title = t("office.theme");
    this.themeSelect.setAttribute("aria-label", t("office.theme"));
    for (const opt of this.themeSelect.options) opt.textContent = t(`office.theme.${opt.value}`);
    this.manager.el.title = t("office.you");
    this.element.setAttribute("aria-label", t("office.title"));
    for (const chip of legend.querySelectorAll<HTMLElement>(".office__chip")) {
      chip.textContent = t(`office.status.${chip.dataset.status}`);
    }
    this.emptyEl.textContent = t("office.empty");
    this.board.title = t("office.board");
    this.board.setAttribute("aria-label", t("office.board"));
  }

  private setTheme(id: ThemeId): void {
    if (id === this.theme) return;
    this.theme = id;
    saveTheme(id);
    this.applyTheme();
    this.render();
  }

  /// The parts drawn once per theme: the floor's class, the lobby's
  /// furniture and name. Rooms and desks follow on the next render.
  private applyTheme(): void {
    for (const id of THEME_IDS) this.element.classList.toggle(`office--theme-${id}`, id === this.theme);
    this.lobbyFurniture.innerHTML = lobbyProps(this.theme);
    this.lobbyLabel.textContent = t(this.theme === "office" ? "office.lobby" : `office.lobby.${this.theme}`);
  }

  focus(): void {
    this.element.focus({ preventScroll: true });
  }

  dispose(): void {
    this.disposed = true;
    for (const c of this.cleanups) c();
    for (const id of this.timers) clearTimeout(id);
    this.timers.clear();
    this.peek.dispose();
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
    const input = { ...this.source.input(), heroOf: (id: Uuid) => this.heroOf(id) };
    const w = this.floor.clientWidth;
    const h = this.floor.clientHeight;
    if (w > 0 && h > 0) {
      // Corridors a fixed number of pixels wide, like everything drawn on
      // the floor: a bigger window means more room, not bigger pixels.
      this.model = buildOffice({ ...input, corridor: { x: CORRIDOR_PX / w, y: CORRIDOR_PX / h } });
    } else {
      this.model = buildOffice(input);
    }
    this.titleEl.textContent = `${t(`office.theme.${this.theme}`)} — ${this.source.workspaceName()}`;
    this.applyScale();
    this.renderKinds();
    this.renderRooms();
    this.reconcile();
    this.ensurePets();
    this.emptyEl.hidden = this.model.chars.length > 0;
  }

  /// How the smallest room compares with the space a desk and its character
  /// need (1 = enough).
  private fitScale(model: OfficeModel, w: number, h: number): number {
    let fit = Number.POSITIVE_INFINITY;
    for (const r of model.rooms) {
      fit = Math.min(fit, (r.rect.h * h) / 190, (r.rect.w * w) / 150);
    }
    return Number.isFinite(fit) ? fit : 1;
  }

  private applyScale(): void {
    const w = this.floor.clientWidth;
    const h = this.floor.clientHeight;
    if (w <= 0 || h <= 0) return;
    const fit = this.fitScale(this.model, w, h);
    // Sprites, furniture and floor tiles keep one size whatever the window:
    // whole art pixels (3 CSS px) that never stretch. The rooms take up the
    // slack instead.
    this.floor.style.setProperty("--o-scale", "1");
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
        d.innerHTML = deskSvg(this.theme);
        d.title = desk.label;
        // Above characters at the same height: a seated lead sits behind it.
        this.position(d, desk.at, 20);
        d.addEventListener("click", (ev) => {
          ev.stopPropagation();
          this.peekOrOpen(desk.paneId);
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
    this.placePeek();
    this.renderFixtures();
    this.renderBoard();
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
    props.innerHTML = roomProps(room.kind, this.theme);
    const door = document.createElement("div");
    door.className = "office-room__door";
    inner.append(wall, props, plate, door);
    el.appendChild(inner);
    const active = room.desks.find((d) => d.active) ?? room.desks[0];
    const target = this.canPeek(active?.paneId) ? active?.paneId : (room.desks.find((d) => this.canPeek(d.paneId))?.paneId ?? active?.paneId);
    // A click peeks at its terminal; a double click goes to the pane.
    el.addEventListener("click", (ev) => {
      ev.stopPropagation();
      if (target) this.peekOrOpen(target);
    });
    el.addEventListener("dblclick", (ev) => {
      ev.stopPropagation();
      if (target) this.source.openPane(target);
    });
    return el;
  }

  // ── Heroes: accounts levelling up with their tokens ──────────────────

  private heroOf(paneId: Uuid): Hero | null {
    if (!this.xp) return null;
    const account = this.source.accountOf(paneId);
    const { level } = levelOf(this.xp.get(account) ?? 0);
    return { cls: this.classOf(account), form: this.formOf(account), tier: tierOf(level), level };
  }

  private classOf(account: string): HeroClass {
    let cls = this.classes.get(account);
    if (!cls) {
      cls = loadHeroClass(account);
      this.classes.set(account, cls);
    }
    return cls;
  }

  private formOf(account: string): HeroForm {
    let form = this.forms.get(account);
    if (!form) {
      form = loadHeroForm(account);
      this.forms.set(account, form);
    }
    return form;
  }

  private async loadXp(): Promise<void> {
    let usage: TokenUsage;
    try {
      usage = await this.source.tokenUsage();
    } catch (e) {
      console.warn("office: token usage unavailable:", e);
      if (!this.xp) this.xp = new Map();
      if (this.heroes.open) this.heroes.render(this.heroRows());
      return;
    }
    if (this.disposed) return;
    this.xp = accountXp(usage.rows);
    const levels = new Map([...this.xp].map(([a, n]) => [a, levelOf(n).level]));
    const ups = levelUps(loadSeenLevels(), levels);
    saveSeenLevels(levels);
    this.scheduleRender();
    if (this.heroes.open) this.heroes.render(this.heroRows());
    if (ups.length > 0) this.announceLevelUp(ups[0].account, ups[0].level);
  }

  private accountName(account: string): string {
    if (account === "") return t("usage.default_account");
    if (account === ACCOUNT_NONE) return t("usage.home_account");
    return this.source.accounts().find((p) => p.id === account)?.name || account;
  }

  /// Every account with tokens or a profile, plus the default.
  private heroRows(): HeroRow[] | null {
    if (!this.xp) return null;
    const ids = new Set<string>(["", ...this.xp.keys(), ...this.source.accounts().map((p) => p.id)]);
    return [...ids].map((account) => ({
      account,
      name: this.accountName(account),
      color: this.source.accounts().find((p) => p.id === account)?.color ?? "",
      tokens: this.xp?.get(account) ?? 0,
      cls: this.classOf(account),
      form: this.formOf(account),
    }));
  }

  private toggleHeroes(): void {
    if (this.heroes.open) {
      this.heroes.hide();
      return;
    }
    this.peek.close();
    this.boardPanel.hidden = true;
    this.heroes.render(this.heroRows());
    this.heroes.show();
  }

  private pickClass(account: string, cls: HeroClass): void {
    saveHeroClass(account, cls);
    this.classes.set(account, cls);
    this.heroes.render(this.heroRows());
    this.scheduleRender();
  }

  private pickForm(account: string, form: HeroForm): void {
    saveHeroForm(account, form);
    this.forms.set(account, form);
    this.heroes.render(this.heroRows());
    this.scheduleRender();
  }

  /// A banner over the floor, and the account's agents jump for joy.
  private announceLevelUp(account: string, level: number): void {
    this.levelUpEl.textContent = fill(t("office.heroes.levelUp"), { name: this.accountName(account), n: String(level) });
    this.levelUpEl.hidden = false;
    void this.sleep(LEVEL_UP_MS).then(() => {
      this.levelUpEl.hidden = true;
    });
    for (const actor of this.actors.values()) {
      if (actor.model?.role === "lead" && !actor.gone && this.source.accountOf(actor.model.paneId) === account) {
        this.cheer(actor);
      }
    }
  }

  // ── Peek: a room's terminal, read and answered in place ───────────────

  private canPeek(paneId: Uuid | undefined): boolean {
    return paneId !== undefined && this.source.screen(paneId) !== null;
  }

  /// Peek at a terminal; any other pane kind is opened as before.
  private peekOrOpen(paneId: Uuid): void {
    if (!this.canPeek(paneId)) {
      this.source.openPane(paneId);
      return;
    }
    const at = this.peekAt(paneId);
    if (!at) return;
    this.boardPanel.hidden = true;
    this.peek.open(paneId, at.tabs, at.box);
  }

  /// The open peek follows its room (resizes, layout changes) and closes
  /// when the pane is gone.
  private placePeek(): void {
    const id = this.peek.paneId;
    if (!id) return;
    const at = this.peekAt(id);
    if (at) this.peek.update(at.tabs, at.box);
    else this.peek.close();
  }

  private peekAt(paneId: Uuid): { tabs: PeekTab[]; box: ReturnType<typeof peekBox> } | null {
    const room = this.model.rooms.find((r) => r.desks.some((d) => d.paneId === paneId));
    if (!room) return null;
    const w = this.floor.clientWidth;
    const h = this.floor.clientHeight;
    const tabs = room.desks
      .filter((d) => this.canPeek(d.paneId))
      .map((d) => ({ paneId: d.paneId, label: d.label, status: d.status }));
    const box = peekBox({ x: room.rect.x * w, y: room.rect.y * h, w: room.rect.w * w, h: room.rect.h * h }, w, h);
    return { tabs, box };
  }

  // ── Pets: animals wandering the corridors and the lobby ─────────────────

  private petLobby(): PetLobby {
    return { top: 1 - LOBBY_H * 0.42, bottom: 1 - LOBBY_H * 0.1, gate: this.model.lobbyGate };
  }

  /// As many pets as the office has room for (`petsFor`), each on its own
  /// errand loop.
  private ensurePets(): void {
    if (this.floor.clientWidth <= 0) return;
    const want = petsFor(this.model.rooms.length);
    while (this.pets.length > want.length) {
      const pet = this.pets.pop()!;
      pet.alive = false;
      pet.el.remove();
    }
    for (const species of want.slice(this.pets.length)) {
      const el = document.createElement("div");
      el.className = `office-pet office-pet--${species}`;
      el.innerHTML = petSvg(species);
      const bubble = document.createElement("span");
      bubble.className = "office-pet__bubble";
      bubble.hidden = true;
      el.appendChild(bubble);
      const nodes = this.model.graph.nodes;
      const start: PetSpot =
        nodes.length > 0 && Math.random() < 0.7
          ? { at: nodes[Math.floor(Math.random() * nodes.length)], lobby: false }
          : { at: { x: 0.1 + Math.random() * 0.8, y: 1 - LOBBY_H * 0.2 }, lobby: true };
      const pet: Pet = { species, el, bubble, spot: start, alive: true };
      el.addEventListener("click", (ev) => {
        ev.stopPropagation();
        this.petBubble(pet, "heart", 1500);
      });
      this.position(el, start.at, 5);
      this.actorsLayer.appendChild(el);
      this.pets.push(pet);
      void this.petLife(pet);
    }
  }

  private async petLife(pet: Pet): Promise<void> {
    // Not all at once.
    await this.sleep(500 + Math.random() * 2500);
    while (!this.disposed && pet.alive) {
      const errand = petErrand(this.model.graph, this.petLobby(), pet.spot);
      pet.el.classList.remove("office-pet--sleeping");
      pet.bubble.hidden = true;
      if (!this.reduced) {
        pet.el.classList.add("office-pet--walking");
        let at = pet.spot.at;
        for (const to of errand.path) {
          if (this.disposed || !pet.alive) return;
          const ms = this.walkTime(at, to, true, PET_PX_PER_S);
          if (Math.abs(to.x - at.x) > 0.002) pet.el.classList.toggle("office-pet--face-left", to.x < at.x);
          pet.el.style.transition = `left ${ms}ms linear, top ${ms}ms linear`;
          void pet.el.offsetWidth;
          this.position(pet.el, to, 5);
          at = to;
          await this.sleep(ms);
        }
        pet.el.classList.remove("office-pet--walking");
        pet.spot = errand.end;
      }
      if (errand.rest === "sleep") {
        pet.el.classList.add("office-pet--sleeping");
        this.petBubble(pet, "zzz", errand.restMs);
      }
      await this.sleep(errand.restMs);
    }
  }

  private petBubble(pet: Pet, icon: string, ms: number): void {
    pet.bubble.innerHTML = iconSvg(icon);
    pet.bubble.hidden = false;
    void this.sleep(ms).then(() => {
      if (pet.bubble.innerHTML === iconSvg(icon)) pet.bubble.hidden = true;
    });
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
        if (!actor.busy && !actor.gone) {
          // Its pane moved to another room: walk there by the corridors.
          if (actor.room !== c.roomId) this.errand(actor, () => this.travel(actor!, { at: c.home, room: c.roomId }));
          else void this.walk(actor, c.home);
        }
      }
      this.look(actor);
    }
    for (const c of changes.cheered) {
      const actor = this.actors.get(c.key);
      if (actor && !actor.gone && !this.first) this.cheer(actor);
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

  // ── Flags, mailboxes and the board ────────────────────────────────────

  /// Outside each door: a flag while its agent's finished work is unseen,
  /// a mailbox while it holds letters.
  private renderFixtures(): void {
    const out: HTMLElement[] = [];
    for (const room of this.model.rooms) {
      const outside = { x: room.door.x, y: room.rect.y + room.rect.h };
      if (room.doneDesk) {
        const flag = document.createElement("button");
        flag.type = "button";
        flag.className = "office-flag";
        flag.innerHTML = flagSvg();
        flag.title = t("office.flag");
        this.position(flag, outside, 2);
        const target = room.doneDesk;
        flag.addEventListener("click", (ev) => {
          ev.stopPropagation();
          this.source.openPane(target);
        });
        out.push(flag);
      }
      const box = document.createElement("button");
      box.type = "button";
      box.className = `office-mailbox${room.mail.length ? " office-mailbox--full" : ""}`;
      const sealed = room.mail.some((m) => !m.mail.opened);
      box.innerHTML = mailboxSvg(room.mail.length === 0 ? null : sealed ? "sealed" : "open");
      if (room.mail.length > 1) {
        const n = document.createElement("span");
        n.className = "office-mailbox__count";
        n.textContent = String(room.mail.length);
        box.appendChild(n);
      }
      box.title = this.mailTitle(room);
      this.position(box, outside, 2);
      const target = room.mail[0]?.paneId ?? room.desks.find((d) => d.active)?.paneId;
      box.addEventListener("click", (ev) => {
        ev.stopPropagation();
        if (target) this.source.openPane(target);
      });
      out.push(box);
    }
    this.fixturesLayer.replaceChildren(...out);
  }

  private mailTitle(room: RoomModel): string {
    if (room.mail.length === 0) return t("office.mail.empty");
    const lines = room.mail.map(({ mail }) => {
      const who = mail.fromLabel ?? t("office.you");
      const state = t(mail.opened ? "office.mail.opened" : "office.mail.sealed");
      return `${t(`office.kind.${mail.kind}`)} · ${who} · ${state}${mail.snippet ? `\n  “${mail.snippet}”` : ""}`;
    });
    return `${fill(t("office.mail.title"), { n: room.mail.length })}\n${lines.join("\n")}`;
  }

  /// The newest notes pinned on the board, and the list behind it.
  private renderBoard(): void {
    const log = this.source.log();
    const notes = log.slice(0, 6).map((e, i) => {
      const n = document.createElement("i");
      n.className = `office-note office-note--${e.kind}`;
      n.style.setProperty("--i", String(i));
      return n;
    });
    this.boardNotes.replaceChildren(...notes);
    this.board.classList.toggle("office-board--fresh", log.length > 0 && Date.now() - log[0].at < 60_000);
    if (!this.boardPanel.hidden) this.fillBoardPanel();
  }

  private toggleBoardPanel(): void {
    this.boardPanel.hidden = !this.boardPanel.hidden;
    if (!this.boardPanel.hidden) this.fillBoardPanel();
  }

  private fillBoardPanel(): void {
    const log = this.source.log();
    const head = document.createElement("div");
    head.className = "office-board__head";
    head.textContent = t("office.board");
    const list = document.createElement("ol");
    list.className = "office-board__list";
    const rel = new Intl.RelativeTimeFormat(getLang(), { numeric: "auto", style: "short" });
    for (const e of log) {
      const li = document.createElement("li");
      const b = document.createElement("button");
      b.type = "button";
      b.className = `office-board__item office-board__item--${e.kind}`;
      const icon = document.createElement("span");
      icon.className = "office-board__icon";
      icon.innerHTML = iconSvg(e.kind === "done" ? "check" : e.kind === "review" ? "review" : e.kind === "commit" ? "box" : "chat");
      const text = document.createElement("span");
      text.className = "office-board__text";
      text.textContent = noteText(e);
      const when = document.createElement("time");
      when.className = "office-board__time";
      when.dateTime = new Date(e.at).toISOString();
      when.textContent = ago(rel, e.at);
      b.append(icon, text, when);
      b.addEventListener("click", (ev) => {
        ev.stopPropagation();
        this.source.openPane(e.paneId);
      });
      li.appendChild(b);
      list.appendChild(li);
    }
    if (log.length === 0) {
      const empty = document.createElement("p");
      empty.className = "office-board__empty";
      empty.textContent = t("office.board.empty");
      this.boardPanel.replaceChildren(head, empty);
    } else {
      this.boardPanel.replaceChildren(head, list);
    }
  }

  /// A lead just finished: a victory hop and a burst of confetti.
  private cheer(actor: Actor): void {
    if (this.reduced) return;
    actor.el.classList.add("office-char--cheering");
    const burst = document.createElement("div");
    burst.className = "office-confetti";
    this.position(burst, actor.pos, 30);
    const colours = ["#f5c542", "#e05555", "#5aa0d8", "#6cc06f", "#b48cf2", "#ff9f43"];
    for (let i = 0; i < 12; i++) {
      const bit = document.createElement("i");
      const angle = (Math.PI * 2 * i) / 12;
      bit.style.setProperty("--dx", `${Math.round(Math.cos(angle) * (26 + (i % 3) * 10))}px`);
      bit.style.setProperty("--dy", `${Math.round(Math.sin(angle) * 22 - 40)}px`);
      bit.style.background = colours[i % colours.length];
      burst.appendChild(bit);
    }
    this.actorsLayer.appendChild(burst);
    void this.sleep(2200).then(() => {
      actor.el.classList.remove("office-char--cheering");
      burst.remove();
    });
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
        if (actor.model) this.peekOrOpen(actor.model.paneId);
      });
    }
    this.actorsLayer.appendChild(el);
    const actor: Actor = {
      key,
      el,
      bubble,
      model,
      pos: MANAGER_HOME,
      chain: Promise.resolve(),
      busy: 0,
      gone: false,
      room: model?.roomId ?? null,
    };
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
    this.dress(actor, c);
    const status = t(`office.status.${c.status === "waiting" ? "waiting" : c.status}`);
    el.title = [c.label, status, c.tool].filter(Boolean).join(" · ");
    if (!el.classList.contains("office-char--speaking")) {
      const b = this.bubbleFor(c);
      this.setBubble(actor, b.icon, b.text);
    }
  }

  /// A lead wears its account's hero gear and level; redrawn only when the
  /// look changes (a level-up into a new tier, a new class).
  private dress(actor: Actor, c: CharModel): void {
    const el = actor.el;
    const look = c.hero ? `${c.hero.form}:${c.hero.cls}:${c.hero.tier}` : "";
    if (el.dataset.look !== look) {
      el.dataset.look = look;
      el.querySelector("svg.office-sprite")?.remove();
      el.insertAdjacentHTML("afterbegin", characterSvg(c.kind, c.role, c.hero));
    }
    el.classList.toggle("office-char--hat", !!c.hero && (c.hero.tier >= TIER.OUTFIT || c.hero.form === "human"));
    el.classList.toggle("office-char--human", c.hero?.form === "human");
    el.classList.toggle("office-char--aura", !!c.hero && c.hero.tier >= TIER.AURA);
    el.classList.toggle("office-char--legend", !!c.hero && c.hero.tier >= TIER.LEGEND);
    let badge = el.querySelector<HTMLElement>(".office-char__lv");
    if (c.hero) {
      if (!badge) {
        badge = document.createElement("span");
        badge.className = "office-char__lv";
        el.appendChild(badge);
      }
      badge.textContent = `Lv${c.hero.level}`;
      badge.title = `${this.accountName(this.source.accountOf(c.paneId))} · ${t(`office.class.${c.hero.cls}`)}`;
    } else {
      badge?.remove();
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

  /// Milliseconds to walk from `a` to `b` at the steady pace. A leg of a
  /// longer route has no minimum; a whole walk takes at least 350 ms.
  private walkTime(a: Point, b: Point, leg: boolean, pxPerS = WALK_PX_PER_S): number {
    const dx = (a.x - b.x) * this.floor.clientWidth;
    const dy = (a.y - b.y) * this.floor.clientHeight;
    const ms = (Math.hypot(dx, dy) / pxPerS) * 1000;
    return Math.round(leg ? Math.max(90, ms) : Math.min(5000, Math.max(350, ms)));
  }

  /// Walk to `to` at a steady pace; resolves on arrival. `leg` is one piece
  /// of a longer route: no minimum time, and the walk cycle keeps going.
  private walk(actor: Actor, to: Point, leg = false): Promise<void> {
    if (Math.abs(actor.pos.x - to.x) < 0.002 && Math.abs(actor.pos.y - to.y) < 0.002) {
      return Promise.resolve();
    }
    if (this.reduced) {
      this.place(actor, to);
      return Promise.resolve();
    }
    const ms = this.walkTime(actor.pos, to, leg);
    actor.el.classList.remove("office-char--seated");
    if (Math.abs(to.x - actor.pos.x) > 0.002) {
      actor.el.classList.toggle("office-char--face-left", to.x < actor.pos.x);
    }
    actor.el.classList.add("office-char--walking");
    actor.el.style.transition = `left ${ms}ms linear, top ${ms}ms linear`;
    // Force the transition to start from the current spot.
    void actor.el.offsetWidth;
    actor.pos = to;
    this.position(actor.el, to, 10);
    return this.sleep(ms).then(() => {
      if (leg) return;
      // Arrived: face the room again.
      actor.el.classList.remove("office-char--walking", "office-char--face-left");
      this.seat(actor);
    });
  }

  /// Walk to `to` the way people do: out of the door, along the corridors,
  /// in at the other door (straight across within one room).
  private async travel(actor: Actor, to: Place): Promise<void> {
    const path = planPath(this.model, { at: actor.pos, room: actor.room }, to);
    for (let i = 0; i < path.length; i++) {
      if (this.disposed) return;
      await this.walk(actor, path[i], i < path.length - 1);
    }
    actor.room = to.room;
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
        const home: Place = actor.model
          ? { at: actor.model.home, room: actor.model.roomId }
          : { at: MANAGER_HOME, room: null };
        await this.travel(actor, home);
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
      await this.travel(messenger, { at: spot, room: targetRoom.id });
      const told = targetLead && !targetLead.busy ? this.say(targetLead, "alert", BUBBLE_MS) : null;
      await this.say(messenger, icon);
      await told;
    });
  }
}

/// A board note's line.
function noteText(e: LogEntry): string {
  if (e.kind === "done") {
    return fill(t("office.note.done"), { agent: e.agent ? kindName(e.agent) : t("office.agent"), pane: e.label });
  }
  const kind = t(`office.kind.${e.kind}`);
  return e.fromLabel
    ? fill(t("office.note.fromTo"), { kind, from: e.fromLabel, pane: e.label })
    : fill(t("office.note.to"), { kind, pane: e.label });
}

function ago(rel: Intl.RelativeTimeFormat, at: number): string {
  const s = Math.round((at - Date.now()) / 1000);
  if (s > -60) return rel.format(Math.min(0, s), "second");
  const m = Math.round(s / 60);
  if (m > -60) return rel.format(m, "minute");
  return rel.format(Math.round(m / 60), "hour");
}

function clip(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}
