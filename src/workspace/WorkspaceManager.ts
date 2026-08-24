// Owns all workspaces, their layout trees, and per-workspace pane caches.
// Switching workspaces hides the previous DOM subtree without disposing any
// xterm instances, so scrollback survives — the tmux semantics the user
// explicitly asked for.

import type {
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
import type { Pane } from "../layout/Pane";
import {
  findPane,
  newPane,
  panes,
  removePane,
  setRatioByPath,
  splitPane,
  swapPanes,
  worktreePaths,
} from "../layout/LayoutTree";
import { render, type RenderContext } from "../layout/SplitContainer";
import { beep } from "../util/beep";
import { t } from "../i18n/i18n";
import { promptWorktreeBranch } from "./WorktreeModal";
import { askConfirm } from "../ui/Dialog";
import { showContextMenu, type ContextMenuEntry } from "../menu/ContextMenu";
import { moveItem } from "./reorder";
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
    // any number of browser panes. Requires the `browser-children`
    // capability so Tauri IPC is exposed in the remote-URL webview.
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
    if (!this.focusedPaneId && cache.size > 0) {
      const first = cache.values().next().value as Pane | undefined;
      first?.focus();
    }
  }

  /// Build either a terminal or browser pane based on `spec.pane_kind`. All
  /// focus / hotkey / url change callbacks are wired so the manager can react
  /// to state changes without needing to know the pane subclass.
  private createPane(spec: PaneSpec): Pane {
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
    const resolvedShell = this.resolveShell(spec.shell);
    const finalSpec: PaneSpec = { ...spec, shell: resolvedShell };
    return new TerminalPane({
      spec: finalSpec,
      fontSize: this.fontSize,
      onFocus: () => {
        this.focusedPaneId = spec.id;
      },
      onAttention: (msg) => this.handleAttention(spec.id, msg),
      isVisible: () => this.isPaneVisible(spec.id),
      onContextMenu: (ev) => this.showPaneContextMenu(spec.id, ev),
      persistScrollback: () => this.persistScrollback,
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
    const ctx: RenderContext = {
      paneCache: cache,
      onRatioCommitted: (path, ratio) => {
        const wsObj = this.config.workspaces.find((w) => w.id === ws.id);
        if (!wsObj) return;
        wsObj.root = setRatioByPath(wsObj.root, path, ratio);
        this.persistDebounced();
      },
    };
    render(ws.root, container, ctx);
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

  /// Close the currently focused pane.
  async closeFocused(): Promise<void> {
    const ws = this.active;
    if (!this.focusedPaneId) return;
    const id = this.focusedPaneId;
    // Capture before the tree is mutated below — once the pane is removed
    // from the layout, its spec (and worktree_path) is gone.
    const wtPath = findPane(ws.root, id)?.worktree_path ?? "";
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
      ws.root = {
        kind: "pane",
        id: spec.id,
        title: null,
        shell: defaultShell,
        cwd: null,
        startup_cmd: null,
        env: [],
        pane_kind: "terminal",
        url: null,
        hotkeys: [],
      };
      const replacement = this.createPane(spec);
      cache.set(spec.id, replacement);
      this.renderWorkspace(ws);
      await replacement.spawn();
      replacement.focus();
    } else {
      ws.root = newRoot;
      this.renderWorkspace(ws);
      // Move focus to the first remaining pane in tree (depth-first) order
      // so the new focus is predictable from the user's point of view, not
      // dependent on Map insertion order.
      this.focusedPaneId = null;
      const remaining = panes(ws.root);
      const next = remaining[0] ? cache.get(remaining[0].id) : undefined;
      next?.focus();
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

    const alreadyZoomed = container.classList.contains("workspace--zoomed");
    if (alreadyZoomed) {
      container.classList.remove("workspace--zoomed");
      pane.element.classList.remove("pane--zoomed");
      this.renderWorkspace(ws);
      pane.focus();
      pane.scheduleFit();
      return;
    }
    // Ensure the pane element is directly inside the workspace container so
    // the absolute-positioned overlay covers the whole area, and clear any
    // previous zoom styling from a stale toggle.
    for (const p of cache!.values()) p.element.classList.remove("pane--zoomed");
    container.classList.add("workspace--zoomed");
    pane.element.classList.add("pane--zoomed");
    if (pane.element.parentElement !== container) {
      container.appendChild(pane.element);
    }
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
    const list = panes(ws.root);
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

  /// Directory new worktrees are created under (see `openWorktreePane`).
  get worktreeBaseDir(): string {
    return this.config.worktree_base_dir;
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
    return this.workspaceOfPane(paneId) === this.activeId;
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
  cycleFocus(delta: 1 | -1): void {
    const ws = this.active;
    const list = panes(ws.root);
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

/// Walk `root` in place, apply `patch` to the pane whose id matches `id`, and
/// return true on success. The tree's shape is not altered. Mirrors Rust's
/// `LayoutNode::find_pane_mut`.
function findAndMutatePane(
  root: LayoutNode,
  id: Uuid,
  patch: (spec: PaneSpec) => void,
): boolean {
  if (root.kind === "pane") {
    if (root.id === id) {
      const snapshot: PaneSpec = {
        id: root.id,
        title: root.title,
        shell: root.shell,
        cwd: root.cwd,
        startup_cmd: root.startup_cmd,
        env: root.env,
        pane_kind: root.pane_kind ?? "terminal",
        url: root.url ?? null,
        hotkeys: root.hotkeys ?? [],
        bg_color: root.bg_color ?? "",
        worktree_path: root.worktree_path ?? "",
      };
      patch(snapshot);
      root.title = snapshot.title;
      root.shell = snapshot.shell;
      root.cwd = snapshot.cwd;
      root.startup_cmd = snapshot.startup_cmd;
      root.env = snapshot.env;
      root.pane_kind = snapshot.pane_kind;
      root.url = snapshot.url;
      root.hotkeys = snapshot.hotkeys;
      root.bg_color = snapshot.bg_color;
      root.worktree_path = snapshot.worktree_path;
      return true;
    }
    return false;
  }
  if (root.kind === "split") {
    return findAndMutatePane(root.a, id, patch) || findAndMutatePane(root.b, id, patch);
  }
  if (root.kind === "tabs") {
    for (const c of root.children) {
      if (findAndMutatePane(c, id, patch)) return true;
    }
  }
  return false;
}
