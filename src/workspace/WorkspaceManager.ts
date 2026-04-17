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
import { api } from "../ipc/bridge";
import { TerminalPane } from "../terminal/TerminalPane";
import { BrowserPane } from "../browser/BrowserPane";
import type { Pane } from "../layout/Pane";
import {
  findPane,
  newPane,
  panes,
  removePane,
  setRatioByPath,
  splitPane,
} from "../layout/LayoutTree";
import { render, type RenderContext } from "../layout/SplitContainer";

const MAX_WORKSPACES = 9;

export class WorkspaceManager {
  private config: Config;
  private shells: ShellProfile[];
  private paneCaches = new Map<number, Map<Uuid, Pane>>();
  private workspaceContainers = new Map<number, HTMLElement>();
  private activeId: number;
  private focusedPaneId: Uuid | null = null;
  private saveTimer: number | null = null;
  /// Cache of workspace containers that have already had their panes spawned
  /// on first visit, so subsequent visits are zero-cost.
  private hydrated = new Set<number>();

  constructor(
    private host: HTMLElement,
    config: Config,
    shells: ShellProfile[],
  ) {
    this.config = config;
    this.shells = shells;
    this.activeId = config.active_workspace;
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
    const handlePaneActivation = (target: EventTarget | null) => {
      const el = target as HTMLElement | null;
      if (!el) return;
      const paneEl = el.closest<HTMLElement>(".pane[data-pane-id]");
      const id = paneEl?.dataset.paneId;
      if (!id) return;
      const cache = this.paneCaches.get(this.activeId);
      const pane = cache?.get(id);
      if (pane) {
        pane.focus();
      }
      this.focusedPaneId = id;
    };
    this.host.addEventListener("focusin", (ev) => handlePaneActivation(ev.target));
    this.host.addEventListener(
      "pointerdown",
      (ev) => handlePaneActivation(ev.target),
      true, // capture phase: run before xterm.js's own handlers
    );

    await this.activate(this.activeId);
  }

  /// Switch to workspace `id`, creating it lazily if needed (up to
  /// MAX_WORKSPACES).
  async activate(id: number): Promise<void> {
    if (id < 1 || id > MAX_WORKSPACES) return;
    if (!this.workspaceContainers.has(id)) {
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
    this.persistDebounced();
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
      });
    }
    const resolvedShell = this.resolveShell(spec.shell);
    const finalSpec: PaneSpec = { ...spec, shell: resolvedShell };
    return new TerminalPane({
      spec: finalSpec,
      onFocus: () => {
        this.focusedPaneId = spec.id;
      },
      onHotKeysChange: (hotkeys) => {
        this.updatePaneSpec(spec.id, (p) => {
          p.hotkeys = hotkeys;
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
      pane_kind: "browser",
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
    const newRoot = removePane(ws.root, id);
    const cache = this.paneCaches.get(ws.id)!;
    const pane = cache.get(id);
    pane?.dispose();
    cache.delete(id);

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

  /// Request the focused pane to toggle its scrollback search bar. Only
  /// TerminalPane exposes this; for non-terminal panes the call is a no-op.
  toggleSearchOnFocused(): void {
    const id = this.focusedPaneId;
    if (!id) return;
    const pane = this.paneCaches.get(this.activeId)?.get(id);
    (pane as { toggleSearch?: () => void } | undefined)?.toggleSearch?.();
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

  /// Replace the default shell used for newly created panes.
  setDefaultShell(name: string): void {
    if (this.shells.some((s) => s.name === name)) {
      // Reorder so `name` is first — subsequent splits inherit it.
      this.shells = [
        ...this.shells.filter((s) => s.name === name),
        ...this.shells.filter((s) => s.name !== name),
      ];
    }
  }
}

// Re-export a helper for the rest of the app. Not used internally but used by
// unit tests and the main module.
export { MAX_WORKSPACES };
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
      };
      patch(snapshot);
      // Write fields back onto the tree node (which is structurally identical
      // to PaneSpec aside from the `kind: "pane"` discriminator).
      root.title = snapshot.title;
      root.shell = snapshot.shell;
      root.cwd = snapshot.cwd;
      root.startup_cmd = snapshot.startup_cmd;
      root.env = snapshot.env;
      root.pane_kind = snapshot.pane_kind;
      root.url = snapshot.url;
      root.hotkeys = snapshot.hotkeys;
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
