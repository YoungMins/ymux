// Right-side file dock: one app-wide files pane (src/files/FilesPane.ts),
// not part of any layout tree (so it is never saved), following the active
// pane's working directory. It hosts the pane directly — there is no PTY,
// no `ydir --dock` process and no yipc round-trip (spec §2.4): a cwd change
// goes through `CwdFollow`'s debounce and dedupe and then straight into
// `FilesPane.navigate`.

import type { UnlistenFn } from "@tauri-apps/api/event";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import { FilesPane } from "../files/FilesPane";
import { api, onOpenFile, onPaneCwd } from "../ipc/bridge";
import { CwdFollow } from "./cwdFollow";
import {
  clampDockWidth,
  parseDockState,
  serializeDockState,
  type DockState,
} from "./dockModel";

const STORAGE_KEY = "ymux.fileDock";

/// The dock's pane id. Not a PTY id any more, only the `data-pane-id` its
/// element carries; kept out of the UUID space so no layout pane can clash.
const DOCK_ID = "file-dock";

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
  private readonly pane: FilesPane;
  private state = readState();
  private cwdUnlisten: UnlistenFn | null = null;
  /// Bumped on every follow re-subscription, so a slow, superseded one
  /// unlistens itself instead of leaking.
  private followGen = 0;
  /// `follow`, not `navigate`: held while the user is typing in the list.
  private readonly follow = new CwdFollow((dir) => this.pane.follow(dir));

  constructor(private readonly manager: WorkspaceManager) {
    this.element = document.createElement("div");
    this.element.className = "file-dock";
    this.element.style.width = `${this.state.width}px`;

    const resizer = document.createElement("div");
    resizer.className = "file-dock__resizer";
    resizer.addEventListener("pointerdown", (ev) => this.startResize(resizer, ev));
    this.element.appendChild(resizer);

    // No `onFocus`, deliberately (spec §3.7): the dock's pane must never
    // become the manager's focused pane, or "the pane active before the dock
    // took focus" — where Enter on a file opens its viewer tab — is lost.
    this.pane = new FilesPane({
      id: DOCK_ID,
      dir: null,
      docked: true,
      ownChrome: false,
      openFile: (path) => this.manager.openFileInViewerTab(path),
      openTerminal: (dir) => this.manager.splitTerminalAt(null, dir),
    });
    this.element.appendChild(this.pane.element);

    manager.onActivePaneChange(() => void this.followActivePane());
    // A `ydir --dock` still on PATH (and run by hand) can ask for a file to
    // be opened over yipc; that route goes in step 3 with the viewer tab.
    void onOpenFile((path) => {
      void this.manager.openFileInViewerTab(path);
    }).catch((e) => console.warn("open-file listen failed:", e));
  }

  /// Apply the persisted state. Call once the element is in the DOM.
  async start(): Promise<void> {
    // Open on the active pane's directory rather than home-then-jump.
    const id = this.manager.activePaneId();
    const cwd = id ? await api.getPaneCwd(id).catch(() => null) : null;
    if (cwd) this.pane.navigate(cwd);
    this.follow.reset(cwd);
    await this.pane.spawn();
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
      // Shown after `display: none`: re-measure and list whatever the
      // follow queued while hidden (rule 14's "shown again" path).
      this.pane.scheduleFit();
      if (focus) this.pane.focus();
    } else if (hadFocus) {
      this.manager.focusActivePane();
    }
    // The workspace area changed width: refit it like a window resize.
    requestAnimationFrame(() => this.manager.refitActive());
  }

  /// Re-point the cwd subscription at the active pane and hand its current
  /// dir to CwdFollow. Runs while the dock is hidden too; the pane only
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
      this.pane.scheduleFit();
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
/// exists and its directory is known.
export function mountFileDock(parent: HTMLElement, manager: WorkspaceManager): void {
  instance = new FileDock(manager);
  parent.appendChild(instance.element);
  void instance.start();
}

/// Show or hide the dock. A no-op before `mountFileDock`.
export function toggleFileDock(): void {
  instance?.toggle();
}
