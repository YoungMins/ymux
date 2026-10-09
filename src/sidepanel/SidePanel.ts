// Right-side panel: ONE app-wide aside (not part of any layout tree, so never
// saved) with a VS Code-style activity switcher on top choosing between two
// views, both always mounted so each keeps its state across switches:
//
//  - "files": the Files pane (src/files/FilesPane.ts), following the active
//    pane's working directory. It is hosted directly — no PTY, no yipc
//    round-trip (spec §2.4): a cwd change goes through `CwdFollow`'s debounce
//    and dedupe and then straight into `FilesPane.navigate`.
//  - "usage": the AI token usage pane (src/tokenmonitor/TokenMonitor.ts).

import type { UnlistenFn } from "@tauri-apps/api/event";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import { FilesPane } from "../files/FilesPane";
import { CwdFollow } from "../filedock/cwdFollow";
import {
  clampDockWidth,
  migrateDockState,
  nextDockState,
  toggleDockOpen,
  parseDockState,
  serializeDockState,
  type DockState,
  type DockView,
} from "../filedock/dockModel";
import { api, onPaneCwd } from "../ipc/bridge";
import { onLangChange, t } from "../i18n/i18n";
import { TokenMonitorPane } from "../tokenmonitor/TokenMonitor";
import "./sidepanel.css";

const STORAGE_KEY = "ymux.sidePanel";
/// The two docks this panel replaced; read once to seed the first run.
const LEGACY_FILE_KEY = "ymux.fileDock";
const LEGACY_TOKEN_KEY = "ymux.tokenDock";

/// The files pane's id. Not a PTY id, only the `data-pane-id` its element
/// carries; kept out of the UUID space so no layout pane can clash.
const FILES_ID = "file-dock";

const ICONS: Record<DockView, string> = {
  files: `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>`,
  usage: `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 20V10m8 10V4m8 16v-7"/></svg>`,
};
const TITLE_KEYS: Record<DockView, string> = { files: "files.title", usage: "usage.title" };
const VIEWS: DockView[] = ["files", "usage"];

function readItem(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function readState(): DockState {
  const stored = readItem(STORAGE_KEY);
  if (stored !== null) return parseDockState(stored);
  const migrated = migrateDockState(readItem(LEGACY_FILE_KEY), readItem(LEGACY_TOKEN_KEY));
  writeState(migrated);
  return migrated;
}

function writeState(state: DockState): void {
  try {
    localStorage.setItem(STORAGE_KEY, serializeDockState(state));
  } catch {
    /* localStorage unavailable: the panel just won't persist */
  }
}

class SidePanel {
  readonly element = document.createElement("aside");
  private readonly files: FilesPane;
  private readonly usage = new TokenMonitorPane({ id: "token-dock", ownChrome: false });
  private readonly viewEls = {} as Record<DockView, HTMLElement>;
  private readonly tabs = {} as Record<DockView, HTMLButtonElement>;
  private readonly closeBtn = document.createElement("button");
  private state = readState();
  private cwdUnlisten: UnlistenFn | null = null;
  /// Bumped on every follow re-subscription, so a slow, superseded one
  /// unlistens itself instead of leaking.
  private followGen = 0;
  /// `follow`, not `navigate`: held while the user is typing in the list.
  private readonly follow = new CwdFollow((dir) => this.files.follow(dir));

  constructor(private readonly manager: WorkspaceManager) {
    this.element.className = "side-panel";
    this.element.style.width = `${this.state.width}px`;

    const resizer = document.createElement("div");
    resizer.className = "side-panel__resizer";
    resizer.addEventListener("pointerdown", (ev) => this.startResize(resizer, ev));

    const bar = document.createElement("div");
    bar.className = "side-panel__tabs";
    const tablist = document.createElement("div");
    tablist.className = "side-panel__tablist";
    tablist.setAttribute("role", "tablist");
    tablist.addEventListener("keydown", (ev) => this.onTabKey(ev));
    for (const view of VIEWS) {
      const tab = document.createElement("button");
      tab.type = "button";
      tab.className = "side-panel__tab";
      tab.id = `side-panel-tab-${view}`;
      tab.setAttribute("role", "tab");
      tab.setAttribute("aria-controls", `side-panel-view-${view}`);
      tab.innerHTML = ICONS[view];
      tab.addEventListener("click", () => this.apply(nextDockState(this.state, view, false), true));
      this.tabs[view] = tab;
      tablist.appendChild(tab);
    }
    bar.appendChild(tablist);
    this.closeBtn.type = "button";
    this.closeBtn.className = "side-panel__close";
    this.closeBtn.textContent = "×";
    this.closeBtn.addEventListener("click", () => this.apply({ ...this.state, open: false }, false));
    bar.appendChild(this.closeBtn);

    // No `onFocus`, deliberately (spec §3.7): the files pane must never
    // become the manager's focused pane, or "the pane active before the dock
    // took focus" — where Enter on a file opens its viewer tab — is lost.
    this.files = new FilesPane({
      id: FILES_ID,
      dir: null,
      docked: true,
      ownChrome: false,
      openFile: (path) => this.manager.openFileInViewerTab(path),
      openTerminal: (dir) => this.manager.splitTerminalAt(null, dir),
    });
    const panes: Record<DockView, HTMLElement> = {
      files: this.files.element,
      usage: this.usage.element,
    };
    this.element.append(resizer, bar);
    for (const view of VIEWS) {
      const wrap = document.createElement("div");
      wrap.className = "side-panel__view";
      wrap.id = `side-panel-view-${view}`;
      wrap.setAttribute("role", "tabpanel");
      wrap.setAttribute("aria-labelledby", `side-panel-tab-${view}`);
      wrap.appendChild(panes[view]);
      this.viewEls[view] = wrap;
      this.element.appendChild(wrap);
    }

    const translate = (): void => {
      for (const view of VIEWS) {
        const label = t(TITLE_KEYS[view]);
        this.tabs[view].title = label;
        this.tabs[view].setAttribute("aria-label", label);
      }
      const close = t("usage.close");
      this.closeBtn.title = close;
      this.closeBtn.setAttribute("aria-label", close);
      this.element.setAttribute("aria-label", t(TITLE_KEYS[this.state.view]));
    };
    translate();
    onLangChange(translate);
    this.render();

    manager.onActivePaneChange(() => void this.followActivePane());
  }

  /// Apply the persisted state. Call once the element is in the DOM.
  async start(): Promise<void> {
    void this.usage.spawn();
    // Open on the active pane's directory rather than home-then-jump.
    const id = this.manager.activePaneId();
    const cwd = id ? await api.getPaneCwd(id).catch(() => null) : null;
    if (cwd) this.files.navigate(cwd);
    this.follow.reset(cwd);
    await this.files.spawn();
    this.apply(this.state, false);
    void this.followActivePane();
  }

  private render(): void {
    const { open, view: current } = this.state;
    this.element.classList.toggle("side-panel--open", open);
    for (const view of VIEWS) {
      const active = view === current;
      this.viewEls[view].classList.toggle("side-panel__view--active", active);
      this.tabs[view].setAttribute("aria-selected", String(active));
      this.tabs[view].tabIndex = active ? 0 : -1;
    }
    this.element.setAttribute("aria-label", t(TITLE_KEYS[current]));
  }

  /// "Show `view`": `toggle` closes the panel when `view` is already showing.
  show(view: DockView, toggle: boolean): void {
    this.apply(nextDockState(this.state, view, toggle), true);
  }

  /// Open on the last-used view, or close.
  toggle(): void {
    this.apply(toggleDockOpen(this.state), true);
  }

  /// Arrow/Home/End move the selection (never close) and focus the new tab.
  private onTabKey(ev: KeyboardEvent): void {
    const i = VIEWS.indexOf(this.state.view);
    let to: number;
    if (ev.key === "ArrowRight") to = (i + 1) % VIEWS.length;
    else if (ev.key === "ArrowLeft") to = (i - 1 + VIEWS.length) % VIEWS.length;
    else if (ev.key === "Home") to = 0;
    else if (ev.key === "End") to = VIEWS.length - 1;
    else return;
    ev.preventDefault();
    this.apply(nextDockState(this.state, VIEWS[to], false), false);
    this.tabs[VIEWS[to]].focus();
  }

  private apply(next: DockState, focus: boolean): void {
    const hadFocus = this.element.contains(document.activeElement);
    const wasOpen = this.state.open;
    this.state = next;
    this.render();
    writeState(next);
    if (next.open) {
      // Shown after `display: none` (panel or view): re-measure and list
      // whatever the follow queued while hidden, refresh usage (rule 14's
      // "shown again" path — never a raw fit).
      const pane = next.view === "files" ? this.files : this.usage;
      pane.scheduleFit();
      if (focus) pane.focus();
    } else if (wasOpen && hadFocus) {
      this.manager.focusActivePane();
    }
    // The workspace area changed width only if the panel opened or closed.
    if (wasOpen !== next.open) requestAnimationFrame(() => this.manager.refitActive());
  }

  /// Re-point the cwd subscription at the active pane and hand its current
  /// dir to CwdFollow. Runs while the panel is hidden too; the pane only
  /// records the directory then and lists it when shown.
  private async followActivePane(): Promise<void> {
    const gen = ++this.followGen;
    this.cwdUnlisten?.();
    this.cwdUnlisten = null;
    const id = this.manager.activePaneId();
    if (!id) {
      this.follow.activePaneChanged(null, null);
      return;
    }
    const unlisten = await onPaneCwd(id, (cwd) => this.follow.cwdChanged(id, cwd)).catch(
      () => null,
    );
    const cwd = await api.getPaneCwd(id).catch(() => null);
    if (gen !== this.followGen) {
      unlisten?.();
      return;
    }
    this.cwdUnlisten = unlisten;
    this.follow.activePaneChanged(id, cwd);
  }

  private startResize(handle: HTMLElement, ev: PointerEvent): void {
    if (ev.button !== 0) return;
    ev.preventDefault();
    handle.setPointerCapture(ev.pointerId);
    const container = this.element.parentElement;
    const onMove = (e: PointerEvent): void => {
      const right = container?.getBoundingClientRect().right ?? window.innerWidth;
      const width = clampDockWidth(right - e.clientX, container?.clientWidth ?? window.innerWidth);
      this.state = { ...this.state, width };
      this.element.style.width = `${width}px`;
    };
    const onUp = (): void => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      writeState(this.state);
      (this.state.view === "files" ? this.files : this.usage).scheduleFit();
      requestAnimationFrame(() => this.manager.refitActive());
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  }
}

let instance: SidePanel | null = null;

/// Create the panel inside `parent` (the `.app-body` row, after
/// `.workspace-host`). Call after `manager.start()`, so the active pane
/// exists and its directory is known.
export function mountSidePanel(parent: HTMLElement, manager: WorkspaceManager): void {
  instance = new SidePanel(manager);
  parent.appendChild(instance.element);
  void instance.start();
}

/// Toolbar button: open on the last-used view, or close. A no-op before
/// `mountSidePanel`.
export function toggleSidePanel(): void {
  instance?.toggle();
}

/// Files view: open it, or close the panel if it is already showing. A no-op
/// before `mountSidePanel`.
export function toggleFileDock(): void {
  instance?.show("files", true);
}

/// Usage view, same toggle semantics.
export function toggleTokenDock(): void {
  instance?.show("usage", true);
}

/// Usage view, never closes.
export function openTokenDock(): void {
  instance?.show("usage", false);
}
