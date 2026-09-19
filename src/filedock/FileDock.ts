// Right-side file dock: one app-wide yDir, in a normal TerminalPane that is
// not part of any layout tree (so it is never saved), following the active
// pane's working directory. Spawned lazily on first open and kept alive
// while hidden. When ydir exits or cannot start, a message and a Restart
// button take its place.

import type { UnlistenFn } from "@tauri-apps/api/event";
import type { Uuid } from "../types";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import { TerminalPane } from "../terminal/TerminalPane";
import { api, onPaneCwd } from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";
import { CwdFollow } from "./cwdFollow";
import {
  clampDockWidth,
  dockArgv,
  parseDockState,
  serializeDockState,
  type DockState,
} from "./dockModel";

/// The dock's PTY id (the spec's `__ydir_dock__`). It must be a UUID,
/// because every PTY command (`spawn_pane`, `write_pane`, `resize_pane`,
/// `kill_pane`) takes one.
export const DOCK_PANE_ID: Uuid = "00000000-0000-4000-8000-0000000d0c00";

const STORAGE_KEY = "ymux.fileDock";

type DockMessage = "filedock.exited" | "filedock.failed";

function readState(): DockState {
  try {
    return parseDockState(localStorage.getItem(STORAGE_KEY));
  } catch {
    return parseDockState(null);
  }
}

function writeState(state: DockState): void {
  try {
    localStorage.setItem(STORAGE_KEY, serializeDockState(state));
  } catch {
    /* localStorage unavailable: the dock just won't persist */
  }
}

class FileDock {
  readonly element: HTMLElement;
  private readonly body: HTMLElement;
  private pane: TerminalPane | null = null;
  private message: DockMessage | null = null;
  private starting = false;
  private state = readState();
  private cwdUnlisten: UnlistenFn | null = null;
  /// Bumped on every follow re-subscription, so a slow, superseded one
  /// unlistens itself instead of leaking.
  private followGen = 0;
  private readonly follow = new CwdFollow((dir) => {
    void api.fileDockChangeDir(dir).catch((e) =>
      console.warn("fileDockChangeDir failed:", e),
    );
  });

  constructor(private readonly manager: WorkspaceManager) {
    this.element = document.createElement("div");
    this.element.className = "file-dock";
    this.element.style.width = `${this.state.width}px`;

    const resizer = document.createElement("div");
    resizer.className = "file-dock__resizer";
    resizer.addEventListener("pointerdown", (ev) => this.startResize(resizer, ev));
    this.element.appendChild(resizer);

    this.body = document.createElement("div");
    this.body.className = "file-dock__body";
    this.element.appendChild(this.body);

    manager.onActivePaneChange(() => void this.followActivePane());
    window.addEventListener("resize", () => {
      if (this.state.open) this.pane?.scheduleFit();
    });
    onLangChange(() => {
      if (this.message) this.showMessage(this.message);
    });
  }

  /// Apply the persisted state. Call once the element is in the DOM.
  start(): void {
    if (this.state.open) this.setOpen(true, false);
    void this.followActivePane();
  }

  toggle(): void {
    this.setOpen(!this.state.open, true);
  }

  private setOpen(open: boolean, focus: boolean): void {
    const hadFocus = this.element.contains(document.activeElement);
    this.state = { ...this.state, open };
    this.element.classList.toggle("file-dock--open", open);
    writeState(this.state);
    if (open) {
      if (this.pane) {
        this.pane.scheduleFit();
        if (focus) this.pane.focus();
      } else if (!this.message) {
        void this.startYdir(focus);
      }
    } else if (hadFocus) {
      this.manager.focusActivePane();
    }
    // The workspace area changed width: refit it like a window resize.
    requestAnimationFrame(() => this.manager.refitActive());
  }

  private async startYdir(focus: boolean): Promise<void> {
    if (this.starting) return;
    this.starting = true;
    this.message = null;
    this.body.replaceChildren();
    // A previous session may still be being killed: `dispose` fires
    // `killPane` without awaiting, and Tauri commands run on a worker pool.
    // Serialize it here, or a late kill lands on the pane about to spawn
    // under the same reserved id. It is a no-op error when nothing is there.
    await api.killPane(DOCK_PANE_ID).catch(() => {});
    const activeId = this.manager.activePaneId();
    const cwd = activeId ? await api.getPaneCwd(activeId).catch(() => null) : null;
    const pane = new TerminalPane({
      spec: { id: DOCK_PANE_ID, title: "yDir", shell: "ydir", cwd, env: [] },
      argv: dockArgv(cwd),
      fontSize: this.manager.fontSize,
      persistScrollback: () => false,
      onExit: () => this.showMessage("filedock.exited"),
    });
    this.pane = pane;
    this.body.appendChild(pane.element);
    this.follow.reset(cwd);
    try {
      await pane.spawn();
      if (focus) pane.focus();
    } catch {
      this.showMessage("filedock.failed");
    } finally {
      this.starting = false;
    }
  }

  private showMessage(key: DockMessage): void {
    this.pane?.dispose(false);
    this.pane = null;
    this.message = key;

    const box = document.createElement("div");
    box.className = "file-dock__message";
    const text = document.createElement("p");
    text.textContent = t(key);
    const restart = document.createElement("button");
    restart.type = "button";
    restart.className = "file-dock__restart";
    restart.textContent = t("filedock.restart");
    restart.addEventListener("click", () => void this.startYdir(true));
    box.append(text, restart);
    this.body.replaceChildren(box);
  }

  /// Re-point the cwd subscription at the active pane and hand its current
  /// dir to CwdFollow. Runs while the dock is hidden too, which keeps a
  /// running ydir in step and costs one small IPC message.
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
      this.pane?.scheduleFit();
      requestAnimationFrame(() => this.manager.refitActive());
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  }
}

let instance: FileDock | null = null;

/// Create the dock inside `parent` (the `.app-body` row, after
/// `.workspace-host`). Call after `manager.start()`, so the active pane
/// exists and a restored-open dock spawns after the workspace's own panes.
export function mountFileDock(parent: HTMLElement, manager: WorkspaceManager): void {
  instance = new FileDock(manager);
  parent.appendChild(instance.element);
  instance.start();
}

/// Show or hide the dock. A no-op before `mountFileDock`.
export function toggleFileDock(): void {
  instance?.toggle();
}
