// The office view: the active workspace drawn as an office floor. Each pane
// (or tab group) is a room laid out like the split view, each tab a desk,
// each agent a character at its desk, each subagent a helper who walks in,
// works, reports back to its lead and leaves. Work one session sends to
// another (`officeEvents`) is carried over by a character on foot.
//
// It is also a control room: a queue of the agents waiting for the user
// (Alt+N / Tab walks it, a digit answers a menu and moves on), "approve all"
// for plain yes/no prompts, a done flag that opens the diff review, notes on
// the board with what each turn did, stuck agents sweating, the providers'
// quota as energy bars, a floor per workspace, and a mini mode in a corner.
// Busy agents run to the shelves, the machines and the window as they work.
//
// Rooms and desks are rebuilt on every update (they are cheap and static);
// characters are kept by key so they can walk between updates.

import type { PaneKind, Uuid } from "../types";
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
import { corkSvg, deskSvg, flagSvg, iconSvg, lobbyProps, mailboxSvg, paintProps, roomProps } from "./sprites";
import { characterSprite, paletteOf, petSprite } from "./sheet";
import { surfaceTile } from "./hiFloors";
import { tileSurface } from "./tiles";
import { petErrand, petsFor, type PetLobby, type PetSpot } from "./petModel";
import type { PetSpecies } from "./hiPets";
import type { LogEntry } from "./officeLog";
import { getLang } from "../i18n/i18n";
import { menuQuestion } from "./peekKeys";
import { kindName } from "./officeModel";
import { onOfficeMessage, type OfficeMessage } from "./officeEvents";
import { THEME_ICONS, THEME_IDS, loadFloorTheme, loadTheme, saveFloorTheme, type ThemeId } from "./themes";
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
import { loadTasks } from "./heroModel";
import { FunPanel, achievementName, type FunTab } from "./FunPanel";
import { funStore, loadFunSettings, saveFunSettings, type FunSettings } from "./funStore";
import { ACHIEVEMENTS, BOSS_COINS, BOSS_MS, achievementById, buildingOf, dayPart, pizzaTime, normalizeStats, questDef, reportDue, seasonOf, shopItem, type FunEvent } from "./funModel";
import { SHOP_PETS } from "./hiPets";
import { funProp, themePiece } from "./sprites";
import { askChoice, askConfirm, askText } from "../ui/Dialog";
import { chime, type Chime } from "./chiptune";
import { clampMini, loadMini, movedMini, resizedMini, saveMini, type MiniPlace } from "./miniPlace";
import {
  approveOption,
  errandIcon,
  errandSpot,
  nextWaiting,
  quotaMeters,
  shortDuration,
  stuckReason,
  waitQueue,
  wanderSpot,
  type Meter,
  type Stuck,
  type Waiting,
} from "./officeQueue";

/// Corridor width, in CSS pixels (wide enough for a character).
const CORRIDOR_PX = 40;
/// Walking pace, in CSS pixels per second, so a walk takes as long as its
/// on-screen distance whatever the window size.
const WALK_PX_PER_S = 240;

/// A workspace, as a floor of the office.
export interface OfficeFloor {
  id: number;
  label: string;
  /// Its number in the workspace panel (1-based).
  position: number;
  waiting: number;
  working: number;
}

/// Where the lobby's bought furniture stands, in order (left, as a share).
const LOBBY_SLOTS = [0.13, 0.57, 0.63, 0.69, 0.19, 0.75];
/// The office's own events and small talk: how often, in milliseconds.
const EVENT_MS = [150_000, 300_000] as const;
const CHAT_MS = [45_000, 110_000] as const;
const BREAK_MS = [60_000, 150_000] as const;
const CHAT_LINES = 8;

export interface OfficeSource extends PeekSource {
  /// Workspace `wsId`, as the model wants it.
  input(wsId: number): OfficeInput;
  workspaceName(wsId: number): string;
  /// Every workspace, and which one is active.
  floors(): OfficeFloor[];
  activeFloor(): number;
  /// The workspace a pane is in.
  floorOf(paneId: Uuid): number | null;
  /// Agents waiting for the user, in every workspace.
  waiting(): Waiting[];
  /// When a pane's terminal last printed something (epoch ms).
  outputAt(paneId: Uuid): number | null;
  /// When a pane's agent started on its current tool (epoch ms).
  toolSince(paneId: Uuid): number | null;
  /// When a pane's agent entered its current status (epoch ms).
  statusSince(paneId: Uuid): number | null;
  /// Tool calls in a pane's current working turn.
  toolCalls(paneId: Uuid): number;
  /// Open the diff review of the repository a pane works in.
  review(paneId: Uuid): Promise<"ok" | "no-repo">;
  /// The agents an empty desk can start, and starting one there.
  launchers(paneId: Uuid): Promise<{ label: string; command: string }[]>;
  launch(paneId: Uuid, command: string): void;
  /// The office shrank to a corner (true) or filled the window again.
  setMini(mini: boolean): void;
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
/// The control room's clock: wait times, stuck agents, errands.
const TICK_MS = 1000;
/// Pause at the desk between a busy agent's errands (plus up to as much again).
const ERRAND_GAP_MS = 3500;
/// A screen-pixel drag that turns a press on a board note into a drag.
const DRAG_PX = 6;

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
  private readonly lobby: HTMLElement;
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
  /// The workspace on show, and whether it follows the active one.
  private floorId: number;
  private followActive = true;
  private readonly floorsEl: HTMLElement;
  private readonly queueBtn: HTMLButtonElement;
  private readonly approveBtn: HTMLButtonElement;
  private readonly metersEl: HTMLElement;
  private readonly miniBtn: HTMLButtonElement;
  private readonly launchEl: HTMLElement;
  private meters: Meter[] = [];
  private waitingNow: Waiting[] = [];
  /// The peek was opened from the queue: answering moves on to the next.
  private queueMode = false;
  /// When each busy character takes its next errand (epoch ms).
  private readonly nextErrand = new Map<string, number>();
  private miniOn = false;
  private miniPlace: MiniPlace = loadMini();
  private decorating = false;
  private readonly decorateBar: HTMLElement;
  private readonly helpEl: HTMLElement;
  /// Rooms drawn, by id, with what they were drawn from.
  private roomCache = new Map<string, { sig: string; el: HTMLElement; desks: HTMLElement[] }>();
  private fixturesSig = "";
  private miniTicks = 0;
  // ── The games (`funModel`) ──
  private readonly fun: FunPanel;
  private funSettings: FunSettings = loadFunSettings();
  private readonly funBar: HTMLElement;
  private readonly bannerEl: HTMLElement;
  private readonly banners: string[] = [];
  private bannerBusy = false;
  private readonly buildingEl: HTMLElement;
  private readonly ambienceEl: HTMLElement;
  private readonly bosses = new Map<Uuid, HTMLElement>();
  private nextEvent = Date.now() + 40_000;
  private nextChat = Date.now() + 15_000;
  private readonly nextBreak = new Map<string, number>();
  private readonly reduced =
    typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  constructor(private readonly source: OfficeSource) {
    this.floorId = source.activeFloor();
    this.theme = loadFloorTheme(this.floorId, source.floors().find((f) => f.id === this.floorId)?.position ?? 1);
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
    // A floor per workspace, with a red count of who waits there.
    this.floorsEl = document.createElement("div");
    this.floorsEl.className = "office__floors";
    // The queue: who waits for the user, longest first.
    this.queueBtn = document.createElement("button");
    this.queueBtn.type = "button";
    this.queueBtn.className = "office__back office__queue";
    this.queueBtn.hidden = true;
    this.queueBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      this.nextInQueue();
    });
    this.approveBtn = document.createElement("button");
    this.approveBtn.type = "button";
    this.approveBtn.className = "office__back office__approve";
    this.approveBtn.hidden = true;
    this.approveBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      void this.approveAll();
    });
    // The providers' quota left, as energy bars.
    this.metersEl = document.createElement("div");
    this.metersEl.className = "office__meters";
    this.miniBtn = document.createElement("button");
    this.miniBtn.type = "button";
    this.miniBtn.className = "office__back office__mini-btn";
    this.miniBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      this.setMini(!this.miniOn);
    });
    const spacer = document.createElement("span");
    spacer.className = "office__spacer";
    const helpBtn = document.createElement("button");
    helpBtn.type = "button";
    helpBtn.className = "office__back office__mini-btn";
    helpBtn.textContent = "?";
    helpBtn.title = t("office.help");
    helpBtn.setAttribute("aria-label", t("office.help"));
    helpBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      this.helpEl.hidden = !this.helpEl.hidden;
    });
    head.append(this.titleEl, this.floorsEl, this.queueBtn, this.approveBtn, spacer, heroBtn, this.themeSelect, helpBtn, this.miniBtn, back);
    // A second, quieter row: the status legend, who is who, the quota left.
    const subhead = document.createElement("div");
    subhead.className = "office__subhead";
    // The games: coins, achievements, shop, quests, the paper, switches.
    this.funBar = document.createElement("div");
    this.funBar.className = "office__funbar";
    subhead.append(legend, this.kindsEl, this.metersEl, this.funBar);

    this.floor = document.createElement("div");
    this.floor.className = "office__floor";
    this.roomsLayer = document.createElement("div");
    this.roomsLayer.className = "office__rooms";
    this.actorsLayer = document.createElement("div");
    this.actorsLayer.className = "office__actors";
    const lobby = document.createElement("div");
    lobby.className = "office__lobby";
    this.lobby = lobby;
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
    paintProps(this.board);
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
    this.fun = new FunPanel({
      buy: (id) => {
        if (funStore.buy(id)) {
          this.play("coin");
          this.renderLobbyFun();
          this.scheduleRender();
        }
      },
      wear: (id) => funStore.setTitle(id),
      setSettings: (st) => {
        this.funSettings = st;
        saveFunSettings(st);
        this.applyAmbience();
        this.renderFunUi();
      },
      readReport: () => {
        this.record({ t: "count", what: "report" });
      },
      decorate: () => this.setDecorating(true),
      resetPlaces: () => {
        funStore.resetPlaces();
        this.renderLobbyFun();
        this.scheduleRender();
      },
      exportRecord: () => {
        void navigator.clipboard
          ?.writeText(JSON.stringify(funStore.get()))
          .then(() => this.banner(`📋 ${t("fun.exported")}`))
          .catch(() => undefined);
      },
      importRecord: () => void this.importRecord(),
      resetRecord: () => {
        void askConfirm(t("fun.resetConfirm"), t("fun.reset")).then((ok) => {
          if (!ok) return;
          funStore.replace(null);
          this.renderLobbyFun();
          this.scheduleRender();
        });
      },
    });
    this.helpEl = this.buildHelp();
    // Decorating: a bar over the floor while the furniture can be moved.
    this.decorateBar = document.createElement("div");
    this.decorateBar.className = "office-decorate";
    this.decorateBar.hidden = true;
    const decoText = document.createElement("span");
    decoText.textContent = `🖌 ${t("fun.decorating")}`;
    const decoDone = document.createElement("button");
    decoDone.type = "button";
    decoDone.className = "office-fun__btn";
    decoDone.textContent = t("fun.done");
    decoDone.addEventListener("click", (ev) => {
      ev.stopPropagation();
      this.setDecorating(false);
    });
    this.decorateBar.append(decoText, decoDone);
    this.bannerEl = document.createElement("div");
    this.bannerEl.className = "office-levelup office-banner";
    this.bannerEl.hidden = true;
    this.bannerEl.setAttribute("role", "status");
    this.buildingEl = document.createElement("span");
    this.buildingEl.className = "office__building";
    // Night, evening and the seasons' weather, over the floor.
    this.ambienceEl = document.createElement("div");
    this.ambienceEl.className = "office__ambience";
    this.ambienceEl.setAttribute("aria-hidden", "true");
    // An empty desk's menu: start an agent there, or look at its terminal.
    this.launchEl = document.createElement("div");
    this.launchEl.className = "office-launch";
    this.launchEl.hidden = true;
    this.launchEl.addEventListener("click", (ev) => ev.stopPropagation());
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
      this.launchEl,
      this.ambienceEl,
      this.fun.element,
      this.bannerEl,
      this.helpEl,
      this.decorateBar,
    );
    // While decorating, a press on bought furniture picks it up and other
    // clicks do nothing (no peeks, no walking off to a pane).
    this.floor.addEventListener(
      "pointerdown",
      (ev) => {
        if (!this.decorating || ev.button !== 0) return;
        const item = (ev.target as HTMLElement | null)?.closest<HTMLElement>(".fun-item");
        if (!item) return;
        ev.preventDefault();
        ev.stopPropagation();
        this.dragFurniture(item, ev);
      },
      true,
    );
    this.floor.addEventListener(
      "click",
      (ev) => {
        if (!this.decorating || (ev.target as HTMLElement | null)?.closest(".office-decorate")) return;
        ev.stopPropagation();
      },
      true,
    );
    lobby.appendChild(this.buildingEl);
    // Small in a corner: a click anywhere brings the office back first.
    this.floor.addEventListener(
      "click",
      (ev) => {
        if (!this.miniOn) return;
        ev.stopPropagation();
        this.setMini(false);
      },
      true,
    );
    this.floor.addEventListener("click", () => {
      this.boardPanel.hidden = true;
      this.launchEl.hidden = true;
      this.closePeek();
      this.heroes.hide();
      this.fun.hide();
      this.helpEl.hidden = true;
    });
    this.peek.onAnswered = (paneId) => this.answered(paneId);
    this.peek.onNext = () => this.nextInQueue();
    this.element.append(head, subhead, this.floor);
    this.wireMiniDrag(head);

    this.manager = this.makeActor("manager", null, characterSprite("", "manager"));
    this.manager.el.classList.add("office-char--manager", "office-char--relaxing");
    this.place(this.manager, MANAGER_HOME);
    this.manager.el.addEventListener("click", (ev) => ev.stopPropagation());

    this.element.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") {
        ev.preventDefault();
        if (this.miniOn) return;
        if (this.decorating) this.setDecorating(false);
        else if (!this.helpEl.hidden) this.helpEl.hidden = true;
        else if (!this.launchEl.hidden) this.launchEl.hidden = true;
        else if (this.fun.open) this.fun.hide();
        else if (this.peek.paneId) this.closePeek();
        else if (this.heroes.open) this.heroes.hide();
        else if (!this.boardPanel.hidden) this.boardPanel.hidden = true;
        else this.source.close();
        return;
      }
      // Tab or N on the floor itself (not in a field): the next agent waiting.
      const t = ev.target as HTMLElement | null;
      const typing = !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable || t.closest(".office-peek"));
      if (!typing && !ev.defaultPrevented && ev.key === "?") {
        ev.preventDefault();
        this.helpEl.hidden = !this.helpEl.hidden;
        return;
      }
      if (!typing && !ev.defaultPrevented && !ev.ctrlKey && !ev.metaKey && !ev.altKey && (ev.key === "Tab" || ev.key === "n" || ev.key === "N")) {
        if (this.waitingNow.length > 0) {
          ev.preventDefault();
          this.nextInQueue();
        }
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
    const tick = setInterval(() => this.tick(), TICK_MS);
    this.cleanups.push(() => clearInterval(tick));
    this.cleanups.push(funStore.onChange(() => this.onFun()));
    this.record({ t: "open", at: Date.now() });
    this.record({ t: "theme", id: this.theme });
    this.translate(back, legend);
    this.applyTheme();
    this.applyAmbience();
    this.renderFunUi();
    this.render();
    this.onFun();
    // Friday afternoon (or Monday, for last week): the paper comes by itself.
    const week = reportDue(funStore.get(), Date.now());
    if (week !== null && this.funSettings.banners) {
      funStore.markReport(week);
      this.fun.show("report");
    }
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
    this.queueBtn.title = t("office.queue.title");
    this.approveBtn.title = t("office.approveAll.title");
    this.miniBtn.textContent = "▣";
    this.miniBtn.title = t("office.mini.title");
    this.miniBtn.setAttribute("aria-label", t("office.mini.title"));
    this.floorsEl.setAttribute("aria-label", t("office.floors"));
    this.board.title = t("office.board");
    this.board.setAttribute("aria-label", t("office.board"));
  }

  /// Pick the place for the floor on show.
  private setTheme(id: ThemeId): void {
    if (id === this.theme) return;
    saveFloorTheme(this.floorId, id);
    this.record({ t: "theme", id });
    this.render();
  }

  /// The place a workspace is, by its id.
  private floorTheme(wsId: number): ThemeId {
    const f = this.source.floors().find((x) => x.id === wsId);
    return loadFloorTheme(wsId, f?.position ?? 1);
  }

  /// Dress the office as the floor on show's place, if it changed.
  private followTheme(): void {
    const id = this.floorTheme(this.floorId);
    if (id === this.theme) return;
    this.theme = id;
    this.themeSelect.value = id;
    this.record({ t: "theme", id });
    this.applyTheme();
  }

  /// The parts drawn once per theme: the floor's class, the lobby's
  /// furniture and name. Rooms and desks follow on the next render.
  private applyTheme(): void {
    for (const id of THEME_IDS) this.element.classList.toggle(`office--theme-${id}`, id === this.theme);
    this.lobbyFurniture.innerHTML = lobbyProps(this.theme);
    paintProps(this.lobbyFurniture);
    this.lobbyLabel.textContent = t(this.theme === "office" ? "office.lobby" : `office.lobby.${this.theme}`);
    this.renderLobbyFun();
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
    if (this.followActive) this.floorId = this.source.activeFloor();
    if (!this.source.floors().some((f) => f.id === this.floorId)) this.floorId = this.source.activeFloor();
    this.followTheme();
    const input = { ...this.source.input(this.floorId), heroOf: (id: Uuid) => this.heroOf(id) };
    const w = this.floor.clientWidth;
    const h = this.floor.clientHeight;
    if (w > 0 && h > 0) {
      // Corridors a fixed number of pixels wide, like everything drawn on
      // the floor: a bigger window means more room, not bigger pixels.
      this.model = buildOffice({ ...input, corridor: { x: CORRIDOR_PX / w, y: CORRIDOR_PX / h } });
    } else {
      this.model = buildOffice(input);
    }
    this.titleEl.textContent = `${t(`office.theme.${this.theme}`)} — ${this.source.workspaceName(this.floorId)}`;
    this.applyScale();
    this.renderFloors();
    this.renderKinds();
    this.renderRooms();
    this.reconcile();
    this.ensurePets();
    this.emptyEl.hidden = this.model.chars.length > 0;
    this.tick();
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

  /// Everything a room's elements are drawn from: when it is unchanged the
  /// room and its desks are kept as they are (no new DOM, no repaint).
  private roomSig(room: RoomModel, index: number): string {
    const r = room.rect;
    const n = (v: number) => v.toFixed(4);
    const st = funStore.get();
    return JSON.stringify([
      this.theme,
      index,
      room.kind,
      room.label,
      room.status,
      room.focused,
      [n(r.x), n(r.y), n(r.w), n(r.h)],
      room.desks.map((d) => [d.paneId, d.label, d.status, d.active, n(d.at.x), n(d.at.y), this.canPeek(d.paneId)]),
      st.owned.join(","),
      JSON.stringify(st.places ?? {}),
    ]);
  }

  /// Rooms and desks, rebuilt only where something changed.
  private renderRooms(): void {
    const keep = new Map<string, { sig: string; el: HTMLElement; desks: HTMLElement[] }>();
    const ordered: HTMLElement[] = [];
    this.model.rooms.forEach((room, index) => {
      const sig = this.roomSig(room, index);
      let cached = this.roomCache.get(room.id);
      if (!cached || cached.sig !== sig) {
        for (const d of cached?.desks ?? []) d.remove();
        cached = { sig, el: this.roomEl(room), desks: this.deskEls(room) };
      }
      keep.set(room.id, cached);
      ordered.push(cached.el);
    });
    for (const [id, c] of this.roomCache) if (!keep.has(id)) for (const d of c.desks) d.remove();
    this.roomCache = keep;
    const now = [...this.roomsLayer.children];
    if (now.length !== ordered.length || now.some((el, i) => el !== ordered[i])) this.roomsLayer.replaceChildren(...ordered);
    this.paintSurfaces();
    this.placePeek();
    this.renderFixtures();
    this.renderBoard();
  }

  private deskEls(room: RoomModel): HTMLElement[] {
    const out: HTMLElement[] = [];
    {
      for (const desk of room.desks) {
        const d = document.createElement("button");
        d.type = "button";
        d.className = `office-desk office-desk--${desk.status}${desk.active ? "" : " office-desk--back"}`;
        d.innerHTML = deskSvg(this.theme);
        paintProps(d);
        d.title = desk.label;
        // Above characters at the same height: a seated lead sits behind it.
        this.position(d, desk.at, 20);
        d.addEventListener("click", (ev) => {
          ev.stopPropagation();
          // A terminal with nobody at the desk: offer to start an agent.
          if (room.kind === "terminal" && !this.model.chars.some((c) => c.paneId === desk.paneId && c.role === "lead")) {
            void this.openLauncher(desk.paneId, desk.at);
          } else this.peekOrOpen(desk.paneId);
        });
        // Tabs at the same desk row get a name card under their desk.
        if (room.desks.length > 1) {
          const card = document.createElement("span");
          card.className = "office-desk__name";
          card.textContent = desk.label;
          d.appendChild(card);
        }
        this.actorsLayer.appendChild(d);
        out.push(d);
      }
    }
    return out;
  }

  /// The theme's floor and wall tiles under the corridors, the lobby and
  /// every room (the stylesheet's colours show until they are painted).
  private paintSurfaces(): void {
    tileSurface(this.floor, surfaceTile(this.theme, "corridor"));
    tileSurface(this.lobby, surfaceTile(this.theme, "lobby"));
    for (const room of this.roomsLayer.querySelectorAll<HTMLElement>(".office-room")) {
      const kind = (room.dataset.kind ?? "terminal") as PaneKind;
      const inner = room.querySelector<HTMLElement>(".office-room__inner");
      const wall = room.querySelector<HTMLElement>(".office-room__wall");
      if (inner) tileSurface(inner, surfaceTile(this.theme, "room", kind));
      if (wall) tileSurface(wall, surfaceTile(this.theme, "wall", kind), "hp-tiles--wall");
    }
  }

  private roomEl(room: RoomModel): HTMLElement {
    const el = document.createElement("div");
    el.className = `office-room office-room--${room.kind} office-room--${room.status}${room.focused ? " office-room--focused" : ""}`;
    el.style.left = `${room.rect.x * 100}%`;
    el.style.top = `${room.rect.y * 100}%`;
    el.style.width = `${room.rect.w * 100}%`;
    el.style.height = `${room.rect.h * 100}%`;
    el.title = room.label;
    el.dataset.kind = room.kind;
    el.dataset.room = room.id;
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
    props.innerHTML = roomProps(room.kind, this.theme) + this.roomFun(room);
    placeFurniture(props, funStore.get().places);
    paintProps(props);
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

  /// The office's keys and gestures, as a small pixel window (`?`).
  private buildHelp(): HTMLElement {
    const el = document.createElement("div");
    el.className = "office-heroes office-help";
    el.hidden = true;
    el.setAttribute("role", "dialog");
    el.setAttribute("aria-label", t("office.help"));
    el.addEventListener("click", (ev) => ev.stopPropagation());
    const frame = document.createElement("div");
    frame.className = "office-peek__frame";
    const head = document.createElement("div");
    head.className = "office-peek__head";
    const title = document.createElement("span");
    title.className = "office-peek__title";
    title.textContent = `⌨ ${t("office.help")}`;
    const close = document.createElement("button");
    close.type = "button";
    close.className = "office-peek__btn office-peek__close";
    close.textContent = "✕";
    close.setAttribute("aria-label", t("office.peek.close"));
    close.addEventListener("click", () => {
      el.hidden = true;
    });
    head.append(title, close);
    const list = document.createElement("dl");
    list.className = "office-help__list";
    for (const [keys, text] of [
      ["Tab / N", t("shortcut.officeNext")],
      ["Alt+N", t("shortcut.officePeekNext")],
      ["1 … 9", t("shortcut.officeAnswer")],
      ["Esc", t("office.help.esc")],
      [t("office.help.clickKey"), t("office.help.click")],
      [t("office.help.dblclickKey"), t("office.help.dblclick")],
      [t("office.help.dragKey"), t("office.help.drag")],
      ["Ctrl+Shift+O", t("shortcut.office")],
      ["?", t("shortcut.officeHelp")],
    ]) {
      const dt = document.createElement("dt");
      const kbd = document.createElement("kbd");
      kbd.textContent = keys;
      dt.appendChild(kbd);
      const dd = document.createElement("dd");
      dd.textContent = text;
      list.append(dt, dd);
    }
    frame.append(head, list);
    el.appendChild(frame);
    return el;
  }

  // ── Control room: floors, the queue, approve all, meters, mini ────────

  get mini(): boolean {
    return this.miniOn;
  }

  /// Shrink to a corner (the panes are usable again) or fill the window.
  setMini(on: boolean): void {
    if (on === this.miniOn) return;
    this.miniOn = on;
    this.element.classList.toggle("office--mini", on);
    this.applyMiniPlace();
    this.element.title = on ? t("fun.miniMove") : "";
    if (on) this.record({ t: "count", what: "mini" });
    if (on) {
      this.closePeek();
      this.launchEl.hidden = true;
      this.boardPanel.hidden = true;
      this.heroes.hide();
    }
    this.source.setMini(on);
    if (!on) this.focus();
  }

  /// Mini: drag the head to move it, the corner grip to resize it.
  private wireMiniDrag(head: HTMLElement): void {
    const grip = document.createElement("div");
    grip.className = "office__mini-grip";
    grip.title = t("fun.miniMove");
    this.element.appendChild(grip);
    const area = (): [number, number] => {
      const host = this.element.offsetParent as HTMLElement | null;
      return [host?.clientWidth ?? window.innerWidth, host?.clientHeight ?? window.innerHeight];
    };
    const drag = (ev: PointerEvent, step: (start: MiniPlace, dx: number, dy: number, w: number, h: number) => MiniPlace): void => {
      if (!this.miniOn || ev.button !== 0) return;
      const start = this.miniPlace;
      const [w, h] = area();
      const x0 = ev.clientX;
      const y0 = ev.clientY;
      let moved = false;
      const move = (e: PointerEvent): void => {
        const dx = e.clientX - x0;
        const dy = e.clientY - y0;
        if (!moved && Math.abs(dx) + Math.abs(dy) < 4) return;
        moved = true;
        this.miniPlace = step(start, dx, dy, w, h);
        this.applyMiniPlace();
      };
      const up = (): void => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        if (!moved) return;
        saveMini(this.miniPlace);
        // The click that ends a drag is not a "bring it back".
        window.addEventListener("click", (e) => e.stopPropagation(), { capture: true, once: true });
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
      ev.preventDefault();
    };
    head.addEventListener("pointerdown", (ev) => {
      if ((ev.target as HTMLElement | null)?.closest("button, select, input")) return;
      drag(ev, movedMini);
    });
    grip.addEventListener("pointerdown", (ev) => {
      ev.stopPropagation();
      drag(ev, resizedMini);
    });
  }

  private applyMiniPlace(): void {
    const s = this.element.style;
    if (!this.miniOn) {
      for (const k of ["--mini-r", "--mini-b", "--mini-scale"]) s.removeProperty(k);
      return;
    }
    const host = this.element.offsetParent as HTMLElement | null;
    this.miniPlace = clampMini(this.miniPlace, host?.clientWidth ?? window.innerWidth, host?.clientHeight ?? window.innerHeight);
    s.setProperty("--mini-r", `${this.miniPlace.right}px`);
    s.setProperty("--mini-b", `${this.miniPlace.bottom}px`);
    s.setProperty("--mini-scale", String(this.miniPlace.scale));
  }

  /// One button per workspace: its number, red with how many wait there.
  private renderFloors(): void {
    const floors = this.source.floors();
    this.floorsEl.hidden = floors.length < 2;
    if (floors.length < 2) return;
    this.floorsEl.replaceChildren(
      ...floors.map((f) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = `office__floor-btn${f.id === this.floorId ? " office__floor-btn--on" : ""}`;
        const place = this.floorTheme(f.id);
        b.textContent = `${THEME_ICONS[place]} ${f.position}F`;
        if (f.waiting > 0) {
          const n = document.createElement("span");
          n.className = "office__floor-wait";
          n.textContent = String(f.waiting);
          b.appendChild(n);
        } else if (f.working > 0) b.classList.add("office__floor-btn--busy");
        b.title = `${f.label} · ${t(`office.theme.${place}`)}${f.waiting ? ` · ${fill(t("office.queue"), { n: f.waiting })}` : ""}`;
        b.addEventListener("click", (ev) => {
          ev.stopPropagation();
          this.followActive = f.id === this.source.activeFloor();
          this.showFloor(f.id);
        });
        return b;
      }),
    );
  }

  /// Show workspace `id`: its rooms and people appear in place, no walking in.
  private showFloor(id: number): void {
    if (id === this.floorId) return;
    this.floorId = id;
    this.record({ t: "floor", id: String(id) });
    this.closePeek();
    this.launchEl.hidden = true;
    for (const a of this.actors.values()) a.el.remove();
    this.actors.clear();
    for (const b of this.bosses.values()) b.remove();
    this.bosses.clear();
    for (const c of this.roomCache.values()) for (const d of c.desks) d.remove();
    this.roomCache.clear();
    this.fixturesSig = "";
    this.nextErrand.clear();
    this.prev = [];
    this.first = true;
    this.render();
  }

  private closePeek(): void {
    this.queueMode = false;
    this.peek.close();
  }

  /// Open the next agent waiting for the user (on its own floor).
  private nextInQueue(): void {
    const queue = waitQueue(this.source.waiting());
    const next = nextWaiting(queue, this.peek.paneId);
    if (!next) {
      this.closePeek();
      return;
    }
    if (next.ws !== this.floorId) {
      this.followActive = next.ws === this.source.activeFloor();
      this.showFloor(next.ws);
    }
    this.queueMode = true;
    this.record({ t: "count", what: "queue" });
    if (this.canPeek(next.paneId)) this.peekOrOpen(next.paneId, true);
    else this.source.openPane(next.paneId);
    this.tick();
  }

  /// A menu was answered in the peek: in the queue, on to the next one.
  private answered(paneId: Uuid): void {
    const since = this.waitingNow.find((w) => w.paneId === paneId)?.since ?? null;
    this.record({ t: "answer", waitMs: since === null ? null : Date.now() - since });
    if (!this.queueMode) return;
    void this.sleep(700).then(() => {
      if (this.disposed || this.peek.paneId !== paneId) return;
      const rest = waitQueue(this.source.waiting()).filter((w) => w.paneId !== paneId);
      if (rest.length === 0) this.closePeek();
      else this.nextInQueue();
    });
  }

  /// Every waiting agent whose screen asks a plain yes/no, with its "yes".
  private approvable(): { w: Waiting; n: string; label: string }[] {
    const out: { w: Waiting; n: string; label: string }[] = [];
    for (const w of this.waitingNow) {
      const o = approveOption(this.source.screen(w.paneId));
      if (o) out.push({ w, n: o.n, label: o.label });
    }
    return out;
  }

  /// Answer "yes" in every waiting agent that only asks that, after the
  /// user confirms the list. Each screen is read again just before, so a
  /// prompt that changed meanwhile is left alone.
  private async approveAll(): Promise<void> {
    const list = this.approvable();
    if (list.length === 0) return;
    // What each one asks, so a yes is never given blind.
    const detail = list
      .map((a) => {
        const q = menuQuestion(this.source.screen(a.w.paneId) ?? []);
        return `• ${a.w.label} — ${a.n}. ${a.label}${q.length ? `\n    ${t("office.approveAll.asks")}: ${q.join(" / ")}` : ""}`;
      })
      .join("\n");
    const answer = await askChoice(fill(t("office.approveAll.confirm"), { n: list.length }), detail, [
      { id: "cancel", label: t("dialog.cancel") },
      { id: "ok", label: t("office.approveAll.ok"), primary: true },
    ]);
    if (answer?.id !== "ok" || this.disposed) return;
    let n = 0;
    for (const a of list) {
      const o = approveOption(this.source.screen(a.w.paneId));
      if (o && o.n === a.n && o.label === a.label) {
        this.source.type(a.w.paneId, o.n);
        n++;
      }
    }
    if (n > 0) this.record({ t: "approve", n });
    this.tick();
  }

  /// The once-a-second pass: the queue and approve buttons, wait times and
  /// stuck agents in the bubbles, and errands for the busy.
  private tick(): void {
    if (this.disposed) return;
    // Small in a corner: every third second is enough.
    if (this.miniOn && ++this.miniTicks % 3 !== 0) return;
    const now = Date.now();
    this.waitingNow = waitQueue(this.source.waiting());
    const n = this.waitingNow.length;
    this.queueBtn.hidden = n === 0;
    this.queueBtn.textContent = `⚠ ${fill(t("office.queue"), { n })}`;
    this.element.classList.toggle("office--alert", n > 0);
    const yes = n >= 2 ? this.approvable().length : 0;
    this.approveBtn.hidden = yes < 2;
    this.approveBtn.textContent = `✓ ${fill(t("office.approveAll"), { n: yes })}`;
    if (this.peek.paneId) {
      const w = this.waitingNow.find((x) => x.paneId === this.peek.paneId);
      this.peek.setWait(w?.since ? shortDuration(now - w.since, getLang()) : "");
      const others = this.waitingNow.filter((x) => x.paneId !== this.peek.paneId);
      this.peek.setNext(others.length > 0 ? (nextWaiting(this.waitingNow, this.peek.paneId)?.label ?? null) : null);
    }
    const since = new Map(this.waitingNow.map((w) => [w.paneId, w.since]));
    for (const actor of this.actors.values()) {
      const c = actor.model;
      if (!c || actor.gone) continue;
      const stuck =
        c.role === "lead"
          ? stuckReason({ working: c.status === "working", tool: c.tool, outputAt: this.source.outputAt(c.paneId), toolSince: this.source.toolSince(c.paneId), now })
          : null;
      actor.el.classList.toggle("office-char--stuck", stuck !== null);
      actor.el.dataset.stuck = stuck ?? "";
      if (!actor.el.classList.contains("office-char--speaking")) {
        const b = this.bubbleFor(c, since.get(c.paneId) ?? null, stuck, now);
        this.setBubble(actor, b.icon, b.text);
      }
      if (stuck) actor.el.title = `${c.label} · ${t(`office.stuck.${stuck}`)}`;
      this.busyErrand(actor, now);
    }
    this.funTick(now);
    if (now % 60_000 < TICK_MS) this.applyAmbience();
  }

  /// A busy agent runs to the shelf, the machines or the window now and
  /// then; a busy helper paces about its spot.
  private busyErrand(actor: Actor, now: number): void {
    const c = actor.model;
    if (!c || c.status !== "working" || actor.busy > 0 || this.reduced || this.miniOn) {
      if (c && c.status !== "working") this.nextErrand.delete(actor.key);
      return;
    }
    const due = this.nextErrand.get(actor.key);
    if (due === undefined) {
      this.nextErrand.set(actor.key, now + ERRAND_GAP_MS * (0.3 + Math.random()));
      return;
    }
    if (now < due) return;
    const room = this.roomOf(c);
    if (!room) return;
    const w = this.floor.clientWidth;
    const h = this.floor.clientHeight;
    this.nextErrand.set(actor.key, Number.POSITIVE_INFINITY);
    if (c.role === "lead") {
      const spot = errandSpot(room.rect, w, h, Math.random);
      const icon = errandIcon(c.tool);
      this.errand(actor, async () => {
        await this.walk(actor, spot);
        actor.el.classList.add("office-char--looking");
        await this.say(actor, icon, 1200 + Math.random() * 1400);
        actor.el.classList.remove("office-char--looking");
      });
    } else {
      const spot = wanderSpot(c.home, room.rect, w, h, Math.random);
      this.errand(actor, async () => {
        await this.walk(actor, spot);
        await this.sleep(500 + Math.random() * 900);
      });
    }
    // Next errand once this one is over and it has sat a while.
    void actor.chain.then(() => this.nextErrand.set(actor.key, Date.now() + ERRAND_GAP_MS * (1 + Math.random())));
  }

  /// The providers' quota left, as small energy bars in the header.
  private renderMeters(): void {
    this.metersEl.replaceChildren(
      ...this.meters.map((m) => {
        const el = document.createElement("span");
        el.className = `office__meter${m.left <= 15 ? " office__meter--low" : m.left <= 40 ? " office__meter--mid" : ""}`;
        el.title = fill(t("office.quota"), { provider: kindName(m.provider), window: m.window, n: m.left });
        const name = document.createElement("span");
        name.className = "office__meter-name";
        name.textContent = `${kindName(m.provider).split(" ")[0]} ${m.window}`;
        const bar = document.createElement("span");
        bar.className = "office__meter-bar";
        const fillEl = document.createElement("i");
        fillEl.style.width = `${m.left}%`;
        bar.appendChild(fillEl);
        el.append(name, bar);
        return el;
      }),
    );
  }

  /// An agentless terminal's desk: start an agent there, or peek.
  private async openLauncher(paneId: Uuid, at: Point): Promise<void> {
    const head = document.createElement("div");
    head.className = "office-launch__head";
    head.textContent = t("office.launch.title");
    const peekBtn = document.createElement("button");
    peekBtn.type = "button";
    peekBtn.className = "office-launch__item";
    peekBtn.textContent = `👁 ${t("office.launch.peek")}`;
    peekBtn.addEventListener("click", () => {
      this.launchEl.hidden = true;
      this.peekOrOpen(paneId);
    });
    const wait = document.createElement("div");
    wait.className = "office-launch__note";
    wait.textContent = "…";
    this.launchEl.replaceChildren(head, wait, peekBtn);
    this.launchEl.style.left = `${at.x * 100}%`;
    this.launchEl.style.top = `${at.y * 100}%`;
    this.launchEl.hidden = false;
    const agents = await this.source.launchers(paneId).catch(() => []);
    if (this.disposed || this.launchEl.hidden) return;
    const items = agents.map((a) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "office-launch__item";
      b.textContent = `▶ ${a.label}`;
      b.title = a.command;
      b.addEventListener("click", () => {
        this.launchEl.hidden = true;
        this.source.launch(paneId, a.command);
        this.record({ t: "count", what: "launch" });
      });
      return b;
    });
    if (items.length === 0) {
      wait.textContent = t("office.launch.none");
      this.launchEl.replaceChildren(head, wait, peekBtn);
    } else this.launchEl.replaceChildren(head, ...items, peekBtn);
  }

  /// A board note dropped on an agent: its peek opens with a prompt handing
  /// the note over, for the user to edit and send.
  private handoff(e: LogEntry, paneId: Uuid): void {
    if (!this.canPeek(paneId)) return;
    const text =
      e.kind === "done"
        ? fill(t("office.handoff.review"), { pane: e.label, what: e.turn?.headline || t("office.handoff.lastTurn") })
        : fill(t("office.handoff.message"), { pane: e.label, text: e.snippet });
    this.peekOrOpen(paneId);
    this.peek.prefill(text);
    this.record({ t: "count", what: "handoff" });
  }

  /// Drag a board note with the pointer (HTML drag and drop is taken by the
  /// window's file drop on Windows); drop it on a character.
  private dragNote(e: LogEntry, down: PointerEvent, src: HTMLElement): void {
    const x0 = down.clientX;
    const y0 = down.clientY;
    let ghost: HTMLElement | null = null;
    let over: HTMLElement | null = null;
    const target = (ev: PointerEvent): HTMLElement | null => {
      const hit = document.elementFromPoint(ev.clientX, ev.clientY);
      const el = hit?.closest<HTMLElement>(".office-char");
      return el && el.dataset.pane ? el : null;
    };
    const move = (ev: PointerEvent) => {
      if (!ghost) {
        if (Math.hypot(ev.clientX - x0, ev.clientY - y0) < DRAG_PX) return;
        ghost = document.createElement("div");
        ghost.className = "office-note-ghost";
        ghost.textContent = src.querySelector(".office-board__text")?.textContent ?? "";
        this.element.appendChild(ghost);
        this.element.classList.add("office--dragging");
      }
      const r = this.element.getBoundingClientRect();
      ghost.style.left = `${ev.clientX - r.left + 12}px`;
      ghost.style.top = `${ev.clientY - r.top + 12}px`;
      ghost.hidden = true;
      const t2 = target(ev);
      ghost.hidden = false;
      if (t2 !== over) {
        over?.classList.remove("office-char--drop");
        t2?.classList.add("office-char--drop");
        over = t2;
      }
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      over?.classList.remove("office-char--drop");
      this.element.classList.remove("office--dragging");
      if (!ghost) {
        this.source.openPane(e.paneId);
        return;
      }
      ghost.remove();
      // The click that ends a drag lands on the floor, which would close the
      // peek this opens: swallow it.
      const swallow = (c: MouseEvent) => {
        c.stopPropagation();
        c.preventDefault();
      };
      window.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
      const t2 = target(ev);
      if (t2?.dataset.pane) {
        this.boardPanel.hidden = true;
        this.handoff(e, t2.dataset.pane);
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  // ── The games: achievements, coins, the shop, quests, the paper ───────

  private record(ev: FunEvent): void {
    funStore.record(ev);
  }

  /// The store changed: new achievements and quests to celebrate, bosses
  /// beaten, the coins and the panel.
  private onFun(): void {
    if (this.disposed) return;
    const news = funStore.takeNews();
    if (this.funSettings.banners) {
      // A few at a time; a pile (after a while away) is one banner.
      if (news.unlocked.length > 2) {
        const coins = news.unlocked.reduce((n, a) => n + a.reward, 0);
        this.banner(`🏆 ${fill(t("fun.unlockedMany"), { n: news.unlocked.length, coins })}`);
      } else for (const a of news.unlocked) this.banner(`🏆 ${fill(t("fun.unlocked"), { name: achievementName(a) })} · ${fill(t("fun.reward"), { n: a.reward })}`);
      for (const id of news.quests.slice(-2)) {
        const q = questDef(id);
        this.banner(`📜 ${fill(t("fun.questDone"), { name: q ? fill(t(`fun.quest.${id}`), { n: q.target }) : id })}`);
      }
    }
    for (let i = 0; i < news.bosses; i++) this.banner(`👹 ${fill(t("fun.bossDefeated"), { n: BOSS_COINS })}`);
    if (news.unlocked.length > 0 && !this.reduced) this.cheer(this.manager);
    // One tune per change, the biggest news first.
    if (news.unlocked.length > 0) this.play("unlock");
    else if (news.bosses > 0) this.play("boss");
    else if (news.quests.length > 0) this.play("quest");
    for (const id of news.used) this.useItem(id);
    this.renderFunUi();
  }

  /// Paste a record copied out with export.
  private async importRecord(): Promise<void> {
    const text = await askText(t("fun.importPrompt"));
    if (text === null || this.disposed) return;
    let raw: unknown;
    try {
      raw = JSON.parse(text);
    } catch {
      raw = null;
    }
    if (!isRecord(raw)) {
      this.banner(`⚠ ${t("fun.importBad")}`);
      return;
    }
    funStore.replace(normalizeStats(raw));
    this.renderLobbyFun();
    this.scheduleRender();
  }

  /// A little tune, when the sounds are on.
  private play(kind: Chime): void {
    if (this.funSettings.sound && !this.miniOn) chime(kind);
  }

  /// A consumable was bought: the floor shows it.
  private useItem(id: string): void {
    if (this.reduced) return;
    if (id === "snacks") {
      for (const a of this.actors.values()) if (a.model?.role === "lead") this.popOver(a.el, "💗");
    } else if (id === "party") {
      this.play("party");
      this.paperRain(36);
      this.cheer(this.manager);
      for (const a of this.actors.values()) if (a.model?.role === "lead" && a.model.status !== "working" && a.busy === 0) this.cheer(a);
    } else if (id === "fireworks") {
      this.play("party");
      this.fireworks();
    }
  }

  /// A short-lived emoji rising over a character.
  private popOver(host: HTMLElement, glyph: string): void {
    const el = document.createElement("span");
    el.className = "office-pop";
    el.textContent = glyph;
    host.appendChild(el);
    void this.sleep(1600).then(() => el.remove());
  }

  /// Bits of paper falling over the floor.
  private paperRain(n: number): void {
    const layer = document.createElement("div");
    layer.className = "office-rain";
    for (let i = 0; i < n; i++) {
      const bit = document.createElement("i");
      bit.style.left = `${Math.random() * 100}%`;
      bit.style.animationDelay = `${Math.random() * 0.8}s`;
      bit.style.setProperty("--hue", `${Math.floor(Math.random() * 360)}`);
      layer.appendChild(bit);
    }
    this.floor.appendChild(layer);
    void this.sleep(3200).then(() => layer.remove());
  }

  /// Bursts in the sky over the floor.
  private fireworks(): void {
    const layer = document.createElement("div");
    layer.className = "office-fireworks";
    for (let i = 0; i < 6; i++) {
      const burst = document.createElement("span");
      burst.className = "office-firework";
      burst.style.left = `${10 + Math.random() * 80}%`;
      burst.style.top = `${8 + Math.random() * 30}%`;
      burst.style.animationDelay = `${i * 0.35}s`;
      burst.style.setProperty("--hue", `${Math.floor(Math.random() * 360)}`);
      layer.appendChild(burst);
    }
    this.floor.appendChild(layer);
    void this.sleep(3600).then(() => layer.remove());
  }

  /// A banner over the floor; several wait their turn.
  private banner(text: string): void {
    this.banners.push(text);
    if (this.bannerBusy) return;
    this.bannerBusy = true;
    const next = (): void => {
      const msg = this.banners.shift();
      if (msg === undefined || this.disposed) {
        this.bannerBusy = false;
        this.bannerEl.hidden = true;
        return;
      }
      this.bannerEl.textContent = msg;
      this.bannerEl.hidden = false;
      // Restart the pop-in animation.
      this.bannerEl.style.animation = "none";
      void this.bannerEl.offsetWidth;
      this.bannerEl.style.animation = "";
      void this.sleep(2600).then(next);
    };
    next();
  }

  /// The games' row in the header, the panel, the building's sign and the
  /// user's title.
  private renderFunUi(): void {
    const st = funStore.get();
    const button = (tab: FunTab, label: string, title: string): HTMLButtonElement => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "office__fun-btn";
      b.textContent = label;
      b.title = title;
      b.addEventListener("click", (ev) => {
        ev.stopPropagation();
        if (this.fun.open && this.fun.current === tab) {
          this.fun.hide();
          return;
        }
        this.closePeek();
        this.heroes.hide();
        this.boardPanel.hidden = true;
        this.fun.show(tab);
      });
      return b;
    };
    const today = st.quests.done.length;
    const fmt = new Intl.NumberFormat(getLang());
    this.funBar.replaceChildren(
      button("shop", `🪙 ${fmt.format(st.coins)}`, t("fun.shop")),
      button("achievements", `🏆 ${Object.keys(st.unlocked).length}/${ACHIEVEMENTS.length}`, t("fun.achievements")),
      button("quests", `📜 ${today}/3`, t("fun.quests")),
      button("report", "📰", t("fun.report")),
      button("settings", "⚙", t("fun.settings")),
    );
    this.fun.render(st, this.funSettings);
    const b = buildingOf(st.counters.turns ?? 0);
    const name = t(`fun.building.${b.name}`);
    this.buildingEl.textContent = `🏢 ${name} ${"★".repeat(b.step)}`;
    this.buildingEl.title = b.next === null ? fill(t("fun.building.max"), { name }) : fill(t("fun.building.title"), { name, n: b.next - (st.counters.turns ?? 0) });
    // The user wears their title.
    let tag = this.manager.el.querySelector<HTMLElement>(".office-char__title");
    const worn = st.title ? achievementById(st.title) : undefined;
    if (worn) {
      if (!tag) {
        tag = document.createElement("span");
        tag.className = "office-char__title";
        this.manager.el.appendChild(tag);
      }
      tag.textContent = `${worn.family.icon} ${achievementName(worn)}`;
    } else tag?.remove();
  }

  /// The lobby's bought furniture, and the season's and Friday's props.
  private renderLobbyFun(): void {
    for (const old of this.lobbyFurniture.querySelectorAll(".office-prop--fun")) old.remove();
    const st = funStore.get();
    // Bought furniture: where the user put it in the lobby, else the lobby
    // pieces in their slots.
    const placed = st.owned.filter((id) => st.places[id]?.room === null);
    const slotted = st.owned.filter((id) => shopItem(id)?.place === "lobby" && !st.places[id]);
    let html =
      placed.map((id) => funProp(id, `office-prop office-prop--fun office-prop--placed fun-item fun-item--${id}`)).join("") +
      slotted
        .slice(0, LOBBY_SLOTS.length)
        .map((id) => funProp(id, `office-prop office-prop--fun office-prop--slot fun-item fun-item--${id}`))
        .join("");
    const now = Date.now();
    if (this.funSettings.ambience) {
      const season = seasonOf(now);
      if (season === "xmas") html += themePiece("snow", (th) => th.lobby.centre, "office-prop office-prop--fun office-prop--fun-tree");
      if (season === "halloween") html += funProp("pumpkin", "office-prop office-prop--fun office-prop--fun-pumpkin-l") + funProp("pumpkin", "office-prop office-prop--fun office-prop--fun-pumpkin-r");
    }
    if (pizzaTime(now)) html += funProp("pizza", "office-prop office-prop--fun office-prop--fun-pizza");
    this.lobbyFurniture.insertAdjacentHTML("beforeend", html);
    this.lobbyFurniture.querySelectorAll<HTMLElement>(".office-prop--slot").forEach((el, i) => {
      el.style.left = `${LOBBY_SLOTS[i] * 100}%`;
    });
    placeFurniture(this.lobbyFurniture, st.places);
    paintProps(this.lobbyFurniture);
  }

  /// A room's share of the bought furniture: one floor piece in a corner,
  /// and the wall pieces along its back wall (spread over the rooms).
  private roomFun(room: RoomModel): string {
    const st = funStore.get();
    const i = this.model.rooms.findIndex((r) => r.id === room.id);
    // Where the user put things in this room; the rest goes one per room
    // (never the same piece twice).
    const placed = st.owned.filter((id) => st.places[id]?.room === room.id);
    const floor = st.owned.filter((id) => shopItem(id)?.place === "room" && !st.places[id]);
    const wall = st.owned.filter((id) => shopItem(id)?.place === "wall" && !st.places[id]);
    let html = placed.map((id) => funProp(id, `office-prop office-prop--placed fun-item fun-item--${id}`)).join("");
    if (i >= 0 && i < floor.length) html += funProp(floor[i], `office-prop office-prop--deco fun-item fun-item--${floor[i]}`);
    if (i >= 0 && i < wall.length) html += `<div class="office-wall-row office-wall-row--fun">${funProp(wall[i], `office-prop fun-item fun-item--${wall[i]}`)}</div>`;
    return html;
  }

  private setDecorating(on: boolean): void {
    this.decorating = on;
    this.element.classList.toggle("office--decorating", on);
    this.decorateBar.hidden = !on;
    if (on) {
      this.fun.hide();
      this.closePeek();
      this.boardPanel.hidden = true;
    }
  }

  /// Carry a piece of furniture with the pointer and set it down in the room
  /// (or the lobby) under it.
  private dragFurniture(item: HTMLElement, down: PointerEvent): void {
    const id = [...item.classList].find((c) => c.startsWith("fun-item--"))?.slice("fun-item--".length);
    if (!id) return;
    const box = item.getBoundingClientRect();
    const dx = down.clientX - box.left;
    const dy = down.clientY - box.top;
    // A copy follows the pointer over the whole office (rooms clip their
    // own contents); the canvases are copied by hand, cloning leaves them blank.
    const ghost = item.cloneNode(true) as HTMLElement;
    const from = item.querySelectorAll("canvas");
    ghost.querySelectorAll("canvas").forEach((c, i) => c.getContext("2d")?.drawImage(from[i], 0, 0));
    ghost.classList.add("fun-item--lifted");
    const host = this.element.getBoundingClientRect();
    ghost.style.position = "absolute";
    ghost.style.right = ghost.style.bottom = "auto";
    const follow = (x: number, y: number) => {
      ghost.style.left = `${x - dx - host.left}px`;
      ghost.style.top = `${y - dy - host.top}px`;
    };
    follow(down.clientX, down.clientY);
    this.element.appendChild(ghost);
    item.style.visibility = "hidden";
    const move = (ev: PointerEvent) => follow(ev.clientX, ev.clientY);
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      ghost.remove();
      item.style.visibility = "";
      const under = document.elementFromPoint(ev.clientX, ev.clientY);
      const roomEl = under?.closest<HTMLElement>(".office-room");
      const area = roomEl ? roomEl.querySelector<HTMLElement>(".office-room__props") : under?.closest(".office__lobby") ? this.lobbyFurniture : null;
      if (!area) return;
      const r = area.getBoundingClientRect();
      const clamp = (v: number) => Math.max(0, Math.min(0.95, v));
      funStore.setPlace(id, {
        room: roomEl ? (roomEl.dataset.room ?? null) : null,
        x: clamp((ev.clientX - dx - r.left) / Math.max(1, r.width)),
        y: clamp((ev.clientY - dy - r.top) / Math.max(1, r.height)),
      });
      this.renderLobbyFun();
      this.render();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  /// Night, evening and the season, as classes the stylesheet dresses.
  private applyAmbience(): void {
    const now = Date.now();
    const on = this.funSettings.ambience;
    const part = on ? dayPart(now) : "day";
    this.element.classList.toggle("office--night", part === "night");
    this.element.classList.toggle("office--evening", part === "evening");
    const season = on ? seasonOf(now) : null;
    for (const s of ["xmas", "halloween", "newyear", "spring"]) this.element.classList.toggle(`office--season-${s}`, season === s);
    this.ambienceEl.textContent = season === "newyear" ? `🎉 ${new Date(now).getFullYear()} 🎉` : "";
  }

  /// The once-a-second life of the office: bosses over long tasks, the
  /// office's events, small talk and breaks.
  private funTick(now: number): void {
    this.bossTick(now);
    if (!this.funSettings.life || this.reduced || this.miniOn) return;
    if (now >= this.nextEvent) {
      this.nextEvent = now + EVENT_MS[0] + Math.random() * (EVENT_MS[1] - EVENT_MS[0]);
      this.officeEvent(now);
    }
    if (now >= this.nextChat) {
      this.nextChat = now + CHAT_MS[0] + Math.random() * (CHAT_MS[1] - CHAT_MS[0]);
      this.smallTalk();
    }
    for (const actor of this.actors.values()) this.takeBreak(actor, now);
  }

  /// A lead working longer than `BOSS_MS` has a boss in its room, losing
  /// health as the time goes; the turn finishing defeats it.
  private bossTick(now: number): void {
    const alive = new Set<Uuid>();
    for (const actor of this.actors.values()) {
      const c = actor.model;
      if (!c || c.role !== "lead" || c.status !== "working" || actor.gone) continue;
      const since = this.source.statusSince(c.paneId);
      if (since === null || now - since < BOSS_MS) continue;
      const room = this.roomOf(c);
      if (!room) continue;
      alive.add(c.paneId);
      let el = this.bosses.get(c.paneId);
      if (!el) {
        el = document.createElement("div");
        el.className = "office-boss";
        el.appendChild(characterSprite("boss", "lead"));
        const hp = document.createElement("span");
        hp.className = "office-boss__hp";
        hp.appendChild(document.createElement("i"));
        el.appendChild(hp);
        this.actorsLayer.appendChild(el);
        this.bosses.set(c.paneId, el);
      }
      this.position(el, { x: room.rect.x + room.rect.w * 0.82, y: room.rect.y + room.rect.h * 0.62 }, 15);
      // Every tool call the agent makes is a blow: the boss loses health
      // with the work done, not with the clock.
      const hits = this.source.toolCalls(c.paneId);
      const left = Math.max(0.05, 1 - hits * 0.04);
      el.querySelector<HTMLElement>(".office-boss__hp i")!.style.width = `${Math.round(left * 100)}%`;
      if (Number(el.dataset.hits ?? hits) < hits && !this.reduced) {
        el.classList.remove("office-boss--hit");
        void el.offsetWidth;
        el.classList.add("office-boss--hit");
      }
      el.dataset.hits = String(hits);
      el.title = fill(t("fun.boss"), { t: shortDuration(now - since, getLang()) });
    }
    for (const [pane, el] of this.bosses) {
      if (alive.has(pane)) continue;
      this.bosses.delete(pane);
      const done = this.model.chars.some((c) => c.paneId === pane && c.role === "lead" && c.status === "done");
      if (done && !this.reduced) {
        el.classList.add("office-boss--defeated");
        void this.sleep(1200).then(() => el.remove());
      } else el.remove();
    }
  }

  /// Leads free for a chat: not on an errand, not waiting for the user.
  private freeLeads(): Actor[] {
    return [...this.actors.values()].filter((a) => a.model?.role === "lead" && !a.busy && !a.gone && a.model.status !== "waiting");
  }

  /// One lead walks over to another for a word.
  private smallTalk(): void {
    const free = this.freeLeads().filter((a) => a.model?.status !== "working");
    if (free.length < 2) return;
    const a = free[Math.floor(Math.random() * free.length)];
    const others = free.filter((x) => x !== a);
    const b = others[Math.floor(Math.random() * others.length)];
    const room = b.model ? this.roomOf(b.model) : undefined;
    if (!room || !b.model) return;
    this.record({ t: "count", what: "chat" });
    const aLine = this.talkLine(a, b);
    const bLine = this.talkLine(b, a);
    this.errand(a, async () => {
      await this.travel(a, { at: besideSpot(b.model!.home, room.rect), room: room.id });
      await this.sayText(a, "chat", aLine, 2400);
      if (!b.busy) await this.sayText(b, "chat", bLine, 2400);
    });
  }

  /// What `who` says to `to`: about its own last turn, the other's room, its
  /// tool or tests when the office knows them, else a pleasantry.
  private talkLine(who: Actor, to: Actor): string {
    const log = this.source.log();
    const mine = who.model ? log.find((e) => e.kind === "done" && e.paneId === who.model!.paneId)?.turn : undefined;
    const theirs = to.model ? log.find((e) => e.kind === "done" && e.paneId === to.model!.paneId)?.turn : undefined;
    const pane = to.model ? (this.model.rooms.find((r) => r.id === to.model!.roomId)?.label ?? to.model.label) : "";
    const lines: string[] = [];
    if (mine?.headline) lines.push(fill(t("fun.talk.done"), { what: clip(mine.headline, 40) }));
    if (mine && (mine.add ?? 0) + (mine.del ?? 0) > 0) lines.push(fill(t("fun.talk.lines"), { n: (mine.add ?? 0) + (mine.del ?? 0) }));
    if (to.model?.status === "working" && to.model.tool) lines.push(fill(t("fun.talk.tool"), { tool: clip(to.model.tool, 16) }));
    if (theirs?.test === true) lines.push(fill(t("fun.talk.pass"), { pane: clip(pane, 18) }));
    if (theirs?.test === false) lines.push(fill(t("fun.talk.fail"), { pane: clip(pane, 18) }));
    if (pane) lines.push(fill(t("fun.talk.ask"), { pane: clip(pane, 18) }));
    // Now and then just small talk, so it is not all shop.
    if (lines.length === 0 || Math.random() < 0.3) return t(`fun.chat.${1 + Math.floor(Math.random() * CHAT_LINES)}`);
    return lines[Math.floor(Math.random() * lines.length)];
  }

  /// A finished or idle lead now and then stretches its legs: coffee in the
  /// lobby, or a pat for one of the animals.
  private takeBreak(actor: Actor, now: number): void {
    const c = actor.model;
    if (!c || c.role !== "lead" || actor.busy || actor.gone || (c.status !== "done" && c.status !== "idle")) {
      this.nextBreak.delete(actor.key);
      return;
    }
    const due = this.nextBreak.get(actor.key);
    if (due === undefined) {
      this.nextBreak.set(actor.key, now + BREAK_MS[0] * Math.random() + 20_000);
      return;
    }
    if (now < due) return;
    this.nextBreak.set(actor.key, now + BREAK_MS[0] + Math.random() * (BREAK_MS[1] - BREAK_MS[0]));
    // Only a pet in the lobby: the corridors are no place to stop for a pat.
    const lobbyPets = this.pets.filter((p) => p.spot.lobby);
    const pet = lobbyPets.length > 0 && Math.random() < 0.4 ? lobbyPets[Math.floor(Math.random() * lobbyPets.length)] : null;
    const coffee = { x: 0.8, y: 1 - LOBBY_H * 0.3 };
    this.errand(actor, async () => {
      if (pet) {
        await this.travel(actor, { at: pet.spot.at, room: null });
        this.petBubble(pet, "heart", 1500);
        await this.say(actor, "heart", 1500);
        this.record({ t: "count", what: "pet" });
      } else {
        await this.travel(actor, { at: coffee, room: null });
        await this.say(actor, "coffee", 2200);
      }
    });
  }

  /// Now and then something happens in the office.
  private officeEvent(now: number): void {
    const night = dayPart(now) === "night";
    const kinds = ["courier", "inspection", ...(pizzaTime(now) || Math.random() < 0.2 ? ["pizza"] : []), ...(night ? ["star"] : [])];
    const kind = kinds[Math.floor(Math.random() * kinds.length)];
    this.record({ t: "event", kind });
    if (kind === "star") {
      const star = document.createElement("i");
      star.className = "office-star";
      star.style.top = `${4 + Math.random() * 30}%`;
      this.floor.appendChild(star);
      void this.sleep(1600).then(() => star.remove());
      return;
    }
    if (kind === "inspection") {
      // The user's character makes the rounds.
      const rooms = [...this.model.rooms].sort(() => Math.random() - 0.5).slice(0, 2);
      this.errand(this.manager, async () => {
        for (const r of rooms) {
          await this.travel(this.manager, { at: r.door, room: r.id });
          await this.say(this.manager, "heart", 900);
        }
      });
      return;
    }
    // A courier walks in from the lobby with a parcel (or the pizza).
    const courier = this.makeActor(`courier:${now}`, null, characterSprite("courier", "lead"));
    courier.el.classList.add("office-char--courier");
    courier.el.title = t(`fun.event.${kind}`);
    this.place(courier, { x: 0.04, y: 1 - LOBBY_H * 0.3 });
    const room = this.model.rooms[Math.floor(Math.random() * this.model.rooms.length)];
    const icon = kind === "pizza" ? "heart" : "box";
    this.errand(courier, async () => {
      if (kind === "pizza" || !room) {
        await this.walk(courier, { x: 0.42, y: 1 - LOBBY_H * 0.3 });
        await this.sayText(courier, icon, t(`fun.event.${kind}`), 2400);
        // Whoever is free comes down for a slice.
        for (const a of this.freeLeads().filter((x) => x.model?.status !== "working").slice(0, 3)) {
          this.errand(a, async () => {
            await this.travel(a, { at: { x: 0.36 + Math.random() * 0.12, y: 1 - LOBBY_H * 0.25 }, room: null });
            await this.say(a, "heart", 2000);
          });
        }
      } else {
        await this.travel(courier, { at: room.door, room: room.id });
        await this.sayText(courier, icon, t(`fun.event.${kind}`), 2000);
        await this.travel(courier, { at: { x: 0.04, y: 1 - LOBBY_H * 0.3 }, room: null });
      }
      await this.fadeOut(courier);
      courier.gone = true;
      courier.el.remove();
    });
  }

  /// A bubble with an icon and a line of text, for `ms`.
  private async sayText(actor: Actor, icon: string, text: string, ms: number): Promise<void> {
    actor.el.classList.add("office-char--speaking");
    this.setBubble(actor, icon, text);
    await this.sleep(ms);
    actor.el.classList.remove("office-char--speaking");
    if (actor.model) this.look(actor);
    else actor.bubble.hidden = true;
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
    this.xp = accountXp(usage.rows, loadTasks());
    this.meters = quotaMeters(usage.providers, Date.now());
    this.renderMeters();
    if (this.meters.length > 0) this.record({ t: "quota", left: Math.min(...this.meters.map((m) => m.left)) });
    const levels = new Map([...this.xp].map(([a, n]) => [a, levelOf(n).level]));
    if (levels.size > 0) this.record({ t: "level", level: Math.max(...levels.values()) });
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
    const tasks = loadTasks();
    return [...ids].map((account) => ({
      account,
      name: this.accountName(account),
      color: this.source.accounts().find((p) => p.id === account)?.color ?? "",
      tokens: this.xp?.get(account) ?? 0,
      tasks: tasks[account] ?? 0,
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
    this.record({ t: "class", id: cls });
    this.classes.set(account, cls);
    this.heroes.render(this.heroRows());
    this.scheduleRender();
  }

  private pickForm(account: string, form: HeroForm): void {
    saveHeroForm(account, form);
    this.record({ t: "form", id: form });
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
  /// `fromQueue`: opened by walking the queue, so answering moves on.
  private peekOrOpen(paneId: Uuid, fromQueue = false): void {
    this.queueMode = fromQueue;
    this.launchEl.hidden = true;
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
    const owned = SHOP_PETS.filter((p) => funStore.get().owned.includes(p));
    const want = [...petsFor(this.model.rooms.length), ...owned];
    while (this.pets.length > want.length) {
      const pet = this.pets.pop()!;
      pet.alive = false;
      pet.el.remove();
    }
    for (const species of want.slice(this.pets.length)) {
      const el = document.createElement("div");
      el.className = `office-pet office-pet--${species}`;
      el.appendChild(petSprite(species));
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
        this.record({ t: "count", what: "pet" });
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
      if (this.miniOn) {
        await this.sleep(3000);
        continue;
      }
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
        const fresh = this.makeActor(c.key, c, characterSprite(c.kind, c.role));
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
    const sig = JSON.stringify(
      this.model.rooms.map((r) => [r.door, r.rect.y + r.rect.h, r.doneDesk, r.mail.map((m) => [m.paneId, m.mail.id, m.mail.opened]), r.desks.map((d) => d.active)]),
    );
    if (sig === this.fixturesSig) return;
    this.fixturesSig = sig;
    const out: HTMLElement[] = [];
    for (const room of this.model.rooms) {
      const outside = { x: room.door.x, y: room.rect.y + room.rect.h };
      if (room.doneDesk) {
        const flag = document.createElement("button");
        flag.type = "button";
        flag.className = "office-flag";
        flag.innerHTML = flagSvg();
        paintProps(flag);
        flag.title = t("office.flag.review");
        this.position(flag, outside, 2);
        const target = room.doneDesk;
        // The finished work's diff review; a pane outside a repository opens.
        flag.addEventListener("click", (ev) => {
          ev.stopPropagation();
          void this.source.review(target).then((r) => {
            if (r === "ok") this.record({ t: "count", what: "review" });
            else if (!this.disposed) this.source.openPane(target);
          });
        });
        out.push(flag);
      }
      const box = document.createElement("button");
      box.type = "button";
      box.className = `office-mailbox${room.mail.length ? " office-mailbox--full" : ""}`;
      const sealed = room.mail.some((m) => !m.mail.opened);
      box.innerHTML = mailboxSvg(room.mail.length === 0 ? null : sealed ? "sealed" : "open");
      paintProps(box);
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
      // What the turn did (the activity log's headline, the diff, the time),
      // or the message's first line.
      const facts = turnLine(e);
      if (facts) {
        const sub = document.createElement("span");
        sub.className = "office-board__facts";
        sub.textContent = facts;
        b.appendChild(sub);
      }
      b.title = t("office.board.drag");
      // A click opens the pane; dragging the note onto an agent hands it over.
      b.addEventListener("click", (ev) => ev.stopPropagation());
      b.addEventListener("pointerdown", (ev) => {
        if (ev.button !== 0) return;
        ev.preventDefault();
        this.dragNote(e, ev, b);
      });
      li.appendChild(b);
      if (e.kind === "done") {
        const rv = document.createElement("button");
        rv.type = "button";
        rv.className = "office-board__review";
        rv.textContent = t("office.board.review");
        rv.addEventListener("click", (ev) => {
          ev.stopPropagation();
          void this.source.review(e.paneId).then((r) => {
            if (r === "ok") this.record({ t: "count", what: "review" });
            else if (!this.disposed) this.source.openPane(e.paneId);
          });
        });
        li.appendChild(rv);
      }
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

  private makeActor(key: string, model: CharModel | null, sprite: HTMLElement): Actor {
    const el = document.createElement("div");
    el.className = "office-char";
    if (model?.role === "sub") el.classList.add("office-char--sub");
    el.appendChild(sprite);
    const bubble = document.createElement("span");
    bubble.className = "office-char__bubble";
    bubble.hidden = true;
    el.appendChild(bubble);
    if (model) {
      el.dataset.pane = model.paneId;
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
      const since = this.waitingNow.find((w) => w.paneId === c.paneId)?.since ?? null;
      const b = this.bubbleFor(c, since, (el.dataset.stuck || null) as Stuck | null, Date.now());
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
      el.querySelector(".office-sprite")?.remove();
      el.prepend(characterSprite(c.kind, c.role, c.hero));
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
    // Unchanged: leave the DOM alone (this runs every second for everyone).
    const key = `${icon ?? ""}|${text}`;
    if (actor.bubble.dataset.key === key) return;
    actor.bubble.dataset.key = key;
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

  /// The bubble: the tool at work (a sweat drop when stuck), how long it has
  /// waited for the user, a tick when done, zzz when idle.
  private bubbleFor(c: CharModel, waitingSince: number | null, stuck: Stuck | null, now: number): { icon: string | null; text: string } {
    if (stuck) return { icon: "sweat", text: c.tool ? clip(c.tool, 12) : "" };
    if (waitingSince !== null && (c.status === "waiting" || c.role === "lead")) {
      return { icon: "alert", text: shortDuration(now - waitingSince, getLang()) };
    }
    switch (c.status) {
      case "working":
        return c.tool ? { icon: null, text: clip(c.tool, 14) } : { icon: "dots", text: "" };
      case "waiting":
        return { icon: "alert", text: "" };
      case "done": {
        if (c.role !== "lead") return { icon: null, text: "" };
        // The turn's tests: a star when they passed, a rain cloud when not.
        const test = this.source.log().find((e) => e.kind === "done" && e.paneId === c.paneId)?.turn?.test;
        return { icon: test === true ? "star" : test === false ? "cloud" : "check", text: "" };
      }
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

/// Put each placed piece of furniture under `root` where the user set it.
function placeFurniture(root: HTMLElement, places: Readonly<Record<string, { x: number; y: number }>>): void {
  for (const el of root.querySelectorAll<HTMLElement>(".office-prop--placed")) {
    const id = [...el.classList].find((c) => c.startsWith("fun-item--"))?.slice("fun-item--".length);
    const p = id ? places[id] : undefined;
    if (!p) continue;
    el.style.left = `${p.x * 100}%`;
    el.style.top = `${p.y * 100}%`;
  }
}

/// A finished turn's facts ("Changed 2 files (+10/-3) · 3m"), or a
/// message's first line.
function turnLine(e: LogEntry): string {
  if (e.kind !== "done") return e.snippet ? `“${e.snippet}”` : "";
  const turn = e.turn;
  if (!turn) return "";
  const parts: string[] = [];
  if (turn.headline) parts.push(turn.headline);
  else if (turn.files !== null) parts.push(fill(t("office.note.stats"), { files: turn.files, add: turn.add ?? 0, del: turn.del ?? 0 }));
  if (turn.durationMs !== null) parts.push(shortDuration(turn.durationMs, getLang()));
  return parts.join(" · ");
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

/// Looks like a record export (an object with counters).
function isRecord(v: unknown): boolean {
  return !!v && typeof v === "object" && !Array.isArray(v) && typeof (v as { counters?: unknown }).counters === "object";
}
