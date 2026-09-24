// Files pane: a GUI file manager rendered by ymux itself (spec §2), used both
// as a layout pane (`PaneKind::Files`) and as the body of the right-side file
// dock. It implements `Pane` the way `BrowserPane` does, so SplitContainer
// and PaneGroup treat it like any other leaf. There is no PTY.
//
// Every decision lives in a pure module and is tested there: ordering, the
// selection machine and overwrite rules in `fileModel.ts`, the keymap and
// the "ymux globals always win" predicate in `keys.ts`, the preview in
// `preview.ts`. This file is the DOM and IPC wiring around them.
//
// Three things here exist because of how panes are hosted:
//
//  - **The list is virtualised.** `fs_list_dir` has no pagination, so a
//    directory of 50,000 entries arrives whole; only the rows in view (plus
//    a small overscan) exist as DOM, drawn from a fixed pool, so the cost of
//    a huge directory is one array and one sort, not 50,000 nodes.
//  - **The scroll offset is pane state, not DOM state.** SplitContainer and
//    PaneGroup re-parent pane elements on every layout mutation, which
//    resets a scroll container's `scrollTop` to 0 behind our back (the same
//    hazard CLAUDE.md rule 14 documents for xterm). `scheduleFit()` — which
//    both call — restores it on the next frame and re-measures, since a list
//    shown after `display: none` measured a height of 0.
//  - **Nothing is listed while the pane is not on screen.** A hidden tab or
//    a closed dock records the directory it should show and lists it when
//    shown, so following a busy terminal's `cd`s costs nothing in the
//    background.

import type { Pane } from "../layout/Pane";
import type { Uuid } from "../types";
import { fsApi, errorKind } from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";
import { IS_MAC, shortcutLabel } from "../platform";
import { showContextMenu, type ContextMenuEntry } from "../menu/ContextMenu";
import { askChoice, askConfirm, askText } from "../ui/Dialog";
import {
  applyHidden,
  baseName,
  canReplace,
  crumbs,
  extendTo,
  findConflict,
  formatSize,
  isHiddenName,
  joinPath,
  moveCursor,
  nextSelectionAfterDelete,
  parentPath,
  reconcile,
  resolveOverwrite,
  selectAll,
  selectOnly,
  sortEntries,
  stemEnd,
  targetNames,
  toggleAt,
  typeAhead,
  uniqueName,
  type FileEntry,
  type OverwriteChoice,
  type Selection,
} from "./fileModel";
import { paneKeyAction, type PaneKeyAction } from "./keys";
import {
  BINARY_SNIFF_BYTES,
  MAX_PREVIEW_BYTES,
  decodePreview,
  directoryPreview,
  isProbablyBinary,
  type Preview,
} from "./preview";
import { getClipboard, onClipboardChange, setClipboard } from "./clipboard";

export interface FilesPaneOptions {
  id: Uuid;
  /// Directory to show first. `null` opens the home directory.
  dir: string | null;
  title?: string | null;
  /// Draw the pane's own title row. False inside a tab group (the group
  /// draws one) and in the dock (which has no title).
  ownChrome?: boolean;
  /// The dock's layout: preview always below the list, never beside it.
  docked?: boolean;
  /// Focus reporting for layout panes. The dock passes none, on purpose:
  /// its pane must never become the manager's focused pane (spec §3.7).
  onFocus?: () => void;
  /// The shown directory changed (persisted as the layout pane's `cwd`).
  onDirChange?: (dir: string) => void;
  /// Open a text file in the host's viewer.
  openFile: (path: string) => void | Promise<void>;
  /// Open a terminal whose cwd is `dir`.
  openTerminal?: (dir: string) => void | Promise<void>;
}

/// Row height in px. Fixed, because the list is virtualised by arithmetic.
const ROW_H = 24;
/// Rows drawn above and below the viewport.
const OVERSCAN = 6;
/// Keys typed within this long of each other extend one type-ahead query.
const TYPEAHEAD_MS = 800;
/// A cursor that rests this long gets a preview. Holding ↓ reads nothing.
const PREVIEW_DELAY_MS = 90;

/// Above this many entries the window-focus refresh is skipped.
const FOCUS_REFRESH_MAX = 10_000;

const PREFS_KEY = "ymux.files.prefs";

interface Prefs {
  showHidden: boolean;
  showPreview: boolean;
}

function readPrefs(): Prefs {
  try {
    const v = JSON.parse(localStorage.getItem(PREFS_KEY) ?? "{}") as Partial<Prefs>;
    return { showHidden: v.showHidden === true, showPreview: v.showPreview !== false };
  } catch {
    return { showHidden: false, showPreview: true };
  }
}

function writePrefs(p: Prefs): void {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(p));
  } catch {
    /* per-viewer convenience only */
  }
}

/// A backend error as the user should read it: the translated kind when
/// there is one (`fsError.not_found`, …), else the backend's own message.
export function describeFsError(e: unknown): string {
  const kind = errorKind(e);
  const key = `fsError.${kind}`;
  const text = t(key);
  if (text !== key) return text;
  return e instanceof Error ? e.message : String(e);
}

function fill(template: string, vars: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m));
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/// Today → `14:03`; otherwise `2026-09-24`. Numeric on purpose: it reads the
/// same in all 13 languages and sorts visually.
function formatModified(ms: number, now = new Date()): string {
  if (!ms) return "";
  const d = new Date(ms);
  if (
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  ) {
    return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
  }
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

// Small line icons, 14px, drawn in `currentColor`. Static markup only.
const ICON = {
  folder:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 4.5a1 1 0 0 1 1-1h3.6l1.5 1.5h5.9a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z" fill="currentColor" fill-opacity=".22" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/></svg>',
  file: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 1.5h6l3 3v10h-9z" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/><path d="M9.5 1.5v3h3" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/></svg>',
  up: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 13V3.5M3.5 8 8 3.5 12.5 8" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  refresh:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13 8a5 5 0 1 1-1.5-3.6M13 2.5v3h-3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  newFolder:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 4.5a1 1 0 0 1 1-1h3.6l1.5 1.5h5.9a1 1 0 0 1 1 1v6.5a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/><path d="M8 7.5v4M6 9.5h4" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>',
  newFile:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 1.5h6l3 3v10h-9z" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/><path d="M8 7v5M5.5 9.5h5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>',
  hidden:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z" fill="none" stroke="currentColor" stroke-width="1.1"/><circle cx="8" cy="8" r="2" fill="currentColor"/></svg>',
  preview:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" rx="1" fill="none" stroke="currentColor" stroke-width="1.1"/><path d="M1.5 9h13" stroke="currentColor" stroke-width="1.1"/></svg>',
  link: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 10 12 4M7.5 4H12v4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>',
};

interface RowEls {
  row: HTMLElement;
  icon: HTMLElement;
  name: HTMLElement;
  link: HTMLElement;
  size: HTMLElement;
  date: HTMLElement;
}

export class FilesPane implements Pane {
  readonly id: Uuid;
  readonly element: HTMLElement;

  private titleEl: HTMLElement | null = null;
  private readonly crumbsEl: HTMLElement;
  private readonly pathInput: HTMLInputElement;
  private readonly list: HTMLElement;
  private readonly spacer: HTMLElement;
  private readonly overlay: HTMLElement;
  private readonly previewEl: HTMLElement;
  private readonly previewHead: HTMLElement;
  private readonly previewBody: HTMLElement;
  private readonly statusCount: HTMLElement;
  private readonly statusMsg: HTMLElement;
  private readonly buttons: { el: HTMLButtonElement; key: string }[] = [];
  private readonly hiddenBtn: HTMLButtonElement;
  private readonly previewBtn: HTMLButtonElement;

  private dir: string | null;
  private title: string | null;
  private entries: FileEntry[] = [];
  private names: string[] = [];
  private sel: Selection = selectOnly([], 0);
  private listError: string | null = null;
  private prefs = readPrefs();
  /// The directory changed while the pane was not on screen.
  private stale = true;
  private loadGen = 0;
  private previewGen = 0;
  private previewTimer: number | null = null;
  private scrollTop = 0;
  private rowPool: RowEls[] = [];
  private typed = "";
  private typedAt = 0;
  private busy = false;
  private statusTimer: number | null = null;
  private disposed = false;
  private readonly cleanups: (() => void)[] = [];

  constructor(private readonly opts: FilesPaneOptions) {
    this.id = opts.id;
    this.dir = opts.dir;
    this.title = opts.title ?? null;

    this.element = document.createElement("div");
    this.element.className = "pane files-pane";
    if (opts.docked) this.element.classList.add("files-pane--docked");
    this.element.dataset.paneId = this.id;
    this.element.tabIndex = -1;

    // ── Toolbar: up, refresh │ breadcrumb │ new folder, new file, hidden, preview
    const bar = document.createElement("div");
    bar.className = "files__bar";
    const up = this.makeButton(ICON.up, "files.up", () => void this.goUp());
    const refresh = this.makeButton(ICON.refresh, "files.refresh", () => void this.refresh());

    this.crumbsEl = document.createElement("div");
    this.crumbsEl.className = "files__crumbs";
    this.crumbsEl.addEventListener("dblclick", (ev) => {
      if (ev.target === this.crumbsEl) this.editPath();
    });
    this.pathInput = document.createElement("input");
    this.pathInput.type = "text";
    this.pathInput.className = "files__path";
    this.pathInput.spellcheck = false;
    this.pathInput.hidden = true;
    this.pathInput.addEventListener("keydown", (ev) => this.onPathKey(ev));
    this.pathInput.addEventListener("blur", () => this.endEditPath());

    const newFolder = this.makeButton(ICON.newFolder, "files.newFolder", () => void this.newFolder());
    const newFile = this.makeButton(ICON.newFile, "files.newFile", () => void this.newFile());
    this.hiddenBtn = this.makeButton(ICON.hidden, "files.showHidden", () => this.toggleHidden());
    this.previewBtn = this.makeButton(ICON.preview, "files.togglePreview", () =>
      this.togglePreview(),
    );
    const sep = () => {
      const s = document.createElement("span");
      s.className = "files__bar-sep";
      return s;
    };
    bar.append(up, refresh, sep(), this.crumbsEl, this.pathInput, sep());
    bar.append(newFolder, newFile, this.hiddenBtn, this.previewBtn);

    // ── Body: the list and the preview
    const main = document.createElement("div");
    main.className = "files__main";

    this.list = document.createElement("div");
    this.list.className = "files__list";
    this.list.tabIndex = 0;
    this.list.setAttribute("role", "listbox");
    this.list.setAttribute("aria-multiselectable", "true");
    this.spacer = document.createElement("div");
    this.spacer.className = "files__spacer";
    this.list.appendChild(this.spacer);
    this.overlay = document.createElement("div");
    this.overlay.className = "files__overlay";
    this.overlay.hidden = true;
    this.list.appendChild(this.overlay);

    this.previewEl = document.createElement("div");
    this.previewEl.className = "files__preview";
    this.previewHead = document.createElement("div");
    this.previewHead.className = "files__preview-head";
    this.previewBody = document.createElement("div");
    this.previewBody.className = "files__preview-body";
    this.previewEl.append(this.previewHead, this.previewBody);
    main.append(this.list, this.previewEl);

    // ── Status line
    const status = document.createElement("div");
    status.className = "files__status";
    this.statusCount = document.createElement("span");
    this.statusCount.className = "files__status-count";
    this.statusMsg = document.createElement("span");
    this.statusMsg.className = "files__status-msg";
    this.statusMsg.setAttribute("aria-live", "polite");
    status.append(this.statusCount, this.statusMsg);

    this.element.append(bar, main, status);
    if (opts.ownChrome !== false && !opts.docked) this.buildTitle();

    this.wireList();
    this.applyPrefs();

    this.element.addEventListener("focusin", () => this.opts.onFocus?.());

    // Refresh on window focus, as ydir's Ctrl+R but automatic. Not for a
    // huge folder: re-listing 10k+ entries on every alt-tab costs more than
    // it is worth, and F5 is one key away.
    const onWinFocus = () => {
      if (this.isShown() && !this.busy && this.names.length <= FOCUS_REFRESH_MAX) {
        void this.load({ quiet: true });
      }
    };
    window.addEventListener("focus", onWinFocus);
    this.cleanups.push(() => window.removeEventListener("focus", onWinFocus));

    const ro = new ResizeObserver(() => this.onResize());
    ro.observe(this.element);
    this.cleanups.push(() => ro.disconnect());

    this.cleanups.push(onLangChange(() => this.updateLang()));
    this.cleanups.push(onClipboardChange(() => this.renderRows()));
    this.updateLang();
  }

  // ── Pane interface ────────────────────────────────────────────────────────

  focus(): void {
    this.list.focus({ preventScroll: true });
  }

  /// Called by SplitContainer / PaneGroup around every re-parent and un-hide.
  /// One frame later the box is measurable: restore the scroll offset the
  /// re-parent threw away, and list the directory if it changed off screen.
  scheduleFit(): void {
    requestAnimationFrame(() => this.onShown());
  }

  async spawn(): Promise<void> {
    if (!this.dir) {
      try {
        this.dir = await fsApi.homeDir();
      } catch {
        this.dir = null;
      }
    }
    this.stale = true;
    this.renderCrumbs();
    this.updateTitle();
    this.onShown();
  }

  dispose(): void {
    this.disposed = true;
    this.loadGen++;
    this.previewGen++;
    if (this.previewTimer !== null) clearTimeout(this.previewTimer);
    if (this.statusTimer !== null) clearTimeout(this.statusTimer);
    for (const c of this.cleanups) c();
    this.element.remove();
  }

  // ── Host API ──────────────────────────────────────────────────────────────

  /// Show `dir`. The dock's cwd-follow calls this; so does every in-pane
  /// navigation. Listed now if on screen, else when next shown.
  navigate(dir: string, prefer?: string): void {
    this.dir = dir;
    this.stale = true;
    this.listError = null;
    this.renderCrumbs();
    this.updateTitle();
    if (this.isShown()) void this.load({ prefer, changedDir: true });
  }

  currentDir(): string | null {
    return this.dir;
  }

  setTitle(title: string | null): void {
    this.title = title && title.trim() ? title : null;
    this.updateTitle();
  }

  /// Inside a tab group the group draws the title; alone, the pane does.
  setOwnChrome(enabled: boolean): void {
    if (this.opts.docked) return;
    this.element.classList.toggle("pane--tab", !enabled);
    if (enabled && !this.titleEl) this.buildTitle();
    if (!enabled && this.titleEl) {
      this.titleEl.remove();
      this.titleEl = null;
    }
  }

  // ── Listing ───────────────────────────────────────────────────────────────

  private isShown(): boolean {
    return this.element.isConnected && this.element.getClientRects().length > 0;
  }

  private onShown(): void {
    if (this.disposed || !this.isShown()) return;
    this.onResize();
    if (this.stale) {
      void this.load({ changedDir: true });
      return;
    }
    if (this.list.scrollTop !== this.scrollTop) this.list.scrollTop = this.scrollTop;
    this.renderRows();
  }

  /// List `this.dir`. A newer call wins: each one takes a generation number
  /// and a stale answer is dropped, so a slow network folder can never paint
  /// over the folder the user moved on to.
  private async load(o: { prefer?: string; select?: string[]; changedDir?: boolean; quiet?: boolean } = {}): Promise<void> {
    const dir = this.dir;
    if (!dir) return;
    const gen = ++this.loadGen;
    this.stale = false;
    let list: FileEntry[];
    try {
      list = await fsApi.listDir(dir, this.prefs.showHidden);
    } catch (e) {
      if (gen !== this.loadGen || this.disposed) return;
      if (o.quiet && !o.changedDir && errorKind(e) !== "not_found") return;
      this.entries = [];
      this.names = [];
      this.sel = selectOnly([], 0);
      this.listError = describeFsError(e);
      this.renderAll();
      return;
    }
    if (gen !== this.loadGen || this.disposed) return;
    this.listError = null;
    const prevNames = this.names;
    this.entries = sortEntries(applyHidden(list, this.prefs.showHidden));
    this.names = this.entries.map((e) => e.name);
    if (o.changedDir) {
      this.sel = o.prefer
        ? reconcile(selectOnly([], 0), [], this.names, o.prefer)
        : selectOnly(this.names, 0);
      this.scrollTop = 0;
      this.list.scrollTop = 0;
      this.opts.onDirChange?.(dir);
    } else {
      this.sel = reconcile(this.sel, prevNames, this.names, o.prefer);
    }
    if (o.select?.length) {
      const want = new Set(o.select);
      const first = this.names.findIndex((n) => want.has(n));
      if (first >= 0) this.sel = { cursor: first, anchor: first, selected: want };
    }
    this.renderAll();
    // Only when the cursor was *placed*: a plain refresh (window focus)
    // must not yank a list the user scrolled with the wheel.
    if (o.changedDir || o.prefer !== undefined || o.select?.length) {
      this.ensureVisible(this.sel.cursor);
    }
  }

  private async refresh(): Promise<void> {
    await this.load({});
  }

  // ── Rendering ─────────────────────────────────────────────────────────────

  private renderAll(): void {
    this.spacer.style.height = `${this.names.length * ROW_H}px`;
    this.renderOverlay();
    this.renderRows();
    this.renderStatus();
    this.schedulePreview();
  }

  private renderOverlay(): void {
    this.overlay.replaceChildren();
    if (this.listError) {
      this.overlay.hidden = false;
      this.overlay.classList.add("files__overlay--error");
      const head = document.createElement("p");
      head.className = "files__overlay-title";
      head.textContent = t("files.cantOpen");
      const why = document.createElement("p");
      why.textContent = this.listError;
      const row = document.createElement("div");
      row.className = "files__overlay-actions";
      const up = document.createElement("button");
      up.type = "button";
      up.className = "files__overlay-btn";
      up.textContent = t("files.goUp");
      up.disabled = !this.dir || parentPath(this.dir) === null;
      up.addEventListener("click", () => void this.goUp());
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "files__overlay-btn";
      retry.textContent = t("files.retry");
      retry.addEventListener("click", () => void this.load({ changedDir: true }));
      row.append(up, retry);
      this.overlay.append(head, why, row);
      return;
    }
    this.overlay.classList.remove("files__overlay--error");
    if (this.names.length === 0 && !this.stale) {
      this.overlay.hidden = false;
      const p = document.createElement("p");
      p.textContent = t("files.empty");
      this.overlay.appendChild(p);
      return;
    }
    this.overlay.hidden = true;
  }

  /// Draw the rows in view from the pool. Rows are positioned slots, reused
  /// in place, so a click's element survives the re-render it triggers and
  /// the second click of a double-click lands on the same node.
  private renderRows(): void {
    const h = this.list.clientHeight;
    if (h === 0) return;
    const n = this.names.length;
    const first = Math.max(0, Math.floor(this.list.scrollTop / ROW_H) - OVERSCAN);
    const last = Math.min(n, Math.ceil((this.list.scrollTop + h) / ROW_H) + OVERSCAN);
    const count = Math.max(0, last - first);
    while (this.rowPool.length < count) this.rowPool.push(this.makeRow());
    const clip = getClipboard();
    const cutHere =
      clip?.mode === "cut" && this.dir !== null && clip.dir.normalize("NFC") === this.dir.normalize("NFC")
        ? new Set(clip.items.map((i) => i.name))
        : null;
    const focusedList = document.activeElement === this.list;
    for (let k = 0; k < this.rowPool.length; k++) {
      const els = this.rowPool[k];
      const i = first + k;
      if (k >= count) {
        if (els.row.parentElement) els.row.remove();
        continue;
      }
      if (els.row.parentElement !== this.spacer) this.spacer.appendChild(els.row);
      const e = this.entries[i];
      els.row.dataset.index = String(i);
      els.row.style.transform = `translateY(${i * ROW_H}px)`;
      if (els.name.textContent !== e.name) {
        els.name.textContent = e.name;
        els.row.title = e.name;
      }
      els.icon.innerHTML = e.is_dir ? ICON.folder : ICON.file;
      els.link.hidden = !e.is_symlink;
      els.size.textContent = e.is_dir ? "" : formatSize(e.size);
      els.date.textContent = formatModified(e.modified_ms);
      const selected = this.sel.selected.has(e.name);
      const cls = els.row.classList;
      cls.toggle("files__row--dir", e.is_dir);
      cls.toggle("files__row--dotfile", isHiddenName(e.name));
      cls.toggle("files__row--selected", selected);
      cls.toggle("files__row--cursor", i === this.sel.cursor);
      cls.toggle("files__row--cut", !!cutHere?.has(e.name));
      els.row.setAttribute("aria-selected", String(selected));
    }
    this.list.classList.toggle("files__list--focused", focusedList);
  }

  private makeRow(): RowEls {
    const row = document.createElement("div");
    row.className = "files__row";
    row.setAttribute("role", "option");
    const icon = document.createElement("span");
    icon.className = "files__icon";
    const name = document.createElement("span");
    name.className = "files__name";
    const link = document.createElement("span");
    link.className = "files__link";
    link.innerHTML = ICON.link;
    const size = document.createElement("span");
    size.className = "files__size";
    const date = document.createElement("span");
    date.className = "files__date";
    row.append(icon, name, link, size, date);
    return { row, icon, name, link, size, date };
  }

  private renderCrumbs(): void {
    this.crumbsEl.replaceChildren();
    if (!this.dir) return;
    const parts = crumbs(this.dir);
    parts.forEach((c, i) => {
      if (i > 0) {
        const s = document.createElement("span");
        s.className = "files__crumb-sep";
        s.textContent = "›";
        this.crumbsEl.appendChild(s);
      }
      const b = document.createElement("button");
      b.type = "button";
      b.tabIndex = -1;
      b.className = "files__crumb";
      if (i === parts.length - 1) b.classList.add("files__crumb--current");
      b.textContent = c.label;
      b.title = c.path;
      b.addEventListener("mousedown", (ev) => ev.preventDefault());
      b.addEventListener("click", () => {
        if (i < parts.length - 1) this.navigate(c.path, parts[i + 1]?.label);
        this.focus();
      });
      this.crumbsEl.appendChild(b);
    });
    // The deep end of a long path is the part that says where you are.
    requestAnimationFrame(() => {
      this.crumbsEl.scrollLeft = this.crumbsEl.scrollWidth;
    });
  }

  private renderStatus(): void {
    const total = this.names.length;
    const n = this.sel.selected.size;
    this.statusCount.textContent =
      n > 1 ? fill(t("files.selected"), { n, total }) : fill(t("files.items"), { n: total });
  }

  /// A transient line in the status bar. Errors stay until the next action.
  private say(text: string, error = false): void {
    if (this.statusTimer !== null) clearTimeout(this.statusTimer);
    this.statusMsg.textContent = text;
    this.statusMsg.classList.toggle("files__status-msg--error", error);
    if (!error && text) {
      this.statusTimer = window.setTimeout(() => {
        this.statusMsg.textContent = "";
      }, 2500);
    }
  }

  private updateTitle(): void {
    const label = this.title || (this.dir ? baseName(this.dir) : t("files.title"));
    if (this.titleEl) this.titleEl.textContent = label;
  }

  private buildTitle(): void {
    this.titleEl = document.createElement("div");
    this.titleEl.className = "pane-title";
    this.element.insertBefore(this.titleEl, this.element.firstChild);
    this.updateTitle();
  }

  private updateLang(): void {
    for (const { el, key } of this.buttons) el.title = t(key);
    this.crumbsEl.title = `${t("files.editPath")} (${shortcutLabel("Ctrl+L")})`;
    this.updateTitle();
    this.renderOverlay();
    this.renderStatus();
  }

  private onResize(): void {
    const w = this.element.clientWidth;
    if (w === 0) return;
    this.element.classList.toggle("files-pane--narrow", w < 380);
    this.element.classList.toggle("files-pane--tiny", w < 270);
    this.element.classList.toggle("files-pane--wide", !this.opts.docked && w >= 720);
    // Shown by a path that did not call scheduleFit (a workspace container
    // un-hidden, say): the size change is the signal, so list now.
    if (this.stale && this.dir && !this.disposed) {
      void this.load({ changedDir: true });
      return;
    }
    this.renderRows();
  }

  private ensureVisible(i: number): void {
    const h = this.list.clientHeight;
    if (h === 0) return;
    const top = i * ROW_H;
    if (top < this.list.scrollTop) this.list.scrollTop = top;
    else if (top + ROW_H > this.list.scrollTop + h) this.list.scrollTop = top + ROW_H - h;
    this.scrollTop = this.list.scrollTop;
  }

  // ── Preview ───────────────────────────────────────────────────────────────

  private schedulePreview(): void {
    if (this.previewTimer !== null) clearTimeout(this.previewTimer);
    const gen = ++this.previewGen;
    if (!this.prefs.showPreview) return;
    this.previewTimer = window.setTimeout(() => {
      this.previewTimer = null;
      void this.loadPreview(gen);
    }, PREVIEW_DELAY_MS);
  }

  private async loadPreview(gen: number): Promise<void> {
    const e = this.entries[this.sel.cursor];
    this.previewHead.replaceChildren();
    if (!e || !this.isShown()) {
      this.previewBody.replaceChildren();
      return;
    }
    const name = document.createElement("span");
    name.className = "files__preview-name";
    name.textContent = e.name;
    const meta = document.createElement("span");
    meta.className = "files__preview-meta";
    meta.textContent = e.is_dir ? "" : formatSize(e.size);
    this.previewHead.append(name, meta);

    let preview: Preview;
    try {
      if (e.is_dir) {
        const peek = await fsApi.peekDir(e.path, this.prefs.showHidden);
        preview = directoryPreview(peek.entries, peek.more);
      } else if (e.size === 0) {
        preview = { kind: "text", lines: [], truncated: false };
      } else {
        const head = await fsApi.readHead(e.path, MAX_PREVIEW_BYTES);
        preview = decodePreview(head, e.size);
      }
    } catch (err) {
      if (gen !== this.previewGen) return;
      this.previewBody.replaceChildren(this.previewNote(describeFsError(err), true));
      return;
    }
    if (gen !== this.previewGen || this.disposed) return;
    this.renderPreview(preview);
  }

  private previewNote(text: string, error = false): HTMLElement {
    const p = document.createElement("p");
    p.className = error ? "files__preview-note files__preview-note--error" : "files__preview-note";
    p.textContent = text;
    return p;
  }

  private renderPreview(p: Preview): void {
    if (p.kind === "binary") {
      this.previewBody.replaceChildren(this.previewNote(t("files.binary")));
      return;
    }
    if (p.kind === "text") {
      if (p.lines.length === 0) {
        this.previewBody.replaceChildren(this.previewNote(t("files.previewEmpty")));
        return;
      }
      const pre = document.createElement("pre");
      pre.className = "files__preview-text";
      pre.textContent = p.lines.join("\n");
      this.previewBody.replaceChildren(pre);
      if (p.truncated) this.previewBody.appendChild(this.previewNote(t("files.previewMore")));
      return;
    }
    const ul = document.createElement("ul");
    ul.className = "files__preview-dir";
    for (const e of p.entries) {
      const li = document.createElement("li");
      li.className = e.is_dir ? "files__preview-entry files__preview-entry--dir" : "files__preview-entry";
      const icon = document.createElement("span");
      icon.className = "files__icon";
      icon.innerHTML = e.is_dir ? ICON.folder : ICON.file;
      const label = document.createElement("span");
      label.className = "files__name";
      label.textContent = e.name;
      li.append(icon, label);
      ul.appendChild(li);
    }
    this.previewBody.replaceChildren(ul);
    if (p.entries.length === 0) this.previewBody.replaceChildren(this.previewNote(t("files.empty")));
    else if (p.more) this.previewBody.appendChild(this.previewNote(t("files.previewMore")));
  }

  // ── Input ─────────────────────────────────────────────────────────────────

  private wireList(): void {
    this.list.addEventListener("scroll", () => {
      // A detached or hidden list reports 0; that is the re-parent reset,
      // not the user, and must not overwrite the real offset.
      if (this.list.clientHeight > 0 && this.element.isConnected) {
        this.scrollTop = this.list.scrollTop;
      }
      this.renderRows();
    });
    this.list.addEventListener("focus", () => this.renderRows());
    this.list.addEventListener("blur", () => this.renderRows());
    this.list.addEventListener("keydown", (ev) => this.onKey(ev));
    this.list.addEventListener("mousedown", (ev) => this.onMouseDown(ev));
    this.list.addEventListener("dblclick", (ev) => {
      const i = this.rowIndexAt(ev.target);
      if (i !== null) void this.openAt(i);
    });
    this.list.addEventListener("contextmenu", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      this.showMenu(ev);
    });
  }

  private rowIndexAt(target: EventTarget | null): number | null {
    const row = (target as HTMLElement | null)?.closest<HTMLElement>(".files__row");
    const i = row ? Number(row.dataset.index) : NaN;
    return Number.isInteger(i) && i >= 0 && i < this.names.length ? i : null;
  }

  private onMouseDown(ev: MouseEvent): void {
    const i = this.rowIndexAt(ev.target);
    this.focus();
    if (i === null) return;
    const mod = IS_MAC ? ev.metaKey : ev.ctrlKey;
    if (ev.button === 2) {
      // Right-click keeps a selection it lands in, so the menu acts on it.
      if (!this.sel.selected.has(this.names[i])) this.setSel(selectOnly(this.names, i));
      return;
    }
    if (ev.button !== 0) return;
    if (ev.shiftKey) this.setSel(extendTo(this.sel, this.names, i));
    else if (mod) this.setSel(toggleAt(this.sel, this.names, i));
    else this.setSel(selectOnly(this.names, i));
  }

  private setSel(next: Selection, scroll = true): void {
    const moved = next.cursor !== this.sel.cursor;
    this.sel = next;
    if (scroll) this.ensureVisible(next.cursor);
    this.renderRows();
    this.renderStatus();
    if (moved || next.selected.size <= 1) this.schedulePreview();
  }

  private onKey(ev: KeyboardEvent): void {
    const action = paneKeyAction(ev, IS_MAC);
    if (!action) return;
    ev.preventDefault();
    this.say("");
    void this.run(action);
  }

  private async run(a: PaneKeyAction): Promise<void> {
    const names = this.names;
    const page = Math.max(1, Math.floor(this.list.clientHeight / ROW_H) - 1);
    switch (a.kind) {
      case "move":
        return this.setSel(moveCursor(this.sel, names, a.delta, a.extend));
      case "page":
        return this.setSel(moveCursor(this.sel, names, a.dir * page, a.extend));
      case "edge": {
        const to = a.end ? names.length - 1 : 0;
        return this.setSel(a.extend ? extendTo(this.sel, names, to) : selectOnly(names, to));
      }
      case "open":
        return this.openAt(this.sel.cursor);
      case "parent":
        return this.goUp();
      case "rename":
        return this.rename();
      case "delete":
        return this.remove(a.permanent);
      case "selectAll":
        return this.setSel(selectAll(this.sel, names), false);
      case "copy":
        return this.toClipboard("copy");
      case "cut":
        return this.toClipboard("cut");
      case "paste":
        return this.paste();
      case "refresh":
        return this.refresh();
      case "editPath":
        return this.editPath();
      case "newFolder":
        return this.newFolder();
      case "togglePreview":
        return this.togglePreview();
      case "escape": {
        const clip = getClipboard();
        if (clip?.mode === "cut") setClipboard(null);
        else this.setSel(selectOnly(names, this.sel.cursor));
        return;
      }
      case "typeAhead": {
        const now = Date.now();
        const extending = now - this.typedAt < TYPEAHEAD_MS;
        this.typed = extending ? this.typed + a.text : a.text;
        this.typedAt = now;
        // A fresh letter starts after the cursor, so pressing `s` again
        // steps through every `s…`; a continued query stays put if it fits.
        const from = extending ? this.sel.cursor : this.sel.cursor + 1;
        const i = typeAhead(names, this.typed, from);
        if (i >= 0) this.setSel(selectOnly(names, i));
        return;
      }
    }
  }

  // ── Actions ───────────────────────────────────────────────────────────────

  private targets(): FileEntry[] {
    const byName = new Map(this.entries.map((e) => [e.name, e]));
    return targetNames(this.sel, this.names)
      .map((n) => byName.get(n))
      .filter((e): e is FileEntry => !!e);
  }

  private async goUp(): Promise<void> {
    if (!this.dir) return;
    const parent = parentPath(this.dir);
    if (parent === null) return;
    this.navigate(parent, baseName(this.dir));
  }

  private async openAt(i: number): Promise<void> {
    const e = this.entries[i];
    if (!e) return;
    if (e.is_dir) {
      this.navigate(e.path);
      return;
    }
    await this.openFile(e);
  }

  /// A text file goes to the host's viewer; a binary one (the same 8 KiB
  /// NUL sniff as the preview and the backend) to the OS default app, which
  /// reveals rather than runs an executable (`fspath::should_reveal`).
  private async openFile(e: FileEntry): Promise<void> {
    try {
      const head = await fsApi.readHead(e.path, BINARY_SNIFF_BYTES);
      if (isProbablyBinary(head)) await fsApi.openDefault(e.path);
      else await this.opts.openFile(e.path);
    } catch (err) {
      this.say(`${e.name}: ${describeFsError(err)}`, true);
    }
  }

  private async openDefault(e: FileEntry): Promise<void> {
    try {
      await fsApi.openDefault(e.path);
    } catch (err) {
      this.say(`${e.name}: ${describeFsError(err)}`, true);
    }
  }

  private async withBusy<T>(fn: () => Promise<T>): Promise<T> {
    this.busy = true;
    this.say(t("files.working"));
    try {
      return await fn();
    } finally {
      this.busy = false;
      if (this.statusMsg.textContent === t("files.working")) this.say("");
    }
  }

  private async rename(): Promise<void> {
    const e = this.entries[this.sel.cursor];
    if (!e || !this.dir) return;
    const dir = this.dir;
    // The raw name, never a normalised one: on APFS an NFC rewrite of an NFD
    // name is a real rename the user did not ask for.
    const next = await askText(t("files.renamePrompt"), e.name, {
      selectEnd: stemEnd(e.name, e.is_dir),
    });
    this.focus();
    if (next === null || next === e.name || !next.trim()) return;
    try {
      await fsApi.rename(e.path, joinPath(dir, next));
    } catch (err) {
      this.say(`${next}: ${describeFsError(err)}`, true);
      return;
    }
    await this.load({ prefer: next });
  }

  private async create(kind: "dir" | "file"): Promise<void> {
    if (!this.dir) return;
    const dir = this.dir;
    const suggested = uniqueName(
      t(kind === "dir" ? "files.defaultFolderName" : "files.defaultFileName"),
      kind === "dir",
      this.names,
    );
    const name = await askText(
      t(kind === "dir" ? "files.newFolderPrompt" : "files.newFilePrompt"),
      suggested,
      { selectEnd: stemEnd(suggested, kind === "dir") },
    );
    this.focus();
    if (name === null || !name.trim()) return;
    try {
      const path = joinPath(dir, name);
      if (kind === "dir") await fsApi.createDir(path);
      else await fsApi.createFile(path);
    } catch (err) {
      this.say(`${name}: ${describeFsError(err)}`, true);
      return;
    }
    await this.load({ prefer: name });
  }

  private newFolder(): Promise<void> {
    return this.create("dir");
  }

  private newFile(): Promise<void> {
    return this.create("file");
  }

  /// Trash with a confirm by default; permanent delete is Shift+Delete. If
  /// the trash is unavailable (a network share, a WSL path), say why and
  /// offer a permanent delete rather than just failing.
  private async remove(permanent: boolean): Promise<void> {
    const targets = this.targets();
    if (!targets.length) return;
    const one = targets.length === 1;
    const message = permanent
      ? one
        ? fill(t("files.confirmDelete"), { name: targets[0].name })
        : fill(t("files.confirmDeleteMany"), { n: targets.length })
      : one
        ? fill(t("files.confirmTrash"), { name: targets[0].name })
        : fill(t("files.confirmTrashMany"), { n: targets.length });
    const ok = await askConfirm(message, t(permanent ? "files.deletePermanent" : "files.trash"));
    this.focus();
    if (!ok) return;
    const paths = targets.map((e) => e.path);
    const names = new Set(targets.map((e) => e.name));
    const next = nextSelectionAfterDelete(this.names, names, this.sel.cursor);
    await this.withBusy(async () => {
      try {
        await fsApi.delete(paths, !permanent);
      } catch (err) {
        if (permanent) {
          this.say(describeFsError(err), true);
          return;
        }
        const reason = err instanceof Error ? err.message : String(err);
        const again = await askConfirm(
          fill(t("files.trashFailed"), { reason }),
          t("files.deletePermanent"),
        );
        this.focus();
        if (!again) return;
        // One path at a time: a trash that failed part-way already removed
        // some, and a batch delete would stop at the first missing one.
        for (const p of paths) {
          try {
            await fsApi.delete([p], false);
          } catch (err2) {
            if (errorKind(err2) !== "not_found") this.say(describeFsError(err2), true);
          }
        }
      }
    });
    await this.load({ prefer: next ?? undefined });
  }

  private toClipboard(mode: "copy" | "cut"): void {
    const targets = this.targets();
    if (!targets.length || !this.dir) return;
    setClipboard({
      mode,
      dir: this.dir,
      items: targets.map((e) => ({ path: e.path, name: e.name, is_dir: e.is_dir })),
    });
    // The paths go on the OS clipboard as text too, so Ctrl+V in a terminal
    // pastes them. Best effort: a denied clipboard does not stop the copy.
    void navigator.clipboard?.writeText(targets.map((e) => e.path).join("\n")).catch(() => {});
  }

  private async copyPaths(): Promise<void> {
    const paths = this.targets().map((e) => e.path);
    if (!paths.length) return;
    try {
      await navigator.clipboard.writeText(paths.join("\n"));
    } catch (err) {
      this.say(String(err), true);
    }
  }

  private async paste(): Promise<void> {
    const clip = getClipboard();
    const dest = this.dir;
    if (!clip || !dest || this.busy) return;
    // NFC-only, as rule 15 prescribes for the frontend: both strings came
    // from ymux's own listings. A respelling that slips through reaches the
    // backend, which refuses a same-path copy and treats a same-path move
    // as a no-op.
    const sameDir = clip.dir.normalize("NFC") === dest.normalize("NFC");
    if (clip.mode === "cut" && sameDir) return;

    const pasted: string[] = [];
    const known: FileEntry[] = [...this.entries];
    let remembered: OverwriteChoice | null = null;
    await this.withBusy(async () => {
      items: for (let k = 0; k < clip.items.length; k++) {
        const item = clip.items[k];
        // This item's answer, kept across a retry so the user is asked once.
        let chosen: OverwriteChoice | null = null;
        // Retried on `already_exists`: the listing can miss a conflict (a
        // hidden file while hidden files are off, or one created since the
        // last listing), and the backend is the authority that catches it.
        // The file it found joins `known`, so the next try prompts or picks
        // a free "(n)" name.
        for (let attempt = 0; attempt < 3; attempt++) {
          const conflict = findConflict(item.name, known);
          let choice: OverwriteChoice = "skip";
          if (conflict) {
            if (clip.mode === "copy" && sameDir) choice = "keep-both";
            else if (chosen) choice = chosen;
            else if (remembered) choice = remembered;
            else {
              const ans = await this.askOverwrite(
                item.is_dir,
                conflict,
                k < clip.items.length - 1,
              );
              if (!ans) break items;
              choice = ans.choice;
              if (ans.all) remembered = choice;
            }
            chosen = choice;
          }
          const plan = resolveOverwrite(
            item.name,
            item.is_dir,
            conflict,
            choice,
            known.map((e) => e.name),
          );
          if (plan.action === "skip") continue items;
          const to = joinPath(dest, plan.name);
          try {
            if (clip.mode === "copy") await fsApi.copy(item.path, to, plan.overwrite);
            else await fsApi.move(item.path, to, plan.overwrite);
          } catch (err) {
            if (errorKind(err) === "already_exists" && !plan.overwrite) {
              const hit = await fsApi.stat(to).catch(() => null);
              if (hit && !findConflict(hit.name, known)) {
                known.push(hit);
                continue;
              }
            }
            this.say(`${item.name}: ${describeFsError(err)}`, true);
            continue items;
          }
          pasted.push(plan.name);
          if (!plan.overwrite) {
            known.push({ ...item, name: plan.name, path: to, is_symlink: false, size: 0, modified_ms: 0 });
          }
          continue items;
        }
      }
    });
    this.focus();
    if (clip.mode === "cut" && pasted.length) setClipboard(null);
    await this.load({ select: pasted });
  }

  private async askOverwrite(
    srcIsDir: boolean,
    dest: FileEntry,
    many: boolean,
  ): Promise<{ choice: OverwriteChoice; all: boolean } | null> {
    const replaceable = canReplace(srcIsDir, dest);
    const choices = [
      ...(replaceable ? [{ id: "replace", label: t("files.replace") }] : []),
      { id: "skip", label: t("files.skip") },
      { id: "keep-both", label: t("files.keepBoth"), primary: true },
    ];
    const ans = await askChoice(
      fill(t("files.conflict"), { name: dest.name }),
      replaceable ? null : t("files.conflictDir"),
      choices,
      many ? t("files.applyToAll") : undefined,
    );
    if (!ans) return null;
    return { choice: ans.id as OverwriteChoice, all: ans.checked };
  }

  private toggleHidden(): void {
    this.prefs = { ...this.prefs, showHidden: !this.prefs.showHidden };
    writePrefs(this.prefs);
    this.applyPrefs();
    void this.load({});
  }

  private togglePreview(): void {
    this.prefs = { ...this.prefs, showPreview: !this.prefs.showPreview };
    writePrefs(this.prefs);
    this.applyPrefs();
    this.schedulePreview();
    requestAnimationFrame(() => this.renderRows());
  }

  private applyPrefs(): void {
    this.element.classList.toggle("files-pane--no-preview", !this.prefs.showPreview);
    this.hiddenBtn.setAttribute("aria-pressed", String(this.prefs.showHidden));
    this.previewBtn.setAttribute("aria-pressed", String(this.prefs.showPreview));
  }

  // ── Path editing (Ctrl+L, or double-click the breadcrumb) ────────────────

  private editPath(): void {
    this.pathInput.value = this.dir ?? "";
    this.crumbsEl.hidden = true;
    this.pathInput.hidden = false;
    this.pathInput.focus();
    this.pathInput.select();
  }

  private endEditPath(): void {
    this.pathInput.hidden = true;
    this.crumbsEl.hidden = false;
  }

  private onPathKey(ev: KeyboardEvent): void {
    if (ev.isComposing || ev.keyCode === 229) return;
    if (ev.key === "Escape") {
      ev.preventDefault();
      this.endEditPath();
      this.focus();
      return;
    }
    if (ev.key !== "Enter") return;
    ev.preventDefault();
    let typed = this.pathInput.value.trim();
    this.endEditPath();
    this.focus();
    if (!typed) return;
    const absolute = /^([A-Za-z]:|\\\\|\/)/.test(typed);
    if (typed === "~" || typed.startsWith("~/") || typed.startsWith("~\\")) {
      void fsApi.homeDir().then((home) => this.navigate(typed === "~" ? home : joinPath(home, typed.slice(2))));
      return;
    }
    if (!absolute && this.dir) typed = joinPath(this.dir, typed);
    this.navigate(typed);
  }

  // ── Context menu ──────────────────────────────────────────────────────────

  private showMenu(ev: MouseEvent): void {
    const onRow = this.rowIndexAt(ev.target) !== null;
    const targets = onRow ? this.targets() : [];
    const one = targets.length === 1 ? targets[0] : null;
    const clip = getClipboard();
    const dir = this.dir;
    const entries: ContextMenuEntry[] = [];
    if (one) {
      entries.push({ label: t("files.open"), onSelect: () => void this.openAt(this.sel.cursor) });
      if (!one.is_dir) {
        entries.push({ label: t("files.openDefault"), onSelect: () => void this.openDefault(one) });
      }
      entries.push({
        label: t("files.reveal"),
        onSelect: () => void fsApi.reveal(one.path).catch((e) => this.say(describeFsError(e), true)),
      });
    }
    if (this.opts.openTerminal && dir) {
      const at = one?.is_dir ? one.path : dir;
      entries.push({ label: t("files.openTerminal"), onSelect: () => void this.opts.openTerminal?.(at) });
    }
    if (entries.length) entries.push("separator");
    if (targets.length) {
      entries.push(
        { label: t("files.cut"), onSelect: () => this.toClipboard("cut") },
        { label: t("menu.copy"), onSelect: () => this.toClipboard("copy") },
      );
    }
    entries.push({ label: t("menu.paste"), disabled: !clip, onSelect: () => void this.paste() });
    entries.push("separator");
    if (one) entries.push({ label: t("files.rename"), onSelect: () => void this.rename() });
    if (targets.length) {
      entries.push(
        { label: t("files.trash"), onSelect: () => void this.remove(false) },
        { label: t("files.deletePermanent"), onSelect: () => void this.remove(true) },
        { label: t("files.copyPath"), onSelect: () => void this.copyPaths() },
      );
    } else {
      entries.push(
        { label: t("files.newFolder"), onSelect: () => void this.newFolder() },
        { label: t("files.newFile"), onSelect: () => void this.newFile() },
        { label: t("files.refresh"), onSelect: () => void this.refresh() },
      );
    }
    showContextMenu(ev.clientX, ev.clientY, entries);
  }

  private makeButton(icon: string, key: string, onClick: () => void): HTMLButtonElement {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "files__btn";
    b.tabIndex = -1;
    b.innerHTML = icon;
    b.title = t(key);
    // Keep keyboard focus in the list: a toolbar click is not a place to
    // leave the caret.
    b.addEventListener("mousedown", (ev) => ev.preventDefault());
    // Focus first: an action that opens a dialog focuses its input
    // synchronously, and refocusing the list after it would steal that.
    b.addEventListener("click", () => {
      this.focus();
      onClick();
    });
    this.buttons.push({ el: b, key });
    return b;
  }
}
