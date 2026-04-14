// Owns all workspaces, their layout trees, and per-workspace pane caches.
// Switching workspaces hides the previous DOM subtree without disposing any
// xterm instances, so scrollback survives — the tmux semantics the user
// explicitly asked for.

import type {
  Config,
  LayoutNode,
  PaneSpec,
  ShellProfile,
  SplitDir,
  Uuid,
  Workspace,
} from "../types";
import { api } from "../ipc/bridge";
import { TerminalPane } from "../terminal/TerminalPane";
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
  private paneCaches = new Map<number, Map<Uuid, TerminalPane>>();
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
      const resolvedShell = this.resolveShell(spec.shell);
      const finalSpec: PaneSpec = { ...spec, shell: resolvedShell };
      const pane = new TerminalPane({
        spec: finalSpec,
        onFocus: () => {
          this.focusedPaneId = spec.id;
        },
      });
      cache.set(spec.id, pane);
    }
    // Re-render now that panes exist in cache.
    this.renderWorkspace(ws);
    // Spawn shells sequentially to avoid hammering the system.
    for (const pane of cache.values()) {
      try {
        await pane.spawn();
      } catch (e) {
        console.error(`spawn failed`, e);
      }
    }
    if (!this.focusedPaneId && cache.size > 0) {
      const first = cache.values().next().value as TerminalPane | undefined;
      first?.focus();
    }
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
    const shellName = this.resolveShell(existing?.shell ?? this.shells[0]?.name ?? "");
    const spec = newPane(shellName, existing?.cwd ?? null);
    ws.root = splitPane(ws.root, focusId, direction, spec);

    // Create a new TerminalPane for the new spec and spawn it.
    const cache = this.paneCaches.get(ws.id)!;
    const pane = new TerminalPane({
      spec,
      onFocus: () => {
        this.focusedPaneId = spec.id;
      },
    });
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
      };
      const replacement = new TerminalPane({
        spec,
        onFocus: () => {
          this.focusedPaneId = spec.id;
        },
      });
      cache.set(spec.id, replacement);
      this.renderWorkspace(ws);
      await replacement.spawn();
      replacement.focus();
    } else {
      ws.root = newRoot;
      this.renderWorkspace(ws);
      // Move focus to the first remaining pane.
      this.focusedPaneId = null;
      const first = cache.values().next().value as TerminalPane | undefined;
      first?.focus();
    }
    this.persistDebounced();
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
