// Owns all workspaces, their layout trees, and per-workspace pane caches.
// Switching workspaces hides the previous DOM subtree without disposing any
// xterm instances, so scrollback survives — the tmux semantics the user
// explicitly asked for.

import type {
  AgentSnapshot,
  Config,
  HotKeyDef,
  LayoutNode,
  PaneSpec,
  ShellProfile,
  SplitDir,
  Uuid,
  Workspace,
} from "../types";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { api } from "../ipc/bridge";
import { TerminalPane } from "../terminal/TerminalPane";
import { clampFontSize, DEFAULT_FONT_SIZE } from "./fontSize";
import { BrowserPane } from "../browser/BrowserPane";
import { EmbeddedBrowserPane } from "../browser/EmbeddedBrowserPane";
import { FilesPane } from "../files/FilesPane";
import { baseName } from "../files/fileModel";
import type { Pane } from "../layout/Pane";
import {
  findAndMutatePane,
  findPane,
  newPane,
  paneNode,
  panes,
  removePane,
  setRatioByPath,
  splitPane,
  swapPanes,
  worktreePaths,
} from "../layout/LayoutTree";
import { uuidv4 } from "../types";
import { PaneGroup } from "../layout/PaneGroup";
import { tabLabel } from "../terminal/tabLabel";
import {
  activateTabFor,
  addTab,
  findGroup,
  groupOfPane,
  isVisibleTab,
  paneGroups,
  stepTab,
  swappablePanes,
  tabIds,
  visiblePanes,
  wrapInTabs,
} from "../layout/tabs";
import { render, type RenderContext } from "../layout/SplitContainer";
import { beep } from "../util/beep";
import { t } from "../i18n/i18n";
import { promptWorktreeBranch } from "./WorktreeModal";
import { askConfirm, askText } from "../ui/Dialog";
import { showContextMenu, type ContextMenuEntry } from "../menu/ContextMenu";
import { moveItem } from "./reorder";
import { newlyWaitingPanes, workspaceIdOfPane } from "./agentTree";
import { pickActivePaneId } from "./activePane";
import { viewerTabAction } from "./viewerTab";
import { zoomAction } from "./zoom";
import type { PaneStatus } from "../terminal/paneStatus";

const MAX_WORKSPACES = 9;

/// Companion tools offered in the terminal right-click menu. These ship as
/// sidecars next to ymux.exe and the installer puts that directory on PATH,
/// so the command is all that's needed. Names are proper nouns — not i18n'd.
const TOOL_MENU: { label: string; command: string }[] = [
  { label: "yDir", command: "ydir" },
  { label: "yMon", command: "ymon" },
  { label: "yCode", command: "ycode" },
  { label: "yGit", command: "ygit" },
];

export class WorkspaceManager {
  private config: Config;
  private shells: ShellProfile[];
  private paneCaches = new Map<number, Map<Uuid, Pane>>();
  /// Tab-group chrome per workspace, keyed by `LayoutNode::Tabs.id`. Mirrors
  /// `paneCaches`: `SplitContainer.render()` rebuilds the DOM on every layout
  /// mutation, and a `PaneGroup` owns a HotKeyBar with a language
  /// subscription, so it must survive those rebuilds.
  private groupCaches = new Map<number, Map<Uuid, PaneGroup>>();
  /// Group id → the pane id of its viewer tab, the one the file dock's
  /// `open-file` reuses (spec §4). Runtime-only and deliberately not
  /// persisted: after a restart that tab reloads as an ordinary `ycode` tab
  /// and the next dock Enter registers a new one.
  private viewerTabs = new Map<Uuid, Uuid>();
  /// Workspace id → the pane zoomed in it (`Ctrl+Shift+Z`); absent when that
  /// workspace is not zoomed. Zoom is CSS plus one re-parenting, both of
  /// which `renderWorkspace` throws away, so it is re-applied after every
  /// render from this — see `applyZoom` and `./zoom.ts`.
  private zoomedPanes = new Map<number, Uuid>();
  /// Latest `panes:labels` snapshot (pane id → deepest running program).
  private _paneLabels: Record<Uuid, string> = {};
  private workspaceContainers = new Map<number, HTMLElement>();
  private activeId: number;
  // Backing field — DO NOT read/write directly. Go through the
  // `focusedPaneId` accessor below so the `.pane--focused` CSS class stays
  // in sync. We need the explicit class because browser panes' OS-level
  // child webviews own keyboard focus, which means CSS `:focus-within`
  // never activates on the `.pane` DOM element.
  private _focusedPaneId: Uuid | null = null;
  private saveTimer: number | null = null;
  /// Cache of workspace containers that have already had their panes spawned
  /// on first visit, so subsequent visits are zero-cost.
  private hydrated = new Set<number>();
  /// Notified whenever the *set* of workspaces changes (add / delete / lazy
  /// creation). The workspace bar registers here to rebuild its tab list.
  private onWorkspacesChangeCb: (() => void) | null = null;
  /// Whether the app window currently has focus. When it's unfocused, every
  /// bell alerts (the user can't be watching any pane).
  private windowFocused = true;
  /// Per-pane derived status (idle/running/done/attention), driven by each
  /// TerminalPane's `PaneStatusMachine`. Backs both the pane's border colour
  /// and the workspace tab's status dot. Browser panes never appear here.
  paneStatus = new Map<Uuid, PaneStatus>();
  /// Notified with the owning workspace id whenever a pane's status changes,
  /// so the workspace bar can re-colour that workspace's tab dot.
  onPaneStatusChange?: (workspaceId: number) => void;
  /// Notified when the default shell changes, so whichever of the two UIs
  /// (toolbar picker / Settings) didn't make the change can follow along.
  onDefaultShellChange?: (name: string) => void;
  /// Fired after `setFontSize` so an open Settings panel can follow along.
  onFontSizeChange?: (px: number) => void;
  /// Latest agent-tree snapshot from the backend (pane id → agents).
  private _agents: AgentSnapshot = {};
  /// Tree listeners (the workspace panel), fired when agents change or any
  /// layout/pane metadata changes. A set so other views can subscribe too.
  private treeListeners = new Set<() => void>();
  /// Notified when the pane the user works in may have changed: focus moved
  /// to another pane, or a workspace switch. No payload; read
  /// `activePaneId()`. The file dock subscribes.
  private activePaneListeners = new Set<() => void>();

  constructor(
    private host: HTMLElement,
    config: Config,
    shells: ShellProfile[],
  ) {
    this.config = config;
    this.shells = shells;
    this.activeId = config.active_workspace;
    this.orderShellsByDefault();
  }

  /// Active pane within the active workspace. Setting this toggles the
  /// `.pane--focused` CSS class on the matching `.pane` element so browser
  /// panes (whose OS-level child webview owns keyboard focus, defeating
  /// `:focus-within`) still show the focus border.
  private get focusedPaneId(): Uuid | null {
    return this._focusedPaneId;
  }

  private set focusedPaneId(id: Uuid | null) {
    if (this._focusedPaneId === id) return;
    if (this._focusedPaneId !== null) {
      // Tell the outgoing pane it lost focus so its status machine can clear
      // a pending done/attention flag on refocus-elsewhere. Only TerminalPane
      // implements blur()/status — browser panes don't track a status.
      const prev = this.findPaneById(this._focusedPaneId);
      if (prev instanceof TerminalPane) prev.blur();
      this.host
        .querySelector<HTMLElement>(`.pane[data-pane-id="${this._focusedPaneId}"]`)
        ?.classList.remove("pane--focused");
    }
    this._focusedPaneId = id;
    if (id !== null) {
      const el = this.host.querySelector<HTMLElement>(
        `.pane[data-pane-id="${id}"]`,
      );
      el?.classList.add("pane--focused");
    }
    this.notifyActivePaneChange();
  }

  get allShells(): ShellProfile[] {
    return this.shells;
  }

  get active(): Workspace {
    return (
      this.config.workspaces.find((w) => w.id === this.activeId) ??
      this.config.workspaces[0]
    );
  }

  get activeIdValue(): number {
    return this.activeId;
  }

  get workspaces(): Workspace[] {
    return this.config.workspaces;
  }

  /// Mount the initial workspace and pre-create empty containers for the
  /// others. Panes are lazily spawned when a workspace is first activated.
  async start(): Promise<void> {
    for (const ws of this.config.workspaces) {
      const el = document.createElement("div");
      el.className = "workspace";
      el.dataset.workspaceId = String(ws.id);
      el.style.display = "none";
      el.style.flex = "1 1 auto";
      this.host.appendChild(el);
      this.workspaceContainers.set(ws.id, el);
      this.paneCaches.set(ws.id, new Map());
    }

    // Authoritative focus tracking. We listen at the host (workspace area)
    // level instead of relying on per-pane handlers because xterm.js mounts
    // a hidden helper textarea + canvases as descendants of `.pane`, and
    // `focus` does not bubble — so a `focus` listener directly on `.pane`
    // never fires when xterm steals input focus into its own elements.
    //
    // Two signals, both at host level so they can't be defeated by a
    // descendant calling `stopPropagation()`:
    //
    //   1. `focusin` — bubbles, fires for any descendant focus. Catches the
    //      xterm textarea focus path naturally.
    //   2. `pointerdown` in the **capture** phase — runs before xterm.js
    //      gets a chance to handle the click. We use this to *forcefully*
    //      call `pane.focus()` on the clicked `.pane`, which guarantees
    //      both DOM focus and `term.focus()` even if xterm later rearranges
    //      things underneath us.
    const handlePaneActivation = (target: EventTarget | null, forceFocus: boolean) => {
      const el = target as HTMLElement | null;
      if (!el) return;
      const paneEl = el.closest<HTMLElement>(".pane[data-pane-id]");
      const id = paneEl?.dataset.paneId;
      if (!id) return;
      this.focusedPaneId = id;
      if (forceFocus) {
        // Don't steal focus from text inputs inside panes (browser URL bar,
        // search bar, hotkey modal inputs). Buttons are fine — the click
        // handler runs regardless and terminals should regain focus.
        if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
          return;
        }
        const cache = this.paneCaches.get(this.activeId);
        const pane = cache?.get(id);
        if (pane) pane.focus();
      }
    };
    this.host.addEventListener("focusin", (ev) => handlePaneActivation(ev.target, false));
    this.host.addEventListener(
      "pointerdown",
      (ev) => handlePaneActivation(ev.target, true),
      true, // capture phase: run before xterm.js's own handlers
    );

    // Track window focus so bell alerts can be suppressed only when the user is
    // actually looking at the completing pane.
    this.windowFocused = document.hasFocus();
    window.addEventListener("focus", () => {
      this.windowFocused = true;
    });
    window.addEventListener("blur", () => {
      this.windowFocused = false;
    });

    // Clicks on an embedded child webview (EmbeddedBrowserPane) bypass the
    // main webview's DOM entirely. The child's initialization_script (see
    // embedded_browser.rs) catches its own `pointerdown` / `focus` and
    // invokes `child_webview_focused` with its pane id; Rust re-emits
    // `ymux:child-focused`, which we listen for here. The id comes from
    // the clicked pane itself — no cursor mapping involved, reliable with
    // any number of browser panes. The child gets no capability at all:
    // Tauri injects `invoke` regardless, and Rust's `guard_embedded_child`
    // only lets a live `eb-<uuid>` webview report its *own* pane id.
    void tauriListen<string>("ymux:child-focused", (ev) => {
      const id = ev.payload;
      if (!id) return;
      const spec = this.getPaneSpec(id);
      if (!spec) return;
      if (
        spec.pane_kind === "embedded_browser" ||
        spec.pane_kind === "native_browser" ||
        spec.pane_kind === "browser"
      ) {
        this.focusedPaneId = id;
      }
    }).catch((e) => console.warn("listen ymux:child-focused failed:", e));

    await this.activate(this.activeId);
  }

  /// Switch to workspace `id`, creating it lazily if it doesn't exist yet.
  /// There is no upper bound — `Ctrl+Alt+1..9` cover the first nine, the
  /// toolbar `+` button reaches the rest.
  async activate(id: number): Promise<void> {
    if (id < 1) return;
    let created = false;
    if (!this.workspaceContainers.has(id)) {
      created = true;
      // New workspace on demand: seed an empty pane with the default shell.
      const defaultShell = this.shells[0]?.name ?? "";
      const ws: Workspace = {
        id,
        name: `workspace-${id}`,
        root: {
          kind: "pane",
          id: newPane(defaultShell).id,
          title: null,
          shell: defaultShell,
          cwd: null,
          startup_cmd: null,
          env: [],
          pane_kind: "terminal",
          url: null,
          hotkeys: [],
        },
      };
      this.config.workspaces.push(ws);
      const el = document.createElement("div");
      el.className = "workspace";
      el.dataset.workspaceId = String(id);
      el.style.display = "none";
      el.style.flex = "1 1 auto";
      this.host.appendChild(el);
      this.workspaceContainers.set(id, el);
      this.paneCaches.set(id, new Map());
    }

    // Hide current.
    const current = this.workspaceContainers.get(this.activeId);
    if (current) current.style.display = "none";

    this.activeId = id;
    this.config.active_workspace = id;

    const next = this.workspaceContainers.get(id)!;
    next.style.display = "flex";

    const ws = this.active;

    if (!this.hydrated.has(id)) {
      this.hydrated.add(id);
      await this.hydrateWorkspace(ws);
    } else {
      this.renderWorkspace(ws);
    }

    // Re-fit everything now that the container is visible.
    const cache = this.paneCaches.get(id)!;
    for (const pane of cache.values()) pane.scheduleFit();

    // A workspace switch changes the active pane even when the focused id
    // (still pointing into the old workspace) does not.
    this.notifyActivePaneChange();
    void api.setActiveWorkspace(id).catch(() => {});
    if (created) this.onWorkspacesChangeCb?.();
    this.persistDebounced();
  }

  /// Register a callback fired whenever the set of workspaces changes. Used by
  /// the workspace bar to rebuild its tab list.
  onWorkspacesChange(cb: () => void): void {
    this.onWorkspacesChangeCb = cb;
  }

  /// Reorder the workspace list: move the workspace at `fromIndex` so it lands
  /// before the one at `insertBefore` (`workspaces.length` = move to the end).
  /// Display order *is* `config.workspaces` order, which TOML's
  /// `[[workspaces]]` array preserves — so this needs no model change. Ids are
  /// untouched, so `Ctrl+Alt+N` keeps pointing at the workspace showing `N`.
  /// Returns whether anything actually moved.
  moveWorkspace(fromIndex: number, insertBefore: number): boolean {
    const next = moveItem(this.config.workspaces, fromIndex, insertBefore);
    if (!next) return false;
    // Mutate in place — the `workspaces` getter hands out this live array.
    this.config.workspaces.splice(0, this.config.workspaces.length, ...next);
    this.onWorkspacesChangeCb?.();
    this.persistDebounced();
    return true;
  }

  /// Lowest unused positive workspace id, so the bar numbering stays compact
  /// (e.g. with {1,3} present the next add reuses 2).
  private lowestFreeId(): number {
    const used = new Set(this.config.workspaces.map((w) => w.id));
    let id = 1;
    while (used.has(id)) id += 1;
    return id;
  }

  /// Create a new workspace at the lowest free id, seeded with one default
  /// terminal pane, and switch to it. Returns the new id.
  async addWorkspace(): Promise<number> {
    const id = this.lowestFreeId();
    await this.activate(id); // lazily creates the workspace + container
    return id;
  }

  /// Delete workspace `id`, disposing its panes (killing their PTYs). Refuses
  /// to delete the last remaining workspace. If the deleted workspace was
  /// active, switches to the first remaining one.
  async deleteWorkspace(id: number): Promise<void> {
    if (this.config.workspaces.length <= 1) return;
    const idx = this.config.workspaces.findIndex((w) => w.id === id);
    if (idx < 0) return;

    // Capture before the workspace is spliced out below — once it's gone,
    // its layout tree (and every pane's worktree_path) is gone with it.
    const wtPaths = worktreePaths(this.config.workspaces[idx].root);

    const cache = this.paneCaches.get(id);
    if (cache) {
      // Permanent: the whole workspace is being deleted, so any persisted
      // scrollback for its panes has no future mount to restore into.
      for (const pane of cache.values()) pane.dispose(true);
      for (const paneId of cache.keys()) this.paneStatus.delete(paneId);
    }
    this.paneCaches.delete(id);
    for (const group of this.groupCaches.get(id)?.values() ?? []) group.dispose();
    this.groupCaches.delete(id);
    this.workspaceContainers.get(id)?.remove();
    this.workspaceContainers.delete(id);
    this.hydrated.delete(id);
    this.config.workspaces.splice(idx, 1);

    if (this.activeId === id) {
      // The active container is gone; activate() skips hiding it (guarded) and
      // shows the neighbour instead.
      await this.activate(this.config.workspaces[0].id);
    }
    this.onWorkspacesChangeCb?.();
    this.persistDebounced();

    // The workspace is fully deleted at this point. Offer to remove each
    // pane's git worktree now that dispose(true) has released any OS-level
    // lock on the worktree directories (important on Windows). A removal
    // failure must never be surfaced as a delete failure.
    for (const wtPath of wtPaths) {
      await this.offerWorktreeRemoval(wtPath);
    }
  }

  /// Spawn PTYs for every pane in the workspace. Called exactly once the
  /// first time a workspace is activated in this session.
  private async hydrateWorkspace(ws: Workspace): Promise<void> {
    const specs = panes(ws.root);
    const cache = this.paneCaches.get(ws.id)!;
    for (const spec of specs) {
      const pane = this.createPane(spec);
      cache.set(spec.id, pane);
    }
    // Re-render now that panes exist in cache.
    this.renderWorkspace(ws);
    // Spawn shells / load iframes sequentially to avoid hammering the system.
    for (const pane of cache.values()) {
      try {
        await pane.spawn();
      } catch (e) {
        console.error(`spawn failed`, e);
      }
    }
    if (!this.focusedPaneId) {
      const first = visiblePanes(ws.root)[0];
      if (first) cache.get(first.id)?.focus();
    }
  }

  /// Build either a terminal or browser pane based on `spec.pane_kind`. All
  /// focus / hotkey / url change callbacks are wired so the manager can react
  /// to state changes without needing to know the pane subclass.
  /// `argv` runs a program directly instead of the spec's shell — the viewer
  /// tab uses it for `ycode <path>`, the same mechanism the file dock uses
  /// for `ydir`.
  private createPane(spec: PaneSpec, argv?: string[]): Pane {
    if (spec.pane_kind === "browser") {
      return new BrowserPane({
        spec,
        onFocus: () => {
          this.focusedPaneId = spec.id;
        },
        onUrlChange: (url) => {
          this.updatePaneSpec(spec.id, (p) => {
            p.url = url;
          });
        },
        onZoomRequested: () => {
          this.focusedPaneId = spec.id;
          this.toggleZoomFocused();
        },
      });
    }
    if (spec.pane_kind === "native_browser" || spec.pane_kind === "embedded_browser") {
      return new EmbeddedBrowserPane({
        spec,
        onFocus: () => {
          this.focusedPaneId = spec.id;
        },
        onUrlChange: (url) => {
          this.updatePaneSpec(spec.id, (p) => {
            p.url = url;
          });
        },
      });
    }
    if (spec.pane_kind === "files") {
      return new FilesPane({
        id: spec.id,
        dir: spec.cwd ?? null,
        title: spec.title ?? null,
        ownChrome: groupOfPane(this.active.root, spec.id) === null,
        onFocus: () => {
          this.focusedPaneId = spec.id;
        },
        onDirChange: (dir) => {
          this.updatePaneSpec(spec.id, (p) => {
            p.cwd = dir;
          });
          this.refreshTabChrome();
        },
        // The files pane is the focused pane when Enter is pressed, so the
        // viewer tab opens in its own group — beside the file list (§3.7).
        openFile: (path) => this.openFileInViewerTab(path),
        openTerminal: (dir) => this.splitTerminalAt(spec.id, dir),
      });
    }
    const resolvedShell = this.resolveShell(spec.shell);
    const finalSpec: PaneSpec = { ...spec, shell: resolvedShell };
    return new TerminalPane({
      spec: finalSpec,
      argv,
      ownChrome: groupOfPane(this.active.root, spec.id) === null,
      fontSize: this.fontSize,
      onFocus: () => {
        this.focusedPaneId = spec.id;
      },
      onAttention: (msg) => this.handleAttention(spec.id, msg),
      isVisible: () => this.isPaneVisible(spec.id),
      onContextMenu: (ev) => this.showPaneContextMenu(spec.id, ev),
      persistScrollback: () => this.persistScrollback,
      bottomAnchor: () => this.bottomAnchor,
      onStatusChange: (status) => {
        this.paneStatus.set(spec.id, status);
        this.applyPaneStatusClass(spec.id, status);
        const wsId = this.workspaceOfPane(spec.id);
        if (wsId !== null) this.onPaneStatusChange?.(wsId);
      },
      onHotKeysChange: (hotkeys) => {
        this.updatePaneSpec(spec.id, (p) => {
          p.hotkeys = hotkeys;
        });
      },
      onBgColorChange: (color) => {
        this.updatePaneSpec(spec.id, (p) => {
          p.bg_color = color ?? "";
        });
      },
    });
  }

  /// Mutate the stored PaneSpec for `id` via `patch`, then debounce-persist.
  /// Used by HotKey edits, browser URL changes, and future pane-metadata UIs.
  updatePaneSpec(id: Uuid, patch: (spec: PaneSpec) => void): void {
    for (const ws of this.config.workspaces) {
      const found = findAndMutatePane(ws.root, id, patch);
      if (found) {
        this.persistDebounced();
        return;
      }
    }
  }

  /// Look up the current PaneSpec snapshot for an id across all workspaces.
  getPaneSpec(id: Uuid): PaneSpec | null {
    for (const ws of this.config.workspaces) {
      const found = findPane(ws.root, id);
      if (found) return found;
    }
    return null;
  }

  /// Update the hotkeys for the currently focused terminal pane. Returns the
  /// new list for the caller to rebind its own UI to, or `null` if no pane is
  /// focused.
  setHotKeysForFocused(hotkeys: HotKeyDef[]): HotKeyDef[] | null {
    const id = this.focusedPaneId;
    if (!id) return null;
    this.updatePaneSpec(id, (p) => {
      p.hotkeys = hotkeys;
    });
    return hotkeys;
  }

  private renderWorkspace(ws: Workspace): void {
    const container = this.workspaceContainers.get(ws.id)!;
    const cache = this.paneCaches.get(ws.id)!;
    this.pruneGroups(ws);
    // A pane that has just been wrapped in a group must give up its own title
    // row and hotkey bar (the group draws one shared set); a pane whose group
    // has just unwrapped must get them back. Idempotent, so running it on
    // every render is both correct and cheap.
    for (const [paneId, pane] of cache) {
      const grouped = groupOfPane(ws.root, paneId) !== null;
      // A group that has just unwrapped left its last hidden tab carrying
      // `.pane--tab-hidden` (`display: none`), and the `PaneGroup` that would
      // have cleared it is already disposed — so the surviving pane would
      // render invisible. Only a group's `update()` ever sets the class, so
      // clearing it for every ungrouped pane is safe and idempotent.
      if (!grouped) pane.element.classList.remove("pane--tab-hidden");
      if (pane instanceof TerminalPane || pane instanceof FilesPane) {
        pane.setOwnChrome(!grouped);
      }
    }
    const ctx: RenderContext = {
      paneCache: cache,
      groupCache: this.groupsFor(ws.id),
      makeGroup: (groupId) =>
        new PaneGroup(groupId, {
          labelOf: (paneId) => this.tabLabelFor(paneId),
          onSelectTab: (paneId) => this.selectTab(paneId),
          onNewTab: () => void this.newTabInGroup(groupId),
          onCloseTab: (paneId) => void this.closePane(paneId),
          onCloseOthers: (paneId) => void this.closeOtherTabs(paneId),
          onRenameTab: (paneId) => void this.promptRenameTab(paneId),
          onHotKeysChange: (paneId, hotkeys) => {
            this.updatePaneSpec(paneId, (p) => {
              p.hotkeys = hotkeys;
            });
            // The config tree alone is not enough: the pane keeps its own
            // `PaneSpec` copy and rebuilds its bar from it when the group
            // unwraps, so it would come back with the pre-grouping list.
            // Same write-both dance as `onBgColorChange` below.
            const pane = this.findPaneById(paneId);
            if (pane instanceof TerminalPane) pane.setHotKeys(hotkeys);
          },
          onBgColorChange: (paneId, color) => {
            this.updatePaneSpec(paneId, (p) => {
              p.bg_color = color ?? "";
            });
            const pane = this.findPaneById(paneId);
            if (pane instanceof TerminalPane) pane.setBgColor(color);
          },
          onHotKeySubmit: (paneId) => {
            const pane = this.findPaneById(paneId);
            if (pane instanceof TerminalPane) pane.noteSubmit();
          },
        }),
      onRatioCommitted: (path, ratio) => {
        const wsObj = this.config.workspaces.find((w) => w.id === ws.id);
        if (!wsObj) return;
        wsObj.root = setRatioByPath(wsObj.root, path, ratio);
        this.persistDebounced();
      },
    };
    render(ws.root, container, ctx);
    // `render` rebuilt the container's children, undoing the re-parenting
    // that zoom depends on while `workspace--zoomed` is still set — which
    // left the whole workspace hidden until the user unzoomed. Re-decide.
    this.applyZoom(ws, container);
  }

  /// Re-apply (or drop) this workspace's zoom after a render. The decision is
  /// the pure `zoomAction`; everything below is the DOM half of it.
  private applyZoom(ws: Workspace, container: HTMLElement): void {
    const zoomedId = this.zoomedPanes.get(ws.id) ?? null;
    const cache = this.paneCaches.get(ws.id);
    const exists =
      zoomedId !== null &&
      cache?.get(zoomedId) !== undefined &&
      findPane(ws.root, zoomedId) !== null;
    const groupId = zoomedId ? (groupOfPane(ws.root, zoomedId)?.id ?? null) : null;
    const action = zoomAction(zoomedId, exists, groupId);
    if (action.kind === "none") return;
    if (action.kind === "clear") {
      this.zoomedPanes.delete(ws.id);
      container.classList.remove("workspace--zoomed");
      return;
    }
    this.zoomElementFor(ws, container, action.paneId, action.groupId);
  }

  /// Put `paneId` (or the group holding it) on screen as the zoom overlay:
  /// mark the container, clear any stale zoom classes, re-parent the element
  /// directly under the container so the overlay covers the whole area, and
  /// re-fit whichever terminal is actually visible — re-parenting resets
  /// `.xterm-viewport`'s scrollTop behind xterm's back.
  private zoomElementFor(
    ws: Workspace,
    container: HTMLElement,
    paneId: Uuid,
    groupId: Uuid | null,
  ): void {
    const cache = this.paneCaches.get(ws.id);
    const pane = cache?.get(paneId);
    if (!cache || !pane) return;
    const groups = this.groupsFor(ws.id);
    const group = groupId ? groups.get(groupId) : undefined;
    for (const p of cache.values()) p.element.classList.remove("pane--zoomed");
    for (const g of groups.values()) g.element.classList.remove("pane-group--zoomed");
    container.classList.add("workspace--zoomed");
    const zoomEl = group?.element ?? pane.element;
    zoomEl.classList.add(group ? "pane-group--zoomed" : "pane--zoomed");
    if (zoomEl.parentElement !== container) container.appendChild(zoomEl);
    // For a group the visible terminal is its active tab, which need not be
    // the pane the zoom was started from (the user can switch tabs zoomed).
    const node = groupId ? findGroup(ws.root, groupId) : null;
    const visibleId = node ? (tabIds(node)[node.active] ?? paneId) : paneId;
    cache.get(visibleId)?.scheduleFit();
  }

  /// Resolve a shell name against the detected list. Falls back to the first
  /// available shell if the saved name doesn't exist (e.g. the user uninstalled
  /// PowerShell 7 between sessions).
  private resolveShell(name: string): string {
    if (this.shells.some((s) => s.name === name)) return name;
    return this.shells[0]?.name ?? name;
  }

  /// Build and open the terminal right-click menu for `paneId`.
  ///
  /// The companion tools run *in the clicked pane*, the same way the HotKey
  /// bar submits a command — they are ordinary CLIs on PATH, so this is the
  /// shortest path from "I want yDir" to having it, and Ctrl+C backs out.
  private showPaneContextMenu(paneId: Uuid, ev: MouseEvent): void {
    const pane = this.findPaneById(paneId);
    if (!(pane instanceof TerminalPane)) return;
    // Right-click targets this pane, so split/close act on it. pointerdown
    // has normally focused it already; this makes that independent of
    // whether the platform fires contextmenu on press or on release.
    this.focusedPaneId = paneId;

    const entries: ContextMenuEntry[] = [
      {
        label: t("menu.copy"),
        disabled: !pane.hasSelection(),
        onSelect: () => void pane.copySelection(),
      },
      { label: t("menu.paste"), onSelect: () => void pane.paste() },
      "separator",
      { label: t("shortcut.splitH"), onSelect: () => void this.splitFocused("horizontal") },
      { label: t("shortcut.splitV"), onSelect: () => void this.splitFocused("vertical") },
      { label: t("files.here"), onSelect: () => void this.splitFocusedFiles("horizontal") },
      "separator",
      ...TOOL_MENU.map((tool) => ({
        label: tool.label,
        onSelect: () => pane.runCommand(tool.command),
      })),
    ];
    showContextMenu(ev.clientX, ev.clientY, entries);
  }

  /// Split the currently focused pane.
  async splitFocused(direction: SplitDir): Promise<void> {
    const ws = this.active;
    const focusId = this.focusedPaneId ?? panes(ws.root)[0]?.id;
    if (!focusId) return;
    const existing = findPane(ws.root, focusId);
    // Use the picker's currently selected default shell (it lives at
    // `this.shells[0]` after `setDefaultShell`), not the focused pane's
    // shell. Users expect "I picked Git Bash, then split → new pane is Git
    // Bash", which inheritance from the parent silently breaks once you've
    // changed the picker.
    const shellName = this.resolveShell(this.shells[0]?.name ?? "");

    // Inherit the *live* working directory from the parent pane (OSC 7
    // tracked by the Rust backend) rather than the stale initial cwd stored
    // in the config. This means "split while in ~/projects/foo" opens the new
    // pane in ~/projects/foo, not wherever the shell originally started.
    let liveCwd: string | null = null;
    try {
      liveCwd = await api.getPaneCwd(focusId);
    } catch {
      // Backend didn't have a cwd (pane not spawned yet, or shell never
      // emitted OSC 7). Fall through to the config-stored cwd below.
    }
    const inheritedCwd = liveCwd ?? existing?.cwd ?? null;
    const spec = newPane(shellName, inheritedCwd);
    ws.root = splitPane(ws.root, focusId, direction, spec);

    const cache = this.paneCaches.get(ws.id)!;
    const pane = this.createPane(spec);
    cache.set(spec.id, pane);
    this.renderWorkspace(ws);
    try {
      await pane.spawn();
      pane.focus();
    } catch (e) {
      console.error("split spawn failed", e);
    }
    this.persistDebounced();
  }

  /// Open a tab next to the focused pane, wrapping it in a group first if it
  /// has none (`Ctrl+Shift+T`, the palette).
  async newTabInFocused(): Promise<void> {
    const ws = this.active;
    const sourceId = this.focusedPaneId ?? visiblePanes(ws.root)[0]?.id;
    if (!sourceId) return;
    await this.addTabFrom(ws, sourceId);
  }

  /// The strip's `+`: always adds to *that* group, whatever holds focus (the
  /// button is not inside any `.pane`, so clicking it moves no focus).
  private async newTabInGroup(groupId: Uuid): Promise<void> {
    const ws = this.active;
    const group = findGroup(ws.root, groupId);
    if (!group) return;
    const ids = tabIds(group);
    const sourceId = ids[group.active] ?? ids[0];
    if (!sourceId) return;
    await this.addTabFrom(ws, sourceId);
  }

  /// Spec §4: a new tab runs "the same shell/cwd as the active tab". The live
  /// OSC 7 cwd is preferred over the stale spec one, exactly as `splitFocused`
  /// does. Hotkeys and background colour are copied too: the group shows one
  /// bar, and copying the list is what makes it *look* shared across tabs
  /// without inventing group-level state.
  private async addTabFrom(ws: Workspace, sourceId: Uuid): Promise<void> {
    const source = findPane(ws.root, sourceId);
    // Browser panes have no shell to duplicate and no strip — tabs are a
    // terminal feature (spec §4), so this is a no-op there.
    if (!source || (source.pane_kind ?? "terminal") !== "terminal") return;
    let group = groupOfPane(ws.root, sourceId);
    if (!group) {
      ws.root = wrapInTabs(ws.root, sourceId, uuidv4());
      group = groupOfPane(ws.root, sourceId);
      if (!group) return;
    }
    const liveCwd = await api.getPaneCwd(sourceId).catch(() => null);
    const spec = newPane(this.resolveShell(source.shell), liveCwd ?? source.cwd ?? null);
    spec.hotkeys = (source.hotkeys ?? []).map((h) => ({ ...h }));
    spec.bg_color = source.bg_color ?? "";
    ws.root = addTab(ws.root, group.id, spec);
    const cache = this.paneCaches.get(ws.id)!;
    const pane = this.createPane(spec);
    cache.set(spec.id, pane);
    this.renderWorkspace(ws);
    try {
      await pane.spawn();
      pane.focus();
    } catch (e) {
      console.error("new tab spawn failed", e);
    }
    this.persistDebounced();
  }

  /// Show a tab and focus it. The strip, the workspace tree and the prev/next
  /// shortcuts all land here.
  selectTab(paneId: Uuid): void {
    const ws = this.active;
    const next = activateTabFor(ws.root, paneId);
    if (next !== ws.root) {
      ws.root = next;
      this.renderWorkspace(ws);
      this.persistDebounced();
    }
    this.paneCaches.get(ws.id)?.get(paneId)?.focus();
  }

  /// `Ctrl+Shift+[` / `Ctrl+Shift+]`. A no-op when the focused pane has no
  /// tabs, which is what makes the shortcuts harmless in a plain pane.
  stepTabInFocused(delta: 1 | -1): void {
    const ws = this.active;
    const focusId = this.focusedPaneId ?? visiblePanes(ws.root)[0]?.id;
    if (!focusId) return;
    const group = groupOfPane(ws.root, focusId);
    if (!group) return;
    const nextId = tabIds(group)[stepTab(group, delta)];
    if (nextId && nextId !== focusId) this.selectTab(nextId);
  }

  /// Tab context menu → "Close other tabs". Sequential because each close can
  /// raise a worktree-removal prompt.
  async closeOtherTabs(paneId: Uuid): Promise<void> {
    const group = groupOfPane(this.active.root, paneId);
    if (!group) return;
    for (const id of tabIds(group).filter((x) => x !== paneId)) {
      await this.closePane(id);
    }
    this.selectTab(paneId);
  }

  /// Double-click on a tab, or its context menu → "Rename tab". Writes the
  /// title into that tab's own `PaneSpec`; there is no group-level title, so
  /// there is no second place for it to drift out of sync.
  async promptRenameTab(paneId: Uuid): Promise<void> {
    const current = this.getPaneSpec(paneId)?.title ?? "";
    const next = await askText(t("tab.rename"), current);
    if (next === null) return;
    const trimmed = next.trim();
    const title = trimmed.length > 0 ? trimmed : null;
    this.updatePaneSpec(paneId, (p) => {
      p.title = title;
    });
    const pane = this.findPaneById(paneId);
    (pane as { setTitle?: (t: string | null) => void } | undefined)?.setTitle?.(title);
    this.refreshTabChrome();
  }

  /// The file dock's yDir pressed Enter on a file. It goes into the viewer
  /// tab of the pane the dock follows — `activePaneId()`, which is the pane
  /// that was active before the dock took focus, because the dock's own pane
  /// lives outside every layout tree and so never becomes the active one.
  /// One viewer tab per pane: the second Enter kills and respawns
  /// `ycode <path>` in the same tab rather than opening another (spec §4).
  async openFileInViewerTab(path: string): Promise<void> {
    if (!path) return;
    const ws = this.active;
    const targetId = this.activePaneId();
    if (!targetId) return;
    let group = groupOfPane(ws.root, targetId);
    if (!group) {
      ws.root = wrapInTabs(ws.root, targetId, uuidv4());
      group = groupOfPane(ws.root, targetId);
      if (!group) return;
    }
    const action = viewerTabAction(this.viewerTabs.get(group.id), tabIds(group));
    if (action.kind === "reuse") {
      await this.respawnViewer(ws, action.paneId, path);
      return;
    }
    const spec = newPane(this.resolveShell(this.shells[0]?.name ?? ""), null);
    ws.root = addTab(ws.root, group.id, spec);
    this.viewerTabs.set(group.id, spec.id);
    const cache = this.paneCaches.get(ws.id)!;
    const pane = this.createPane(spec, ["ycode", path]);
    cache.set(spec.id, pane);
    this.renderWorkspace(ws);
    try {
      await pane.spawn();
      pane.focus();
    } catch (e) {
      console.error("viewer tab spawn failed", e);
    }
    this.persistDebounced();
  }

  /// Replace the file shown by an existing viewer tab. The pane id is kept,
  /// so the tab stays where it is in the strip and nothing else in the layout
  /// moves; only the PTY behind it is swapped.
  private async respawnViewer(ws: Workspace, paneId: Uuid, path: string): Promise<void> {
    const spec = findPane(ws.root, paneId);
    if (!spec) return;
    const cache = this.paneCaches.get(ws.id)!;
    // Not permanent: this tab is not being closed, only re-pointed. The
    // saved scrollback is dropped explicitly below instead, *after* the kill,
    // so nothing can race the delete.
    cache.get(paneId)?.dispose(false);
    cache.delete(paneId);
    // `dispose` fires `killPane` without awaiting and Tauri commands run on a
    // worker pool, so serialize it — otherwise a late kill can land on the
    // pane we are about to spawn under the same id. Same hazard the file
    // dock's own restart path handles this way.
    await api.killPane(paneId).catch(() => {});
    // The pane id is reused, so `spawn()`'s `loadScrollback` would replay the
    // *previous* file's ycode screen above the new one. Awaited for the same
    // reason the kill above is: Tauri commands run on a worker pool, and a
    // fire-and-forget delete could land after the new pane's load.
    await api.deleteScrollback(paneId).catch(() => {});
    const pane = this.createPane(spec, ["ycode", path]);
    cache.set(paneId, pane);
    ws.root = activateTabFor(ws.root, paneId);
    this.renderWorkspace(ws);
    try {
      await pane.spawn();
      pane.focus();
    } catch (e) {
      console.error("viewer tab respawn failed", e);
    }
  }

  /// The label a tab shows: the user's title, else the running program from
  /// the process scan, else the shell name (`src/terminal/tabLabel.ts`).
  tabLabelFor(paneId: Uuid): string {
    const spec = this.getPaneSpec(paneId);
    // A files pane has no shell or process: its label is the folder it shows.
    if (spec?.pane_kind === "files") {
      return spec.title || (spec.cwd ? baseName(spec.cwd) : t("files.title"));
    }
    return tabLabel({
      title: spec?.title ?? null,
      shell: spec?.shell ?? "",
      process: this._paneLabels[paneId] ?? null,
      fallback: t("terminal.defaultTitle"),
    });
  }

  get paneLabels(): Record<Uuid, string> {
    return this._paneLabels;
  }

  /// A new `panes:labels` snapshot. Only the strips and the tree are
  /// repainted — never a full layout render, which would detach every
  /// terminal every couple of seconds.
  applyPaneLabels(next: Record<Uuid, string>): void {
    this._paneLabels = next;
    this.refreshTabChrome();
    this.notifyTree();
  }

  private refreshTabChrome(): void {
    for (const group of this.groupsFor(this.activeId).values()) group.refreshLabels();
  }

  /// Split the focused pane into a fresh git worktree rooted shell. Prompts
  /// for a branch name, creates the worktree via the backend, then spawns a
  /// terminal pane whose cwd is the new worktree directory.
  async openWorktreePane(direction: SplitDir): Promise<void> {
    const ws = this.active;
    const focusId = this.focusedPaneId ?? panes(ws.root)[0]?.id;
    if (!focusId) return;
    const existing = findPane(ws.root, focusId);

    // Resolve the same way `splitFocused` does: prefer the live cwd tracked
    // by the backend (OSC 7), fall back to the config-stored cwd.
    let liveCwd: string | null = null;
    try {
      liveCwd = await api.getPaneCwd(focusId);
    } catch {
      // Backend didn't have a cwd (pane not spawned yet, or shell never
      // emitted OSC 7). Fall through to the config-stored cwd below.
    }
    const baseCwd = liveCwd ?? existing?.cwd ?? null;
    if (!baseCwd) {
      console.warn("worktree: could not resolve base cwd for pane", focusId);
      void api.notify(t("worktree.command"), t("worktree.noCwd")).catch(() => {});
      return;
    }

    if (!(await api.gitIsRepo(baseCwd))) {
      console.warn("worktree: not a git repo", baseCwd);
      void api.notify(t("worktree.command"), t("worktree.notRepo")).catch(() => {});
      return;
    }

    const branch = await promptWorktreeBranch(`agent/${crypto.randomUUID().slice(0, 6)}`);
    if (!branch) return;

    let wtPath: string;
    try {
      wtPath = await api.gitWorktreeAdd(baseCwd, branch, this.worktreeBaseDir);
    } catch (e) {
      console.error("worktree add failed", e);
      const reason = e instanceof Error ? e.message : String(e);
      void api.notify(t("worktree.command"), `${t("worktree.addFailed")} ${reason}`).catch(() => {});
      return;
    }

    const shellName = this.resolveShell(this.shells[0]?.name ?? "");
    const spec = newPane(shellName, wtPath);
    spec.worktree_path = wtPath;
    ws.root = splitPane(ws.root, focusId, direction, spec);

    const cache = this.paneCaches.get(ws.id)!;
    const pane = this.createPane(spec);
    cache.set(spec.id, pane);
    this.renderWorkspace(ws);
    try {
      await pane.spawn();
      pane.focus();
    } catch (e) {
      console.error("worktree spawn failed", e);
    }
    this.persistDebounced();
  }

  /// Split the focused pane and drop a browser pane into the new slot instead
  /// of a terminal. URL defaults to `about:blank` so the user can type into
  /// the URL bar.
  async splitFocusedBrowser(direction: SplitDir, url: string = ""): Promise<void> {
    const ws = this.active;
    const focusId = this.focusedPaneId ?? panes(ws.root)[0]?.id;
    if (!focusId) return;
    const spec: PaneSpec = {
      id: crypto.randomUUID(),
      title: null,
      shell: "",
      cwd: null,
      startup_cmd: null,
      env: [],
      pane_kind: "embedded_browser",
      url: url || null,
      hotkeys: [],
    };
    ws.root = splitPane(ws.root, focusId, direction, spec);
    const cache = this.paneCaches.get(ws.id)!;
    const pane = this.createPane(spec);
    cache.set(spec.id, pane);
    this.renderWorkspace(ws);
    try {
      await pane.spawn();
      pane.focus();
    } catch (e) {
      console.error("browser split failed", e);
    }
    this.persistDebounced();
  }

  /// Split the focused pane and open a files pane in the new slot, showing
  /// the focused pane's live directory (its OSC 7 cwd, else its stored cwd;
  /// a files pane's stored cwd *is* the folder it shows).
  async splitFocusedFiles(direction: SplitDir): Promise<void> {
    const ws = this.active;
    const focusId = this.focusedPaneId ?? panes(ws.root)[0]?.id;
    if (!focusId) return;
    const liveCwd = await api.getPaneCwd(focusId).catch(() => null);
    const spec = newPane("", liveCwd ?? findPane(ws.root, focusId)?.cwd ?? null);
    spec.pane_kind = "files";
    await this.insertSplit(ws, focusId, direction, spec, "files split failed");
  }

  /// Split `anchorId` (or the active pane) with a terminal whose cwd is
  /// `dir` — a files pane's "Open terminal here".
  async splitTerminalAt(anchorId: Uuid | null, dir: string): Promise<void> {
    const ws = this.active;
    const target = anchorId && findPane(ws.root, anchorId) ? anchorId : this.activePaneId();
    if (!target) return;
    const spec = newPane(this.resolveShell(this.shells[0]?.name ?? ""), dir);
    await this.insertSplit(ws, target, "horizontal", spec, "terminal split failed");
  }

  private async insertSplit(
    ws: Workspace,
    targetId: Uuid,
    direction: SplitDir,
    spec: PaneSpec,
    failure: string,
  ): Promise<void> {
    ws.root = splitPane(ws.root, targetId, direction, spec);
    const cache = this.paneCaches.get(ws.id)!;
    const pane = this.createPane(spec);
    cache.set(spec.id, pane);
    this.renderWorkspace(ws);
    try {
      await pane.spawn();
      pane.focus();
    } catch (e) {
      console.error(failure, e);
    }
    this.persistDebounced();
  }

  /// Close the currently focused pane.
  /// Close the currently focused pane — or, when it is a tab, that tab.
  async closeFocused(): Promise<void> {
    if (!this.focusedPaneId) return;
    await this.closePane(this.focusedPaneId);
  }

  /// Close one pane. `removePane` unwraps a group down to a plain pane and
  /// drops the group entirely when its last tab goes, so `Ctrl+Shift+W` keeps
  /// exactly today's meaning: close the active tab, and on the last one close
  /// the pane (spec §4). There is deliberately no tab-specific branch here.
  private async closePane(id: Uuid): Promise<void> {
    const ws = this.active;
    // Captured before the tree is mutated: once the pane is gone so is its
    // spec, and once the group may have unwrapped there is no other way to
    // know which tab should take focus.
    const wtPath = findPane(ws.root, id)?.worktree_path ?? "";
    const group = groupOfPane(ws.root, id);
    const siblings = group ? tabIds(group).filter((x) => x !== id) : [];
    const newRoot = removePane(ws.root, id);
    const cache = this.paneCaches.get(ws.id)!;
    const pane = cache.get(id);
    // Permanent: the user explicitly closed this pane (kill_pane), so its
    // persisted scrollback (if any) should not survive it.
    pane?.dispose(true);
    cache.delete(id);
    this.paneStatus.delete(id);

    if (newRoot === null) {
      // Workspace would be empty; create a replacement pane so there is
      // always something to look at.
      const defaultShell = this.resolveShell(this.shells[0]?.name ?? "");
      const spec = newPane(defaultShell);
      ws.root = paneNode(spec);
      const replacement = this.createPane(spec);
      cache.set(spec.id, replacement);
      this.renderWorkspace(ws);
      await replacement.spawn();
      replacement.focus();
    } else {
      ws.root = newRoot;
      this.focusedPaneId = null;
      // Stay inside the group when one of its tabs was closed; otherwise fall
      // back to the first pane on screen, in depth-first order, so the new
      // focus is predictable rather than Map-insertion dependent.
      const nextId =
        siblings.find((s) => findPane(ws.root, s)) ?? visiblePanes(ws.root)[0]?.id;
      if (nextId) ws.root = activateTabFor(ws.root, nextId);
      this.renderWorkspace(ws);
      if (nextId) cache.get(nextId)?.focus();
    }
    this.persistDebounced();

    // The pane is fully closed at this point (PTY killed, tree updated,
    // focus settled) regardless of what happens below. Offer to remove its
    // git worktree now that dispose(true) has released any OS-level lock on
    // the worktree directory (important on Windows). A removal failure here
    // must never be surfaced as a close failure.
    if (wtPath) {
      await this.offerWorktreeRemoval(wtPath);
    }
  }

  /// Ask the user whether to remove the git worktree at `wtPath`, and do so
  /// if confirmed. A dirty worktree gets a second, forced-removal prompt.
  /// Errors are logged, never thrown — worktree cleanup is best-effort and
  /// must not fail the pane close / workspace delete that triggered it.
  private async offerWorktreeRemoval(wtPath: string): Promise<void> {
    const ok = await askConfirm(
      t("worktree.removeConfirm").replace("{path}", wtPath),
    );
    if (!ok) return;
    try {
      await api.gitWorktreeRemove(wtPath, false);
    } catch {
      // Dirty worktree or similar — offer a forced removal.
      if (await askConfirm(t("worktree.removeForce"))) {
        try {
          await api.gitWorktreeRemove(wtPath, true);
        } catch (e) {
          console.error("worktree remove failed", e);
        }
      }
    }
  }

  /// Toggle "zoom" on the focused pane: hide every other pane in the workspace
  /// by css so the focused one takes the whole viewport. The layout tree is
  /// unchanged; on unzoom, the normal render reappears.
  toggleZoomFocused(): void {
    const ws = this.active;
    const container = this.workspaceContainers.get(ws.id);
    if (!container) return;
    const id = this.focusedPaneId ?? panes(ws.root)[0]?.id;
    if (!id) return;
    const cache = this.paneCaches.get(ws.id);
    const pane = cache?.get(id);
    if (!pane) return;

    if (this.zoomedPanes.has(ws.id)) {
      // Unzoom: forget the state *before* rendering, so the re-apply pass at
      // the end of `renderWorkspace` sees "nothing zoomed" and leaves the
      // rebuilt layout alone.
      this.zoomedPanes.delete(ws.id);
      container.classList.remove("workspace--zoomed");
      for (const p of cache!.values()) p.element.classList.remove("pane--zoomed");
      for (const g of this.groupsFor(ws.id).values()) {
        g.element.classList.remove("pane-group--zoomed");
      }
      this.renderWorkspace(ws);
      pane.focus();
      pane.scheduleFit();
      return;
    }
    // Zoom the group, not the tab: a tab is built with `ownChrome: false`, so
    // zooming its element alone would show a terminal with no title row and
    // no hotkey bar. Which element that is gets re-decided on every render,
    // because the pane can gain or lose tabs while zoomed.
    this.zoomedPanes.set(ws.id, id);
    this.zoomElementFor(ws, container, id, groupOfPane(ws.root, id)?.id ?? null);
    pane.focus();
    pane.scheduleFit();
  }

  /// Returns the current display title of the focused pane, or null if there
  /// is no focus or no custom title set. Used to pre-fill the rename prompt.
  getFocusedTitle(): string | null {
    const id = this.focusedPaneId;
    if (!id) return null;
    return this.getPaneSpec(id)?.title ?? null;
  }

  /// Rename the focused pane. Passing an empty string clears the title so the
  /// default rendering (shell name) is used.
  renameFocused(title: string): void {
    const id = this.focusedPaneId;
    if (!id) return;
    const trimmed = title.trim();
    this.updatePaneSpec(id, (p) => {
      p.title = trimmed.length > 0 ? trimmed : null;
    });
    const pane = this.paneCaches.get(this.activeId)?.get(id);
    (pane as { setTitle?: (t: string | null) => void } | undefined)?.setTitle?.(
      trimmed.length > 0 ? trimmed : null,
    );
    // A tab's own `titleEl` is null — the group draws the shared title row.
    this.refreshTabChrome();
  }

  getWorkspaceName(wsId: number): string | null {
    const ws = this.config.workspaces.find((w) => w.id === wsId);
    return ws?.name ?? null;
  }

  renameWorkspace(wsId: number, name: string): void {
    const ws = this.config.workspaces.find((w) => w.id === wsId);
    if (!ws) return;
    const trimmed = name.trim();
    ws.name = trimmed.length > 0 ? trimmed : `workspace-${wsId}`;
    this.persistDebounced();
  }

  /// Request the focused pane to toggle its scrollback search bar. Only
  /// TerminalPane exposes this; for non-terminal panes the call is a no-op.
  toggleSearchOnFocused(): void {
    const id = this.focusedPaneId;
    if (!id) return;
    const pane = this.paneCaches.get(this.activeId)?.get(id);
    (pane as { toggleSearch?: () => void } | undefined)?.toggleSearch?.();
  }

  /// Swap the focused pane with the previous / next pane in depth-first order
  /// (wrapping at both ends). Only the two panes' slots in the layout change;
  /// their ids, cache entries, DOM elements, and PTYs are preserved, so
  /// terminal scrollback survives and focus stays on the same pane.
  swapFocused(delta: 1 | -1): void {
    const ws = this.active;
    // Only ungrouped panes swap slots: trading a tab for a pane in another
    // split would silently move a terminal out of the chrome it shares and
    // pull an unrelated one in.
    const list = swappablePanes(ws.root);
    if (list.length < 2) return;
    const focusId = this.focusedPaneId ?? list[0].id;
    const idx = list.findIndex((p) => p.id === focusId);
    if (idx < 0) return;
    const targetId = list[(idx + delta + list.length) % list.length].id;
    if (targetId === focusId) return;
    ws.root = swapPanes(ws.root, focusId, targetId);
    this.renderWorkspace(ws);
    // Same id → element is reused with its focus state; re-assert to be safe.
    this.paneCaches.get(ws.id)?.get(focusId)?.focus();
    this.persistDebounced();
  }

  /// Enable/disable bell-completion notifications and persist the choice.
  setNotifyOnBell(enabled: boolean): void {
    this.config.notify_on_bell = enabled;
    this.persistDebounced();
  }

  get notifyOnBell(): boolean {
    return this.config.notify_on_bell;
  }

  /// Enable/disable persisting terminal scrollback to disk and persist the choice.
  setPersistScrollback(enabled: boolean): void {
    this.config.persist_scrollback = enabled;
    this.persistDebounced();
  }

  get persistScrollback(): boolean {
    return this.config.persist_scrollback;
  }

  /// Enable/disable the bottom-anchored prompt and persist the choice. Applies
  /// to every live terminal in every workspace at once, for the same reason
  /// `setFontSize` does: hidden workspaces keep their panes alive.
  setBottomAnchor(enabled: boolean): void {
    this.config.bottom_anchor = enabled;
    for (const cache of this.paneCaches.values()) {
      for (const pane of cache.values()) {
        if (pane instanceof TerminalPane) pane.refreshBottomAnchor();
      }
    }
    this.persistDebounced();
  }

  get bottomAnchor(): boolean {
    return this.config.bottom_anchor;
  }

  /// Directory new worktrees are created under (see `openWorktreePane`).
  get worktreeBaseDir(): string {
    return this.config.worktree_base_dir;
  }

  get agents(): AgentSnapshot {
    return this._agents;
  }

  /// Subscribe to tree-relevant changes. Returns an unsubscribe function.
  onTreeChange(cb: () => void): () => void {
    this.treeListeners.add(cb);
    return () => {
      this.treeListeners.delete(cb);
    };
  }

  private notifyTree(): void {
    for (const cb of this.treeListeners) cb();
  }

  /// Take a new backend snapshot. A lead that just started waiting on the
  /// user raises its pane to `attention`, unless the user is already looking
  /// at that exact pane (same bar as the bell notification).
  applyAgents(next: AgentSnapshot): void {
    for (const id of newlyWaitingPanes(this._agents, next)) {
      if (this.isWatching(id)) continue;
      const pane = this.findPaneById(id);
      if (pane instanceof TerminalPane) pane.markWaiting();
    }
    this._agents = next;
    this.notifyTree();
  }

  /// Switch to the workspace owning `paneId` (hydrating it if never visited)
  /// and focus that pane. Used by the tree's pane and agent rows.
  async focusPane(paneId: Uuid): Promise<void> {
    const wsId = workspaceIdOfPane(this.config.workspaces, paneId);
    if (wsId === null) return;
    if (wsId !== this.activeId) await this.activate(wsId);
    // The target may be a hidden tab (a tree row, the viewer tab): show it
    // first, or `focus()` would land on something the user cannot see.
    const ws = this.active;
    const next = activateTabFor(ws.root, paneId);
    if (next !== ws.root) {
      ws.root = next;
      this.renderWorkspace(ws);
      this.persistDebounced();
    }
    this.paneCaches.get(wsId)?.get(paneId)?.focus();
  }

  get agentTracking(): boolean {
    return this.config.agent_tracking ?? false;
  }

  /// Install/remove the Claude Code hooks, then record the choice. Rejects
  /// (setting unchanged) if the backend couldn't write settings.json.
  async setAgentTracking(enabled: boolean): Promise<void> {
    await api.setAgentTracking(enabled);
    this.config.agent_tracking = enabled;
    this.persistDebounced();
    // The workspace panel's "tracking is off" hint keys off this.
    this.notifyTree();
  }

  /// Can the user see pane `paneId` right now — window focused and its
  /// workspace the visible one? This drives the *status* classification
  /// (`done` = you saw it finish, `attention` = you didn't) and how long a
  /// `done` marker is held.
  ///
  /// Deliberately does NOT require the pane to be the focused one: every pane
  /// in a split is on screen simultaneously, so keyboard focus says nothing
  /// about whether the user saw it. Nor can the pane's own `isFocused` flag
  /// stand in — nothing lowers it when the user switches workspaces or
  /// alt-tabs away, which is exactly how an agent finishing out of sight used
  /// to get classified as `done`.
  private isPaneVisible(paneId: Uuid): boolean {
    if (!this.windowFocused) return false;
    if (this.workspaceOfPane(paneId) !== this.activeId) return false;
    // A hidden tab is not on screen. Without this an agent finishing in a
    // background tab would be scored "the user saw it finish" (`done`)
    // instead of raising `attention`.
    return isVisibleTab(this.active.root, paneId);
  }

  /// Stricter: is the user looking at *this exact pane*? Gates the OS
  /// notification and beep, where the bar is higher — being able to see a
  /// pane in a corner of a split is not a reason to swallow the alert that
  /// the thing you launched has finished.
  private isWatching(paneId: Uuid): boolean {
    return this.isPaneVisible(paneId) && this.focusedPaneId === paneId;
  }

  /// Which workspace owns pane `paneId`, or null if it isn't in any live cache.
  private workspaceOfPane(paneId: Uuid): number | null {
    for (const [wsId, cache] of this.paneCaches) {
      if (cache.has(paneId)) return wsId;
    }
    return null;
  }

  /// Look up a live `Pane` instance by id across every workspace's cache
  /// (the focused pane may belong to a currently-hidden workspace).
  private findPaneById(paneId: Uuid): Pane | undefined {
    for (const cache of this.paneCaches.values()) {
      const pane = cache.get(paneId);
      if (pane) return pane;
    }
    return undefined;
  }

  private groupsFor(wsId: number): Map<Uuid, PaneGroup> {
    let map = this.groupCaches.get(wsId);
    if (!map) {
      map = new Map();
      this.groupCaches.set(wsId, map);
    }
    return map;
  }

  /// Drop the chrome of groups that have left the tree — unwrapped by a close,
  /// or removed with their workspace — disposing the HotKeyBar each one owns
  /// and forgetting its viewer tab.
  private pruneGroups(ws: Workspace): void {
    const groups = this.groupsFor(ws.id);
    const live = new Set<Uuid>();
    for (const entry of paneGroups(ws.root)) {
      if (entry.groupId) live.add(entry.groupId);
    }
    for (const [id, group] of groups) {
      if (live.has(id)) continue;
      group.dispose();
      groups.delete(id);
      this.viewerTabs.delete(id);
    }
  }

  /// Repaint pane `id`'s status border + tooltip. Called whenever a
  /// TerminalPane's derived status (idle/running/done/attention) changes.
  private applyPaneStatusClass(id: Uuid, status: PaneStatus): void {
    const el = this.host.querySelector<HTMLElement>(`.pane[data-pane-id="${id}"]`);
    if (!el) return;
    el.classList.remove(
      "pane--status-idle",
      "pane--status-running",
      "pane--status-done",
      "pane--status-attention",
    );
    el.classList.add(`pane--status-${status}`);
    // "idle" has no dedicated i18n key (it's the common resting state) — leave
    // the tooltip empty rather than showing the raw translation key.
    el.title = status === "idle" ? "" : t(`status.${status}`);
  }

  /// Highest-priority pane status among all live panes in workspace `wsId`
  /// (attention > running > done > idle). Drives the workspace tab's status
  /// dot colour.
  workspaceStatus(wsId: number): PaneStatus {
    const cache = this.paneCaches.get(wsId);
    if (!cache) return "idle";
    let sawRunning = false;
    let sawDone = false;
    for (const id of cache.keys()) {
      const status = this.paneStatus.get(id) ?? "idle";
      if (status === "attention") return "attention";
      if (status === "running") sawRunning = true;
      else if (status === "done") sawDone = true;
    }
    if (sawRunning) return "running";
    if (sawDone) return "done";
    return "idle";
  }

  /// Handle a bell / OSC 9 "attention" signal from a terminal pane — the way a
  /// long-running CLI (claude/codex/gemini) says it finished. Suppressed when
  /// the user is already watching the pane (focused pane, active workspace,
  /// focused window); otherwise fires an OS notification and beeps. The visual
  /// "needs attention" indicator itself is owned entirely by
  /// `PaneStatusMachine`/`pane--status-attention`/`ws-dot--attention` (v0.8.19's
  /// unified 4-state status system) — this handler no longer touches the DOM
  /// directly, it only gates the notification/beep side effects.
  private handleAttention(paneId: Uuid, message: string | null): void {
    if (!this.config.notify_on_bell) return;
    const wsId = this.workspaceOfPane(paneId);
    if (wsId === null) return;

    if (this.isWatching(paneId)) return;

    // OS notification + sound.
    const name = this.getWorkspaceName(wsId);
    const label =
      name && name !== `workspace-${wsId}` ? `${wsId}: ${name}` : `${wsId}`;
    const trimmed = message?.trim();
    const body =
      trimmed && trimmed.length > 0
        ? trimmed
        : t("notify.paneDone").replace("{ws}", label);
    void api.notify(t("notify.title"), body).catch(() => {});
    beep();
  }

  /// Move focus to the next pane in depth-first order.
  /// Move focus to the next pane in depth-first order. Skips hidden tabs:
  /// `Ctrl+Tab` cycles panes, `Ctrl+Shift+[`/`]` cycles tabs.
  cycleFocus(delta: 1 | -1): void {
    const ws = this.active;
    const list = visiblePanes(ws.root);
    if (list.length === 0) return;
    const idx = Math.max(
      0,
      list.findIndex((p) => p.id === this.focusedPaneId),
    );
    const next = list[(idx + delta + list.length) % list.length];
    const cache = this.paneCaches.get(ws.id)!;
    cache.get(next.id)?.focus();
  }

  /// Called from the window resize listener to refit every live terminal in
  /// the active workspace.
  refitActive(): void {
    const cache = this.paneCaches.get(this.activeId);
    if (!cache) return;
    for (const pane of cache.values()) pane.scheduleFit();
  }

  /// Subscribe to active-pane changes. Returns the unsubscribe function.
  onActivePaneChange(cb: () => void): () => void {
    this.activePaneListeners.add(cb);
    return () => {
      this.activePaneListeners.delete(cb);
    };
  }

  private notifyActivePaneChange(): void {
    for (const cb of this.activePaneListeners) cb();
  }

  /// See `pickActivePaneId`. Hidden tabs are not where the user works, so the
  /// list is `visiblePanes`, not `panes` — otherwise the file dock would
  /// follow the cwd of a terminal nobody can see.
  activePaneId(): Uuid | null {
    return pickActivePaneId(
      visiblePanes(this.active.root).map((p) => p.id),
      this._focusedPaneId,
    );
  }

  /// Give keyboard focus back to `activePaneId()`. Used when the file dock
  /// closes while it held focus. The pane is always in the active
  /// workspace, so `focusPane` never switches workspaces here.
  focusActivePane(): void {
    const id = this.activePaneId();
    if (id) void this.focusPane(id);
  }

  /// Type `text` into whichever terminal pane sits under the given viewport
  /// point, focusing it first. Used by the file drag-and-drop handler so a
  /// drop lands in the pane the user aimed at rather than the focused one.
  /// Non-terminal panes (browser) and points outside any pane are ignored.
  typeIntoPaneAt(clientX: number, clientY: number, text: string): void {
    if (!text) return;
    const hit = document.elementFromPoint(clientX, clientY);
    const paneId = hit?.closest<HTMLElement>("[data-pane-id]")?.dataset.paneId;
    if (!paneId) return;
    const pane = this.paneCaches.get(this.activeId)?.get(paneId);
    if (!(pane instanceof TerminalPane)) return;
    pane.focus();
    pane.typeText(text);
  }

  /// Save the current config to disk. Debounced by 500 ms so rapid changes
  /// collapse into a single write.
  private persistDebounced(): void {
    // Every layout / pane-metadata mutation funnels through here — the tree follows it.
    this.notifyTree();
    if (this.saveTimer !== null) {
      clearTimeout(this.saveTimer);
    }
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      void api.saveConfig(this.config).catch((e) => {
        console.error("saveConfig failed", e);
      });
    }, 500) as unknown as number;
  }

  /// Flush pending save immediately. Used on window close.
  async flush(): Promise<void> {
    if (this.saveTimer !== null) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    await api.saveConfig(this.config).catch(() => {});
  }

  /// Shell profile new panes and workspaces are created with, resolved
  /// against what is actually installed.
  get defaultShell(): string {
    return this.resolveShell(this.config.default_shell);
  }

  /// Replace the default shell used for newly created panes, and persist it.
  /// Two UIs set this — the toolbar picker and Settings → General — so the
  /// one that didn't initiate the change is notified through
  /// `onDefaultShellChange`.
  setDefaultShell(name: string): void {
    if (!this.shells.some((s) => s.name === name)) return;
    if (this.config.default_shell === name) return;
    this.config.default_shell = name;
    this.orderShellsByDefault();
    this.persistDebounced();
    this.onDefaultShellChange?.(name);
  }

  /// Terminal font size in CSS pixels, clamped to the range the backend
  /// model documents. Older configs have no value, hence the fallback.
  get fontSize(): number {
    return clampFontSize(this.config.font_size ?? DEFAULT_FONT_SIZE);
  }

  /// Set the terminal font size, apply it to every live pane, and persist.
  ///
  /// Applies across *all* workspaces, not just the active one: panes stay
  /// alive in the background (that is the whole point of workspaces here), so
  /// skipping the hidden ones would leave them on the old size until they
  /// happened to be rebuilt.
  setFontSize(px: number): void {
    const next = clampFontSize(px);
    if (next === this.fontSize) return;
    this.config.font_size = next;
    for (const cache of this.paneCaches.values()) {
      for (const pane of cache.values()) {
        if (pane instanceof TerminalPane) pane.setFontSize(next);
      }
    }
    this.persistDebounced();
    this.onFontSizeChange?.(next);
  }

  /// Nudge the font size by `delta` steps. Used by the zoom shortcuts.
  bumpFontSize(delta: number): void {
    this.setFontSize(this.fontSize + delta);
  }

  resetFontSize(): void {
    this.setFontSize(DEFAULT_FONT_SIZE);
  }

  /// Move the configured default to the front of `this.shells`. Every
  /// new-pane path already reads `this.shells[0]`, so ordering the list is
  /// all it takes for the setting to apply everywhere.
  private orderShellsByDefault(): void {
    const name = this.config.default_shell;
    if (!name || !this.shells.some((s) => s.name === name)) return;
    this.shells = [
      ...this.shells.filter((s) => s.name === name),
      ...this.shells.filter((s) => s.name !== name),
    ];
  }
}

// Re-export a helper for the rest of the app. Not used internally but used by
// unit tests and the main module.
export { MAX_WORKSPACES };
// Re-exported so callers that already hold the manager don't need a second
// import path for the bounds.
export { clampFontSize, MIN_FONT_SIZE, MAX_FONT_SIZE, DEFAULT_FONT_SIZE } from "./fontSize";
// Needed to satisfy `import type { LayoutNode }` at the top-level in other
// files that import from this module.
export type { LayoutNode };
