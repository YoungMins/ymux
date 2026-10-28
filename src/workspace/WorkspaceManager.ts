// Owns all workspaces, their layout trees, and per-workspace pane caches.
// Switching workspaces hides the previous DOM subtree without disposing any
// xterm instances, so scrollback survives — the tmux semantics the user
// explicitly asked for.

import type {
  AccountProfile,
  ActivityConfig,
  AgentSnapshot,
  Config,
  DetectedAgent,
  HotKeyDef,
  LayoutNode,
  PaneSpec,
  ShellProfile,
  SplitDir,
  SummarizerConfig,
  Uuid,
  WebhookDestination,
  Workspace,
} from "../types";
import { defaultActivity, defaultSummarizer } from "../settings/activityModel";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { api, describeError, fsApi, gitApi } from "../ipc/bridge";
import { TerminalPane, type PendingPromptState } from "../terminal/TerminalPane";
import { clampFontSize, DEFAULT_FONT_SIZE } from "./fontSize";
import { BrowserPane } from "../browser/BrowserPane";
import { EmbeddedBrowserPane } from "../browser/EmbeddedBrowserPane";
import { FilesPane } from "../files/FilesPane";
import { baseName } from "../files/fileModel";
import { EditorPane } from "../editor/EditorPane";
import { TokenMonitorPane } from "../tokenmonitor/TokenMonitor";
import { GitPane } from "../git/GitPane";
import { fileName } from "../editor/editorModel";
import { closePlan, closeResult, type Closable, type CloseChoice } from "../editor/closeGuard";
import { coldDraftEntry, orphanDraftIds } from "../editor/draft";
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
import { uuidv4, type HookDirIssue } from "../types";
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
import { getLang, t } from "../i18n/i18n";
import {
  clearAccountRefs,
  effectiveAccount,
  isManagedAccount,
  setAccountsList,
  toBadge,
  type AccountBadge,
} from "../accounts/accountModel";
import { onAutoContinueChange } from "../autocontinue/model";
import { promptWorktreeBranch, removeWorktreeFlow, type LivePane } from "../git/worktreeFlow";
import { askChoice, askConfirm, askText } from "../ui/Dialog";
import { askFanout, type FanoutRequest } from "../fanout/FanoutDialog";
import { showFanoutNotice } from "../fanout/FanoutCompare";
import { laneBranches, newRunId } from "../fanout/lanes";
import { attachNode, fanoutLayout } from "../fanout/layout";
import { laneAgents, laneSpec, plainAgentCommand } from "../fanout/plan";
import { fill, gitErrorText } from "../git/worktreeFlow";
import { shellFamilyFromExecutable, type ShellFamily } from "../terminal/shellQuote";
import { showContextMenu, type ContextMenuEntry } from "../menu/ContextMenu";
import { moveItem } from "./reorder";
import { newlyWaitingPanes, paneLabel, workspaceIdOfPane } from "./agentTree";
import {
  buildInbox,
  nextAttention,
  ReadHistory,
  type InboxCandidate,
  type InboxItem,
  type ReadItem,
} from "./inbox";
import { pickActivePaneId } from "./activePane";
import { viewerTabAction } from "./viewerTab";
import { zoomAction } from "./zoom";
import { formatWorkspaceLabel, workspaceIdAtPosition, workspacePosition } from "./workspaceLabel";
import type { PaneStatus } from "../terminal/paneStatus";
import { OfficeView } from "../office/OfficeView";
import { emitOfficeMessage, type OfficeMessageKind } from "../office/officeEvents";
import { officeLog } from "../office/officeLog";
import type { PeekSendResult } from "../office/OfficePeek";
import { keyBytes } from "../office/peekKeys";
import { addTask } from "../office/heroModel";
import { funStore } from "../office/funStore";
import type { OfficeFloor } from "../office/OfficeView";
import type { Waiting } from "../office/officeQueue";
import { launchableAgents } from "./agentLaunch";
import { openDiffReview } from "../diff/DiffReview";
import { activityApi } from "../ipc/bridge";
import { pushPopup, popPopup } from "../browser/popupBlur";

/// What the top bar's "+" launcher starts in a new tab: a shell, an optional
/// command to type (an agent), and optionally the account to run it under.
export interface TabLaunch {
  shell: string;
  startupCmd: string | null;
  account?: string;
}

const MAX_WORKSPACES = 9;

export class WorkspaceManager {
  private config: Config;
  private shells: ShellProfile[];
  private paneCaches = new Map<number, Map<Uuid, Pane>>();
  /// Tab-group chrome per workspace, keyed by `LayoutNode::Tabs.id`. Mirrors
  /// `paneCaches`: `SplitContainer.render()` rebuilds the DOM on every layout
  /// mutation, and a `PaneGroup` owns a HotKeyBar with a language
  /// subscription, so it must survive those rebuilds.
  private groupCaches = new Map<number, Map<Uuid, PaneGroup>>();
  /// Group id → the pane id of its viewer tab, the editor pane a files
  /// pane's Enter reuses (spec §3.7). Runtime-only and deliberately not
  /// persisted: after a restart that tab reloads as an ordinary editor pane
  /// on its `file_path`, and the next Enter registers a new viewer.
  private viewerTabs = new Map<Uuid, Uuid>();
  /// Panes whose close is waiting on the unsaved-changes prompt, so a second
  /// Ctrl+Shift+W during the dialog cannot dispose the pane twice.
  private closing = new Set<Uuid>();
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
  private inflightSave: Promise<void> | null = null;
  /// A save failed and the user was told; cleared by the next successful save.
  private saveFailureShown = false;
  private noticeEl: HTMLElement | null = null;
  private noticeTimer: number | null = null;
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
  /// When each pane entered its current status (ms epoch), for the inbox.
  private statusSince = new Map<Uuid, number>();
  private readHistoryLog = new ReadHistory();
  private inboxListeners = new Set<() => void>();
  private inboxFrame: number | null = null;
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
  /// When the agent list was last confirmed current: a snapshot arrived or the
  /// backend's process scan finished (ms). The fan-out gate trusts a lead only
  /// if this is newer than the pane's last output.
  private agentSnapshotAt = 0;
  /// Panes a fan-out marked as waiting because a dialog blocks them.
  private fanoutBlocked = new Set<Uuid>();
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
    setAccountsList(config.accounts ?? []);
    // The AUTO badge (and its answer log) lives in the title row.
    onAutoContinueChange(() => this.refreshAccountTitles());
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

  /// 1-based position of workspace `id` in display order (0 if unknown). This
  /// is the number shown in the panel and typed with Ctrl+Alt+N.
  workspacePosition(id: number): number {
    return workspacePosition(this.config.workspaces, id);
  }

  /// Switch to the workspace at 1-based display `position`; no-op if out of range.
  async activateByPosition(position: number): Promise<void> {
    const id = workspaceIdAtPosition(this.config.workspaces, position);
    if (id !== null) await this.activate(id);
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
    void this.sweepOrphanDrafts();
  }

  /// Switch to workspace `id`, creating it lazily if it doesn't exist yet.
  /// There is no upper bound — `Ctrl+Alt+1..9` cover the first nine (by
  /// position, see `activateByPosition`), the toolbar `+` button reaches the
  /// rest. A new workspace is appended, so its position is count + 1.
  async activate(id: number, seedRoot?: LayoutNode): Promise<void> {
    if (id < 1) return;
    let created = false;
    if (!this.workspaceContainers.has(id)) {
      created = true;
      // New workspace on demand: seed an empty pane with the default shell.
      const defaultShell = this.shells[0]?.name ?? "";
      const ws: Workspace = {
        id,
        name: `workspace-${id}`,
        // `paneNode(newPane(...))`, not a hand-written literal, so a new
        // PaneSpec field can't be missed here (CLAUDE.md rule 2).
        root: seedRoot ?? paneNode(newPane(defaultShell)),
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
  /// untouched; the shown number is positional, so numbers re-flow with the move.
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
    if (this.config.workspaces.findIndex((w) => w.id === id) < 0) return;
    // The close guard (spec §3.5): one prompt listing every unsaved editor
    // in the workspace. Asked before anything is torn down.
    const editors = [...(this.paneCaches.get(id)?.values() ?? [])].filter(
      (p): p is EditorPane => p instanceof EditorPane,
    );
    // A workspace never opened this session has no live panes, but its
    // editor panes can still have drafts on disk: list them too, and delete
    // them only once the user has said to.
    const cold = this.hydrated.has(id) ? [] : await this.coldDrafts(id);
    if (!(await this.confirmCloseEditors(editors, cold.map((c) => c.entry)))) return;
    await Promise.all(cold.map((c) => api.deleteEditorDraft(c.paneId).catch(() => {})));
    // Re-resolved after the await: the list may have changed meanwhile.
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
      for (const paneId of cache.keys()) {
        this.paneStatus.delete(paneId);
        this.forgetInbox(paneId);
      }
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
  private createPane(spec: PaneSpec): Pane {
    if (spec.pane_kind === "token_usage") {
      return new TokenMonitorPane({ id: spec.id, title: spec.title, ownChrome: groupOfPane(this.active.root, spec.id) === null, onFocus: () => { this.focusedPaneId = spec.id; } });
    }
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
    if (spec.pane_kind === "git") {
      return new GitPane({
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
        onActivePaneChange: (cb) => this.onActivePaneChange(cb),
        // Follow the pane the user works in — but only in this pane's own
        // workspace (a hidden workspace's git pane must not chase the
        // visible one's terminals), and never itself.
        followTarget: () => {
          const id = this.activePaneId();
          if (!id || id === spec.id) return null;
          return this.workspaceOfPane(id) === this.workspaceOfPane(spec.id) ? id : null;
        },
        worktreeBaseDir: () => this.worktreeBaseDir,
        livePanes: () => this.livePanes(),
        openTerminal: (dir) => this.splitTerminalAt(spec.id, dir),
      });
    }
    if (spec.pane_kind === "editor") {
      return new EditorPane({
        id: spec.id,
        filePath: spec.file_path ?? "",
        title: spec.title ?? null,
        ownChrome: groupOfPane(this.active.root, spec.id) === null,
        fontSize: this.fontSize,
        onFocus: () => {
          this.focusedPaneId = spec.id;
        },
        onPathChange: (path) => {
          this.updatePaneSpec(spec.id, (p) => {
            p.file_path = path;
          });
          this.refreshTabChrome();
        },
        onDirtyChange: () => {
          this.refreshTabChrome();
          this.notifyTree();
        },
      });
    }
    const resolvedShell = this.resolveShell(spec.shell);
    const finalSpec: PaneSpec = { ...spec, shell: resolvedShell };
    return new TerminalPane({
      spec: finalSpec,
      shellExecutable: this.shells.find((s) => s.name === resolvedShell)?.executable,
      ownChrome: groupOfPane(this.active.root, spec.id) === null,
      fontSize: this.fontSize,
      onFocus: () => {
        this.focusedPaneId = spec.id;
      },
      // Badge only: `accountBadgeFor` also asks the pane `isAccountPending()`,
      // which calls back into this callback -- infinite recursion.
      accountBadge: () => this.accountBadgeOnly(spec.id),
      spawnAccounts: () => ({
        account: this.getPaneSpec(spec.id)?.account ?? spec.account ?? "",
        workspaceAccount: this.workspaceAccountOfPane(spec.id),
      }),
      beforeSpawn: () => this.flushPendingSave(),
      onAccountChange: (account) => this.setPaneAccount(spec.id, account),
      onAttention: (msg) => this.handleAttention(spec.id, msg),
      openInViewer: (path) => this.openFileInViewerTab(path, spec.id),
      isVisible: () => this.isPaneVisible(spec.id),
      onContextMenu: (ev) => this.showPaneContextMenu(spec.id, ev),
      persistScrollback: () => this.persistScrollback,
      bottomAnchor: () => this.bottomAnchor,
      onAcknowledged: () => {
        const item = this.buildInboxItem(spec.id);
        if (item) {
          this.readHistoryLog.push({
            paneId: item.paneId,
            readAt: Date.now(),
            status: item.status,
            agentKind: item.agentKind,
          });
        }
      },
      onStatusChange: (status) => {
        this.paneStatus.set(spec.id, status);
        this.statusSince.set(spec.id, Date.now());
        this.scheduleInboxChange();
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

  // ── Accounts (CLAUDE_CONFIG_DIR / CODEX_HOME profiles) ────────────────────
  // The backend injects the env vars when a PTY is spawned, so a change here
  // reaches new terminals only; a live pane's badge is flagged "pending".

  get accounts(): readonly AccountProfile[] {
    return this.config.accounts ?? [];
  }

  /// Replace the profile list (Settings → Accounts) and repaint every badge.
  setAccounts(next: AccountProfile[]): void {
    this.config.accounts = next;
    setAccountsList(next);
    this.refreshAccountTitles();
    this.persistDebounced();
  }

  /// Ask, then delete a profile: panes that used it fall back to "inherit"
  /// and workspaces to "no default", so nothing keeps pointing at a dead id.
  /// The save is flushed before the login folder goes, and the folder is
  /// removed only when ymux made it (`isManagedAccount`) — a hand-entered
  /// directory is the user's own login and is never touched. Resolves to
  /// whether the account was deleted.
  async deleteAccount(id: string): Promise<boolean> {
    const profile = this.accounts.find((a) => a.id === id);
    if (!profile) return false;
    const ok = await askConfirm(
      fill(t("account.deleteConfirm"), { name: profile.name || profile.id }),
      t("settings.accounts.delete"),
    );
    if (!ok) return false;
    this.removeAccount(id);
    await this.flushPendingSave();
    if (isManagedAccount(profile)) {
      try {
        await api.removeAccountDirs(id);
      } catch (e) {
        console.error("remove_account_dirs failed:", e);
        this.notify(fill(t("account.removeDirsFailed"), { error: describeError(e) }));
      }
    }
    return true;
  }

  private removeAccount(id: string): void {
    clearAccountRefs(this.config.workspaces, id);
    for (const cache of this.paneCaches.values()) {
      for (const pane of cache.values()) {
        if (pane instanceof TerminalPane) {
          pane.setAccount(this.getPaneSpec(pane.id)?.account ?? "");
        }
      }
    }
    this.setAccounts(this.accounts.filter((a) => a.id !== id));
  }

  getWorkspaceAccount(wsId: number): string {
    return this.config.workspaces.find((w) => w.id === wsId)?.account ?? "";
  }

  setWorkspaceAccount(wsId: number, account: string): void {
    const ws = this.config.workspaces.find((w) => w.id === wsId);
    if (!ws || (ws.account ?? "") === account) return;
    ws.account = account;
    this.refreshAccountTitles();
    this.persistDebounced();
  }

  setPaneAccount(paneId: Uuid, account: string): void {
    this.updatePaneSpec(paneId, (p) => {
      p.account = account;
    });
    const pane = this.findPaneById(paneId);
    if (pane instanceof TerminalPane) pane.setAccount(account);
    this.refreshAccountTitles();
  }

  private workspaceAccountOfPane(paneId: Uuid): string {
    const wsId = workspaceIdOfPane(this.config.workspaces, paneId);
    return wsId === null ? "" : this.getWorkspaceAccount(wsId);
  }

  /// The badge for a pane's title row (the effective account), nothing else.
  /// Must not touch the pane: `TerminalPane.isAccountPending()` calls this.
  private accountBadgeOnly(paneId: Uuid): AccountBadge | null {
    return toBadge(
      effectiveAccount(
        this.accounts,
        this.getPaneSpec(paneId)?.account,
        this.workspaceAccountOfPane(paneId),
      ),
    );
  }

  /// The badge for a pane's title row and whether the running PTY predates it.
  private accountBadgeFor(paneId: Uuid): { badge: AccountBadge | null; pending: boolean } {
    const pane = this.findPaneById(paneId);
    return {
      badge: this.accountBadgeOnly(paneId),
      pending: pane instanceof TerminalPane && pane.isAccountPending(),
    };
  }

  /// Repaint every title row that can show an account badge.
  private refreshAccountTitles(): void {
    for (const [wsId, cache] of this.paneCaches) {
      for (const pane of cache.values()) {
        if (pane instanceof TerminalPane) pane.paintTitle();
      }
      for (const group of this.groupsFor(wsId).values()) group.refreshLabels();
    }
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
      if (
        pane instanceof TerminalPane ||
        pane instanceof FilesPane ||
        pane instanceof EditorPane ||
        pane instanceof GitPane ||
        pane instanceof TokenMonitorPane
      ) {
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
          onAccountChange: (paneId, account) => this.setPaneAccount(paneId, account),
          accountBadgeOf: (paneId) => this.accountBadgeFor(paneId),
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

  /// Build and open the terminal right-click menu for `paneId`. The files,
  /// editor and git entries split the clicked pane and inherit its cwd.
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
      { label: t("editor.split"), onSelect: () => void this.splitFocusedEditor("horizontal") },
      { label: t("git.here"), onSelect: () => void this.splitFocusedGit("horizontal") },
    ];
    showContextMenu(ev.clientX, ev.clientY, entries);
  }

  /// Split the currently focused pane.
  /// `cwd` starts the new pane there instead of in the parent's directory.
  /// Returns the new pane's id (null when nothing was split).
  async splitFocused(direction: SplitDir, cwd?: string): Promise<Uuid | null> {
    const ws = this.active;
    const focusId = this.activePaneId();
    if (!focusId) return null;
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
    const inheritedCwd = cwd ?? liveCwd ?? existing?.cwd ?? null;
    const spec = newPane(shellName, inheritedCwd);
    // A target the tree cannot split must not leave a live, unplaced pane.
    const next = splitPane(ws.root, focusId, direction, spec);
    if (!findPane(next, spec.id)) return null;
    ws.root = next;

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
    return spec.id;
  }

  /// Open a tab next to the focused pane, wrapping it in a group first if it
  /// has none (`Ctrl+Shift+T`, the palette).
  ///
  /// `launch` is the top bar's "+" launcher: the tab runs that shell and
  /// types `startupCmd` (an agent) instead of copying the source's shell.
  /// Naming its own shell, it can join any visible terminal's group when
  /// focus is on a non-terminal pane; with no terminal on screen it splits
  /// one in beside the focused pane rather than doing nothing.
  /// `launch.account`, when set, is the account the launcher's picker chose
  /// (`ACCOUNT_NONE` for the default login); otherwise the tab inherits the
  /// source pane's.
  async newTabInFocused(launch?: TabLaunch): Promise<void> {
    const ws = this.active;
    // Not `focusedPaneId`: it still points into the previous workspace after
    // a switch, which made this a silent no-op (`activePane.ts`).
    let sourceId = this.activePaneId();
    if (!sourceId) return;
    const source = findPane(ws.root, sourceId);
    if (launch && (source?.pane_kind ?? "terminal") !== "terminal") {
      const term = visiblePanes(ws.root).find((p) => (p.pane_kind ?? "terminal") === "terminal");
      if (!term) {
        const spec = newPane(this.resolveShell(launch.shell), source?.cwd ?? null);
        spec.startup_cmd = launch.startupCmd;
        if (launch.account !== undefined) spec.account = launch.account;
        await this.insertSplit(ws, sourceId, "horizontal", spec, "launcher split failed");
        return;
      }
      sourceId = term.id;
    }
    await this.addTabFrom(ws, sourceId, launch);
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
  private async addTabFrom(
    ws: Workspace,
    sourceId: Uuid,
    launch?: TabLaunch,
  ): Promise<void> {
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
    const spec = newPane(
      this.resolveShell(launch?.shell ?? source.shell),
      liveCwd ?? source.cwd ?? null,
    );
    if (launch) spec.startup_cmd = launch.startupCmd;
    spec.hotkeys = (source.hotkeys ?? []).map((h) => ({ ...h }));
    spec.bg_color = source.bg_color ?? "";
    spec.account = launch?.account ?? source.account ?? "";
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
      // A Cancel at an unsaved-changes prompt stops the whole sweep: the
      // user just said "not that one", not "skip it and carry on".
      if (!(await this.closePane(id))) return;
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

  /// A files pane pressed Enter on a text file (spec §3.7). It goes into the
  /// viewer tab of `activePaneId()`'s group: for a files pane in the layout
  /// that is its own group (the editor opens beside the list); for the dock,
  /// whose pane is outside every layout tree and never becomes the active
  /// one, it is the pane that was active before the dock took focus.
  ///
  /// One viewer tab per group: the second Enter re-points the same editor
  /// pane (`EditorPane.openFile`) rather than opening another tab. No PTY is
  /// killed or respawned any more — the editor asks before replacing an
  /// unsaved buffer, and keeps it if the user cancels.
  ///
  /// `anchorId` targets a specific pane's group instead — a terminal whose
  /// output had a Markdown path Ctrl/Cmd+clicked passes its own id, so the
  /// viewer opens beside *that* terminal even if another pane holds focus.
  /// An anchor that is not in the active workspace falls back to
  /// `activePaneId()`, as `splitTerminalAt` does.
  async openFileInViewerTab(path: string, anchorId?: Uuid): Promise<void> {
    if (!path) return;
    const ws = this.active;
    const targetId =
      anchorId && findPane(ws.root, anchorId) ? anchorId : this.activePaneId();
    if (!targetId) return;
    let group = groupOfPane(ws.root, targetId);
    if (!group) {
      ws.root = wrapInTabs(ws.root, targetId, uuidv4());
      group = groupOfPane(ws.root, targetId);
      if (!group) return;
    }
    const cache = this.paneCaches.get(ws.id)!;
    const action = viewerTabAction(this.viewerTabs.get(group.id), tabIds(group));
    const viewer = action.kind === "reuse" ? cache.get(action.paneId) : undefined;
    if (action.kind === "reuse" && viewer instanceof EditorPane) {
      // Show the tab first, so the unsaved-changes prompt (if any) is asked
      // over the buffer it is about.
      const next = activateTabFor(ws.root, action.paneId);
      if (next !== ws.root) {
        ws.root = next;
        this.renderWorkspace(ws);
        this.persistDebounced();
      }
      await viewer.openFile(path);
      return;
    }
    const spec = newPane("", null);
    spec.pane_kind = "editor";
    spec.file_path = path;
    ws.root = addTab(ws.root, group.id, spec);
    this.viewerTabs.set(group.id, spec.id);
    const pane = this.createPane(spec);
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

  /// The label a tab shows: the user's title, else the running program from
  /// the process scan, else the shell name (`src/terminal/tabLabel.ts`).
  tabLabelFor(paneId: Uuid): string {
    const spec = this.getPaneSpec(paneId);
    if (spec?.pane_kind === "token_usage") return spec.title || t("usage.title");
    // A files pane has no shell or process: its label is the folder it shows.
    if (spec?.pane_kind === "files") {
      return spec.title || (spec.cwd ? baseName(spec.cwd) : t("files.title"));
    }
    // A git pane: its repository's folder name (the pane keeps the root in cwd).
    if (spec?.pane_kind === "git") {
      const pane = this.findPaneById(paneId);
      if (pane instanceof GitPane) return pane.label();
      return spec.title || (spec.cwd ? baseName(spec.cwd) : t("git.title"));
    }
    // An editor pane: its file's name, marked while it has unsaved edits.
    if (spec?.pane_kind === "editor") {
      const base = spec.title || (spec.file_path ? fileName(spec.file_path) : t("editor.untitled"));
      const pane = this.findPaneById(paneId);
      return pane instanceof EditorPane && pane.isDirty() ? `${base} ●` : base;
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
    const focusId = this.activePaneId();
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
    // Check the split can land *before* creating anything on disk: the pane
    // may have closed while the prompt was up, and a split that no-ops
    // would strand the new worktree and branch.
    const probe = newPane("", null);
    if (!findPane(splitPane(ws.root, focusId, direction, probe), probe.id)) return;

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
    // A target the tree cannot split must not leave a live, unplaced pane.
    const next = splitPane(ws.root, focusId, direction, spec);
    if (!findPane(next, spec.id)) return;
    ws.root = next;

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

  // ── Fan-out ───────────────────────────────────────────────────────────────
  /// Serialises the timeout dialogs: `Dialog` shows one at a time and a second
  /// would cancel the first.
  private fanoutDialogs: Promise<void> = Promise.resolve();

  /// "Fan out prompt to agents…": ask, then `startFanout`.
  async openFanout(): Promise<void> {
    const notify = (body: string) =>
      void api.notify(t("fanout.command"), body).catch(() => {});
    const cwd = await this.focusedCwd();
    if (!cwd) return notify(t("worktree.noCwd"));
    let repo: string;
    let branches;
    try {
      repo = await gitApi.repoRoot(cwd);
      branches = await gitApi.branches(repo);
    } catch {
      return notify(t("worktree.notRepo"));
    }
    const detected = await api.detectAgents().catch(() => []);
    const req = await askFanout({
      repo,
      branches,
      agents: laneAgents(detected),
      accounts: this.accounts,
    });
    if (req) await this.startFanout(req, detected);
  }

  /// Create one worktree per lane, lay the lanes out, start the plain agent
  /// CLIs, and hand each its prompt once it is ready. A lane that cannot be
  /// created is reported; the ones that were created are kept and started.
  /// The base is resolved to one sha first, so every lane starts from the
  /// same commit.
  async startFanout(req: FanoutRequest, detected: readonly DetectedAgent[]): Promise<void> {
    const ack = (body: string) =>
      askChoice(t("fanout.title"), body, [{ id: "ok", label: t("dialog.ok"), primary: true }]);
    let start: string;
    try {
      start = await gitApi.resolveCommit(req.repo, req.base);
    } catch (e) {
      await ack(fill(t("fanout.laneFailed"), { lanes: `${req.base}: ${gitErrorText(e)}` }));
      return;
    }
    const runId = newRunId(new Date(), Math.random);
    const existing = await gitApi
      .branches(req.repo)
      .then((b) => b.local)
      .catch(() => [] as string[]);
    const branches = laneBranches(req.prefix, runId, req.lanes, existing);
    const shell = this.resolveShell(this.shells[0]?.name ?? "");
    const family = shellFamilyFromExecutable(this.shells.find((s) => s.name === shell)?.executable);
    const specs: PaneSpec[] = [];
    const failed: string[] = [];
    for (let i = 0; i < req.lanes.length; i++) {
      const lane = req.lanes[i];
      const agent = detected.find((a) => a.id === lane.agent);
      const command = agent ? plainAgentCommand(agent, family) : null;
      if (!agent || command === null) {
        failed.push(`${branches[i]}: ${fill(t("fanout.noAgentCmd"), { agent: lane.agent })}`);
        continue;
      }
      try {
        const path = await gitApi.worktreeAddFrom(
          req.repo,
          branches[i],
          this.worktreeBaseDir,
          start,
          runId,
        );
        specs.push(
          laneSpec({ shell, path, command, account: lane.account, title: branches[i] }),
        );
      } catch (e) {
        failed.push(`${branches[i]}: ${gitErrorText(e)}`);
      }
    }
    if (failed.length > 0) await ack(fill(t("fanout.laneFailed"), { lanes: failed.join("\n") }));
    if (specs.length === 0) return;

    const wsId = await this.placeFanout(specs, req.newWorkspace);
    if (wsId === null) {
      // The worktrees exist; never leave them unannounced.
      await ack(
        fill(t("fanout.placeFailed"), {
          lanes: specs.map((s) => `${s.title ?? ""}: ${s.worktree_path ?? s.cwd ?? ""}`).join("\n"),
        }),
      );
      return;
    }

    const cache = this.paneCaches.get(wsId);
    for (const spec of specs) {
      const pane = cache?.get(spec.id);
      if (!(pane instanceof TerminalPane)) continue;
      const lane = spec.title ?? "";
      pane.setPendingPrompt(req.prompt, {
        lead: () => this._agents[spec.id]?.lead ?? null,
        snapshotAt: () => this.agentSnapshotAt,
        submit: req.submit,
        onState: (s) => this.onFanoutLaneState(spec.id, lane, req.prompt, s),
      });
    }
    showFanoutNotice(specs.length);
  }

  /// Put the lane panes on screen and spawn them, trying in order: beside the
  /// focused pane (or a new workspace when asked and one is free), then a new
  /// workspace, then a tab group beside the whole workspace. Returns the
  /// workspace they landed in, or null when nothing worked.
  private async placeFanout(specs: PaneSpec[], newWorkspace: boolean): Promise<number | null> {
    const room = () => this.config.workspaces.length < MAX_WORKSPACES;
    const separate = newWorkspace && room();
    const layout = fanoutLayout(specs, newWorkspace && !separate);

    const inNewWorkspace = async (): Promise<number | null> => {
      if (!room()) return null;
      const id = this.lowestFreeId();
      await this.activate(id, layout);
      return id;
    };
    const adopt = async (root: LayoutNode): Promise<number> => {
      const ws = this.active;
      ws.root = root;
      const cache = this.paneCaches.get(ws.id)!;
      for (const spec of specs) cache.set(spec.id, this.createPane(spec));
      this.renderWorkspace(ws);
      for (const spec of specs) {
        try {
          await cache.get(spec.id)?.spawn();
        } catch (e) {
          console.error("fanout spawn failed", e);
        }
      }
      this.persistDebounced();
      return ws.id;
    };
    const beside = async (): Promise<number | null> => {
      const anchor = this.activePaneId();
      const next = anchor ? attachNode(this.active.root, anchor, layout) : this.active.root;
      return next === this.active.root ? null : adopt(next);
    };
    const tabsBesideRoot = async (): Promise<number | null> =>
      adopt({
        kind: "split",
        direction: "horizontal",
        ratio: 0.5,
        a: this.active.root,
        b: fanoutLayout(specs, true),
      });

    const attempts = separate
      ? [inNewWorkspace, beside, tabsBesideRoot]
      : [beside, inNewWorkspace, tabsBesideRoot];
    for (const attempt of attempts) {
      try {
        const id = await attempt();
        if (id !== null) return id;
      } catch (e) {
        console.error("fanout placement failed", e);
      }
    }
    return null;
  }

  private onFanoutLaneState(
    paneId: Uuid,
    lane: string,
    prompt: string,
    state: PendingPromptState,
  ): void {
    const notify = (body: string) =>
      void api.notify(t("fanout.command"), body).catch(() => {});
    // Leaving "blocked": take back the attention markWaiting raised.
    if (state !== "blocked" && this.fanoutBlocked.delete(paneId)) {
      const pane = this.findPaneById(paneId);
      if (pane instanceof TerminalPane) pane.markWaitingCleared();
      this.notifyTree();
    }
    if (state === "blocked") {
      // Show up in the inbox / status dots, and say what to do.
      this.fanoutBlocked.add(paneId);
      const pane = this.findPaneById(paneId);
      if (pane instanceof TerminalPane) pane.markWaiting();
      this.notifyTree();
      notify(fill(t("fanout.blocked"), { lane }));
    }
    if (state === "sent" || state === "pasted") this.deliverToOffice(null, paneId, "prompt", prompt);
    if (state === "check") notify(fill(t("fanout.check"), { lane }));
    else if (state === "pasted") notify(fill(t("fanout.pasted"), { lane }));
    else if (state === "failed") notify(fill(t("fanout.sendFailed"), { lane }));
    else if (state === "unsent") {
      notify(fill(t("fanout.unsent"), { lane }));
      this.fanoutDialogs = this.fanoutDialogs.then(async () => {
        const a = await askChoice(fill(t("fanout.unsent"), { lane }), null, [
          { id: "enter", label: t("fanout.pressEnter"), primary: true },
          { id: "close", label: t("dialog.cancel") },
        ]);
        if (a?.id === "enter" && !(await this.submitPending(paneId, prompt))) {
          notify(`${lane}: ${t("fanout.notSent")}`);
        }
      });
    } else if (state === "timeout") {
      this.fanoutDialogs = this.fanoutDialogs.then(async () => {
        const a = await askChoice(fill(t("fanout.timeout"), { lane }), null, [
          { id: "send", label: t("fanout.sendNow"), primary: true },
          { id: "copy", label: t("fanout.copy") },
          { id: "close", label: t("dialog.cancel") },
        ]);
        if (a?.id === "copy") {
          await navigator.clipboard.writeText(prompt).catch(() => {});
        } else if (a?.id === "send") {
          const res = await this.sendToPane(paneId, prompt, true, undefined, { from: null, kind: "prompt" });
          if (res === "pasted") notify(fill(t("fanout.unsent"), { lane }));
          else if (res !== "sent") notify(`${lane}: ${t("fanout.notSent")}`);
        }
      });
    }
  }

  /// Press Enter for a prompt already typed into `paneId` (`echoOf`), under
  /// the same conditions as a fan-out submit: a lead present and not waiting,
  /// no dialog showing, and a fresh agent snapshot after the keypress is due.
  private async submitPending(paneId: Uuid, echoOf: string): Promise<boolean> {
    const pane = this.findPaneById(paneId);
    if (!(pane instanceof TerminalPane)) return false;
    const fit = () => {
      const l = this._agents[paneId]?.lead;
      return !!l && l.status !== "waiting" && pane.screenBlock(echoOf) === null;
    };
    if (!fit()) return false;
    return pane.submitAfterPaste(0, fit, () => this.agentSnapshotAt);
  }

  /// Split the focused pane and drop a browser pane into the new slot instead
  /// of a terminal. URL defaults to `about:blank` so the user can type into
  /// the URL bar.
  async splitFocusedBrowser(direction: SplitDir, url: string = ""): Promise<void> {
    const ws = this.active;
    const focusId = this.activePaneId();
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
    // A target the tree cannot split must not leave a live, unplaced pane.
    const next = splitPane(ws.root, focusId, direction, spec);
    if (!findPane(next, spec.id)) return;
    ws.root = next;
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
    const focusId = this.activePaneId();
    if (!focusId) return;
    const liveCwd = await api.getPaneCwd(focusId).catch(() => null);
    const spec = newPane("", liveCwd ?? findPane(ws.root, focusId)?.cwd ?? null);
    spec.pane_kind = "files";
    await this.insertSplit(ws, focusId, direction, spec, "files split failed");
  }

  /// Add a persistent monitor beside the focused pane.
  async splitFocusedTokenUsage(direction: SplitDir): Promise<void> {
    const ws = this.active;
    const focusId = this.activePaneId();
    if (!focusId) return;
    const spec = newPane("", null);
    spec.pane_kind = "token_usage";
    await this.insertSplit(ws, focusId, direction, spec, "token usage split failed");
  }

  /// Open the repository of the focused pane in a following Git pane.
  async splitFocusedGit(direction: SplitDir): Promise<void> {
    const ws = this.active;
    const focusId = this.activePaneId();
    if (!focusId) return;
    const liveCwd = await api.getPaneCwd(focusId).catch(() => null);
    const spec = newPane("", liveCwd ?? findPane(ws.root, focusId)?.cwd ?? null);
    spec.pane_kind = "git";
    await this.insertSplit(ws, focusId, direction, spec, "git split failed");
  }

  /// Split the focused pane and open an empty editor pane in the new slot
  /// (its empty state offers "Open file…"). Files reach an editor mostly
  /// through a files pane's Enter (`openFileInViewerTab`).
  async splitFocusedEditor(direction: SplitDir): Promise<void> {
    const ws = this.active;
    const focusId = this.activePaneId();
    if (!focusId) return;
    const spec = newPane("", null);
    spec.pane_kind = "editor";
    spec.file_path = "";
    await this.insertSplit(ws, focusId, direction, spec, "editor split failed");
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
    // A target the tree cannot split must not leave a live, unplaced pane.
    const next = splitPane(ws.root, targetId, direction, spec);
    if (!findPane(next, spec.id)) return;
    ws.root = next;
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
  ///
  /// Resolves `false` when the pane stayed open — the unsaved-changes guard
  /// (spec §3.5) said no, or a close of it is already waiting on that guard.
  private async closePane(id: Uuid): Promise<boolean> {
    // The guard runs first, before anything about the tree is captured: the
    // prompt is async, and the layout (even the active workspace) can change
    // while it is open.
    if (this.closing.has(id)) return false;
    const guarded = this.findPaneById(id);
    if (guarded?.canClose) {
      this.closing.add(id);
      let ok = false;
      try {
        ok = await guarded.canClose();
      } catch (e) {
        console.error("close guard failed; keeping the pane", e);
      } finally {
        this.closing.delete(id);
      }
      if (!ok) return false;
    }
    const wsId = this.workspaceOfPane(id);
    const ws = this.config.workspaces.find((w) => w.id === wsId);
    if (!ws) return false;
    const isActive = ws.id === this.activeId;
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
    this.forgetInbox(id);

    if (newRoot === null) {
      // Workspace would be empty; create a replacement pane so there is
      // always something to look at.
      const defaultShell = this.resolveShell(this.shells[0]?.name ?? "");
      const spec = newPane(defaultShell);
      ws.root = paneNode(spec);
      const replacement = this.createPane(spec);
      cache.set(spec.id, replacement);
      this.renderWorkspace(ws);
      // The tree is already mutated: a failed spawn must not skip the save
      // and the worktree offer below (the pane shows the error itself).
      try {
        await replacement.spawn();
        if (isActive) replacement.focus();
      } catch (e) {
        console.error("replacement pane spawn failed", e);
      }
    } else {
      ws.root = newRoot;
      if (isActive) this.focusedPaneId = null;
      // Stay inside the group when one of its tabs was closed; otherwise fall
      // back to the first pane on screen, in depth-first order, so the new
      // focus is predictable rather than Map-insertion dependent.
      const nextId =
        siblings.find((s) => findPane(ws.root, s)) ?? visiblePanes(ws.root)[0]?.id;
      if (nextId) ws.root = activateTabFor(ws.root, nextId);
      this.renderWorkspace(ws);
      if (nextId && isActive) cache.get(nextId)?.focus();
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
    return true;
  }

  /// Every live editor pane, in every workspace — hidden workspaces keep
  /// their panes alive, and their unsaved edits with them.
  private allEditors(): EditorPane[] {
    const out: EditorPane[] = [];
    for (const cache of this.paneCaches.values()) {
      for (const pane of cache.values()) if (pane instanceof EditorPane) out.push(pane);
    }
    return out;
  }

  /// Write every editor draft still waiting in its debounce, now.
  async flushDrafts(): Promise<void> {
    await Promise.all(this.allEditors().map((e) => e.flushDraft()));
  }

  /// Any unsaved editor anywhere (the `beforeunload` check).
  hasUnsavedEditors(): boolean {
    return this.allEditors().some((e) => e.isDirty());
  }

  /// The close guard for several editors at once (a workspace, the window):
  /// one prompt listing every unsaved file (closeGuard.closePlan). A single
  /// dirty editor asks with its own prompt. Resolves true to go ahead.
  /// `cold` lists drafts of editor panes that were never mounted (a
  /// workspace deleted before it was opened): listed as unsaved, never
  /// savable from here.
  private async confirmCloseEditors(editors: EditorPane[], cold: Closable[] = []): Promise<boolean> {
    const plan = closePlan([
      // `hasPath` here means "Save can protect it" (a pending recovered
      // draft cannot be saved from a close prompt).
      ...editors.map((e) => ({ name: e.displayName(), dirty: e.isDirty(), hasPath: e.canSaveOnClose() })),
      ...cold,
    ]);
    if (plan.kind === "close") return true;
    const dirty = editors.filter((e) => e.isDirty());
    if (dirty.length === 1 && cold.length === 0) return dirty[0].canClose();
    const labels: Record<CloseChoice, string> = {
      save: t("editor.saveAll"),
      discard: t("editor.dontSave"),
      cancel: t("dialog.cancel"),
    };
    const ans = await askChoice(
      t("editor.closePromptMany").replace("{n}", String(plan.names.length)),
      plan.names.join("\n"),
      plan.choices.map((id) => ({ id, label: labels[id], primary: id === "save" })),
    );
    const answer = (ans?.id as CloseChoice | undefined) ?? null;
    if (answer !== "save") return closeResult(answer);
    const saved: boolean[] = [];
    for (const e of dirty) {
      const ok = await e.save();
      saved.push(ok);
      // Stop at the first save that did not land (a conflict the user
      // cancelled, a permission error): nothing closes, so asking about the
      // rest would only be noise.
      if (!ok) break;
    }
    return closeResult(answer, saved);
  }

  /// Drafts of the editor panes in workspace `wsId`'s layout, read without
  /// mounting anything (for a workspace that was never opened).
  private async coldDrafts(wsId: number): Promise<{ paneId: Uuid; entry: Closable }[]> {
    const ws = this.config.workspaces.find((w) => w.id === wsId);
    if (!ws) return [];
    const out: { paneId: Uuid; entry: Closable }[] = [];
    for (const spec of panes(ws.root)) {
      if (spec.pane_kind !== "editor") continue;
      const entry = coldDraftEntry(await api.loadEditorDraft(spec.id).catch(() => ""));
      if (entry) out.push({ paneId: spec.id, entry });
    }
    return out;
  }

  /// Startup sweep: delete drafts whose pane id exists nowhere in the config
  /// (closed before its draft was looked for, or the config edited by hand).
  /// A draft of any pane still in the config — opened or not — survives.
  private async sweepOrphanDrafts(): Promise<void> {
    const ids = await api.listEditorDrafts().catch(() => [] as Uuid[]);
    const live = this.config.workspaces.flatMap((w) => panes(w.root).map((p) => p.id));
    for (const id of orphanDraftIds(ids, live)) {
      await api.deleteEditorDraft(id).catch(() => {});
    }
  }

  /// The app is quitting (main.ts's `ymux://quit-requested` listener —
  /// closing the window only hides it to the tray). Every editor in
  /// every workspace is asked about, once. On a go-ahead, the drafts of
  /// buffers the user chose not to save are dropped, so the next launch does
  /// not offer back edits that were explicitly discarded.
  async confirmCloseAll(): Promise<boolean> {
    const editors = this.allEditors();
    if (!(await this.confirmCloseEditors(editors))) return false;
    await Promise.all(editors.filter((e) => e.isDirty()).map((e) => e.discardDraft()));
    return true;
  }

  /// Offer to remove the git worktree at `wtPath`, through the git pane's
  /// removal flow (src/git/worktreeFlow.ts): one confirmation that lists what
  /// goes with it, `--force` only when the listed changes demand it, and a
  /// failure shown rather than retried with `--force`. (This used to force on
  /// *any* failure — on Windows that includes a directory still in use.)
  /// Never throws: cleanup must not fail the pane close / workspace delete
  /// that triggered it.
  private async offerWorktreeRemoval(wtPath: string): Promise<void> {
    let entry;
    try {
      // Asked from inside it, the worktree itself is the `current` entry —
      // the comparison is done in Rust (rule 15).
      entry = (await gitApi.worktrees(wtPath)).find((w) => w.current);
    } catch (e) {
      // Already gone, or no longer a repository: nothing to remove.
      console.warn("worktree lookup failed", wtPath, e);
      return;
    }
    if (!entry) return;
    try {
      // The pane that showed it is closing, so it is not "shown" any more.
      await removeWorktreeFlow({ ...entry, current: false }, () => this.livePanes());
    } catch (e) {
      console.error("worktree remove failed", e);
    }
  }

  /// Every live pane in every workspace (the ones with a pane object — a
  /// workspace never opened this session has nothing running), with the
  /// directory or file it works in: a terminal's live OSC 7 cwd (else its
  /// stored one), a files or git pane's folder, an editor's file. Browser
  /// panes have none. Paths are raw — the worktree check compares them in
  /// Rust (rule 15).
  async livePanes(): Promise<LivePane[]> {
    const out: LivePane[] = [];
    for (const [wsId, cache] of this.paneCaches) {
      const ws = this.config.workspaces.find((w) => w.id === wsId);
      if (!ws) continue;
      for (const id of cache.keys()) {
        const spec = findPane(ws.root, id);
        if (!spec) continue;
        const kind = spec.pane_kind ?? "terminal";
        let path: string | null = null;
        if (kind === "terminal") path = (await api.getPaneCwd(id).catch(() => null)) ?? spec.cwd ?? null;
        else if (kind === "files" || kind === "git") path = spec.cwd ?? null;
        else if (kind === "editor") path = spec.file_path || null;
        if (path) out.push({ label: `${ws.name}: ${this.tabLabelFor(id)}`, path });
      }
    }
    return out;
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

  // ── Office view ───────────────────────────────────────────────────────
  // The active workspace drawn as an office floor (src/office/). An overlay
  // over the split view: the panes underneath stay mounted and running.

  private office: OfficeView | null = null;
  private officeListeners = new Set<(open: boolean) => void>();

  get officeOpen(): boolean {
    return this.office !== null;
  }

  /// Subscribe to the office opening/closing (the top bar's toggle).
  onOfficeChange(cb: (open: boolean) => void): () => void {
    this.officeListeners.add(cb);
    return () => {
      this.officeListeners.delete(cb);
    };
  }

  toggleOffice(): void {
    // A mini office in the corner grows back first.
    if (this.office?.mini) this.office.setMini(false);
    else if (this.office) this.closeOffice();
    else this.openOffice();
  }

  openOffice(): void {
    if (this.office) return;
    funStore.setActivity(this.activityConfig.enabled);
    // Embedded browsers are OS-level child webviews that would float over
    // the overlay; the popup counter hides them until it closes.
    pushPopup();
    this.office = new OfficeView({
      input: (wsId) => {
        const ws = this.config.workspaces.find((w) => w.id === wsId) ?? this.active;
        return {
          root: ws.root,
          agents: this._agents,
          statusOf: (id) => this.paneStatus.get(id) ?? "idle",
          // Terminals by their tab label; other panes as the workspace tree
          // names them ("Browser", "Git", …).
          labelOf: (id) => {
            const spec = findPane(ws.root, id);
            return spec && (spec.pane_kind ?? "terminal") !== "terminal"
              ? paneLabel(spec, this.inboxLabels())
              : this.tabLabelFor(id);
          },
          focused: ws.id === this.activeId ? this.activePaneId() : null,
          mailOf: (id) => officeLog.mailOf(id),
        };
      },
      floors: () => this.officeFloors(),
      activeFloor: () => this.activeId,
      floorOf: (id) => this.workspaceOfPane(id),
      waiting: () => this.officeWaiting(),
      outputAt: (id) => {
        const pane = this.findPaneById(id);
        return pane instanceof TerminalPane ? pane.outputAt : null;
      },
      toolSince: (id) => officeLog.toolSince(id),
      statusSince: (id) => officeLog.statusSince(id),
      toolCalls: (id) => officeLog.toolCalls(id),
      review: (id) => this.reviewFromOffice(id),
      launchers: (id) => this.officeLaunchers(id),
      launch: (id, command) => {
        const pane = this.findPaneById(id);
        if (pane instanceof TerminalPane) pane.typeText(`${command}\r`);
      },
      setMini: (mini) => {
        // Small in a corner, the panes are back in use: embedded browsers
        // may show again.
        if (mini) {
          popPopup();
          const id = this.activePaneId();
          if (id) void this.focusPane(id);
        } else pushPopup();
      },
      log: () => officeLog.list(),
      workspaceName: (wsId) => {
        const ws = this.config.workspaces.find((w) => w.id === wsId) ?? this.active;
        return formatWorkspaceLabel(ws.id, ws.name ?? null, this.workspacePosition(ws.id));
      },
      openPane: (id) => this.closeOffice(id),
      close: () => this.closeOffice(),
      isTerminal: (id) => this.findPaneById(id) instanceof TerminalPane,
      screen: (id) => {
        const pane = this.findPaneById(id);
        return pane instanceof TerminalPane ? pane.screenText() : null;
      },
      send: (id, text) => this.sendFromOffice(id, text),
      // Same resolution as the backend's spawn: the pane's own pick, else
      // the workspace default; "" is the default account.
      accountOf: (id) => this.getPaneSpec(id)?.account || this.workspaceAccountOfPane(id) || "",
      accounts: () => this.accounts,
      tokenUsage: () => api.getTokenUsage(),
      key: (id, key) => {
        const pane = this.findPaneById(id);
        if (pane instanceof TerminalPane) pane.typeText(keyBytes(key, pane.applicationCursorKeys));
      },
      type: (id, data) => {
        const pane = this.findPaneById(id);
        if (pane instanceof TerminalPane) pane.typeText(data);
      },
      cursorKeys: (id) => {
        const pane = this.findPaneById(id);
        return pane instanceof TerminalPane && pane.applicationCursorKeys;
      },
      subscribe: (cb) => {
        const offs = [this.onTreeChange(cb), this.onInboxChange(cb), this.onActivePaneChange(cb), officeLog.onChange(cb)];
        return () => offs.forEach((off) => off());
      },
    });
    this.host.appendChild(this.office.element);
    this.office.focus();
    for (const cb of this.officeListeners) cb(true);
  }

  /// A prompt typed into the office view's peek: the user is looking at the
  /// pane's screen, so unlike `sendToPane` there is no readiness gate. Pasted
  /// (bracketed) and submitted where the app takes pastes; a single line is
  /// typed into one that doesn't. Refused while a dialog is up, where a paste
  /// could answer it: the peek's keys are for that.
  private async sendFromOffice(paneId: Uuid, text: string): Promise<PeekSendResult> {
    const pane = this.findPaneById(paneId);
    if (!(pane instanceof TerminalPane)) return "gone";
    if (pane.screenBlock() !== null) return "blocked";
    const lead = this._agents[paneId]?.lead;
    if (pane.pasteText(text)) {
      if (!(await pane.pasteFlushed())) return "failed";
      // An agent gets a letter carried over; a plain shell has no turn that
      // would ever file it.
      if (lead) this.deliverToOffice(null, paneId, "prompt", text);
      return (await pane.submitAfterPaste(300)) ? "sent" : "failed";
    }
    if (text.includes("\n")) return "multiline";
    pane.typeText(`${text}\r`);
    if (lead) this.deliverToOffice(null, paneId, "prompt", text);
    return "sent";
  }

  /// Every workspace as a floor of the office, with how many of its agents
  /// wait for the user and how many work.
  private officeFloors(): OfficeFloor[] {
    return this.config.workspaces.map((ws) => {
      let waiting = 0;
      let working = 0;
      const walk = (n: LayoutNode): void => {
        if (n.kind === "pane") {
          const lead = this._agents[n.id]?.lead;
          if (this.paneStatus.get(n.id) === "attention" || lead?.status === "waiting") waiting++;
          else if (lead?.status === "working") working++;
        } else if (n.kind === "tabs") n.children.forEach(walk);
        else {
          walk(n.a);
          walk(n.b);
        }
      };
      walk(ws.root);
      return {
        id: ws.id,
        label: formatWorkspaceLabel(ws.id, ws.name ?? null, this.workspacePosition(ws.id)),
        position: this.workspacePosition(ws.id),
        waiting,
        working,
      };
    });
  }

  /// Agents waiting for the user, in every workspace, with since when.
  private officeWaiting(): Waiting[] {
    const out: Waiting[] = [];
    for (const ws of this.config.workspaces) {
      const walk = (n: LayoutNode): void => {
        if (n.kind === "pane") {
          const lead = this._agents[n.id]?.lead;
          if (this.paneStatus.get(n.id) === "attention" || lead?.status === "waiting") {
            out.push({
              paneId: n.id,
              ws: ws.id,
              label: this.tabLabelFor(n.id),
              since: this.statusSince.get(n.id) ?? officeLog.statusSince(n.id),
            });
          }
        } else if (n.kind === "tabs") n.children.forEach(walk);
        else {
          walk(n.a);
          walk(n.b);
        }
      };
      walk(ws.root);
    }
    return out;
  }

  /// The diff review of the repository a pane works in (the office's flag).
  private async reviewFromOffice(paneId: Uuid): Promise<"ok" | "no-repo"> {
    let cwd: string | null = null;
    try {
      cwd = (await api.getPaneCwd(paneId)) || null;
    } catch {
      /* not spawned or no OSC 7: the saved cwd */
    }
    cwd ??= this.getPaneSpec(paneId)?.cwd || null;
    if (!cwd) return "no-repo";
    try {
      await openDiffReview(await gitApi.repoRoot(cwd));
      // Reviewed: the done flag comes down.
      const pane = this.findPaneById(paneId);
      if (pane instanceof TerminalPane) pane.acknowledge();
      return "ok";
    } catch {
      return "no-repo";
    }
  }

  /// The agents an empty desk can start, as the command lines for its shell.
  private async officeLaunchers(paneId: Uuid): Promise<{ label: string; command: string }[]> {
    const spec = this.getPaneSpec(paneId);
    if (!spec || (spec.pane_kind ?? "terminal") !== "terminal") return [];
    const shell = this.resolveShell(spec.shell);
    const family = shellFamilyFromExecutable(this.shells.find((s) => s.name === shell)?.executable);
    const detected = await api.detectAgents().catch(() => []);
    return launchableAgents(detected, family).map(({ agent, command }) => ({ label: agent.name, command }));
  }

  /// Work handed to `to`: walked over in the office view, a note on its
  /// board and a letter in `to`'s mailbox (`officeLog`).
  private deliverToOffice(from: Uuid | null, to: Uuid, kind: OfficeMessageKind, text: string): void {
    officeLog.message(
      {
        kind,
        to,
        label: this.tabLabelFor(to),
        agent: this._agents[to]?.lead?.kind ?? null,
        from,
        fromLabel: from ? this.tabLabelFor(from) : null,
      },
      text,
    );
    emitOfficeMessage({ from, to, kind });
  }

  /// The office log follows the agents: a turn starting opens a pane's
  /// letters, a turn ending files them, and finishing leaves a note.
  private noteAgentsForOffice(prev: AgentSnapshot, next: AgentSnapshot): void {
    const ids = new Set([...Object.keys(prev), ...Object.keys(next)]);
    for (const id of ids) {
      const before = prev[id]?.lead?.status;
      const after = next[id]?.lead?.status;
      const lead = next[id]?.lead;
      if (before !== after) {
        if (after === "working") officeLog.agentWorking(id);
        else if (before === "working") officeLog.agentStopped(id);
        if (after === "done") {
          const started = before === "working" ? officeLog.statusSince(id) : null;
          const ms = started === null ? null : Date.now() - started;
          officeLog.done(id, this.tabLabelFor(id), lead?.kind ?? null, ms);
          funStore.record({ t: "turn", kind: lead?.kind ?? "agent", ms, at: Date.now() });
          // A finished task is experience for the account's hero.
          addTask(this.getPaneSpec(id)?.account || this.workspaceAccountOfPane(id) || "");
          this.describeTurnSoon(id);
        }
      }
      if (lead?.tool && lead.tool !== prev[id]?.lead?.tool) funStore.record({ t: "tool", tool: lead.tool });
      officeLog.track(id, lead?.status ?? null, lead?.tool ?? null);
      // Helpers: how many work for this lead now, and how many are new.
      const subs = next[id]?.subagents ?? [];
      const known = new Set((prev[id]?.subagents ?? []).map((x) => x.id));
      const started = subs.filter((x) => !known.has(x.id)).length;
      const busy = subs.filter((x) => x.status === "working").length;
      if (started > 0 || busy > 0) funStore.record({ t: "subs", n: busy, started });
    }
    const working = Object.values(next).filter((a) => a?.lead?.status === "working").length;
    if (working >= 2) funStore.record({ t: "concurrent", n: working });
  }

  /// The activity log's account of `paneId`'s turn, once its worker has
  /// written it, onto the board's note. Quietly nothing while the log is off.
  private describeTurnSoon(paneId: Uuid): void {
    if (!this.activityConfig.enabled) return;
    const at = Date.now();
    setTimeout(() => {
      void activityApi
        .read(80)
        .then((records) => {
          const end = [...records].reverse().find((r) => r.pane === paneId && r.kind === "turn_end" && r.ts >= at - 120_000);
          if (!end) return;
          const d = end.data as Record<string, unknown>;
          const stats = (d.stats ?? {}) as Record<string, unknown>;
          const summary = (d.summary ?? {}) as Record<string, unknown>;
          const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);
          const test = (d.test ?? null) as Record<string, unknown> | null;
          const passed = test && typeof test.passed === "boolean" ? test.passed : null;
          officeLog.describeTurn(paneId, {
            headline: typeof summary.headline === "string" ? summary.headline : "",
            durationMs: num(d.duration_ms),
            files: num(stats.files),
            add: num(stats.insertions),
            del: num(stats.deletions),
            test: passed,
          });
          funStore.record({ t: "facts", pane: paneId, files: num(stats.files), add: num(stats.insertions), del: num(stats.deletions), test: passed });
        })
        .catch(() => {});
    }, 2500);
  }

  /// Close the office; focus `paneId` (in whatever workspace it lives), or
  /// the pane that was active before.
  closeOffice(paneId?: Uuid): void {
    if (!this.office) return;
    const mini = this.office.mini;
    this.office.dispose();
    this.office = null;
    if (!mini) popPopup();
    for (const cb of this.officeListeners) cb(false);
    const target = paneId ?? this.activePaneId();
    if (target) void this.focusPane(target);
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
    this.scheduleInboxChange();
  }

  // ── Attention inbox ───────────────────────────────────────────────────────

  private forgetInbox(paneId: Uuid): void {
    officeLog.clearMail(paneId);
    this.statusSince.delete(paneId);
    this.readHistoryLog.remove(paneId);
    this.scheduleInboxChange();
  }

  /// Subscribe to inbox changes (status, tree, closes), coalesced to at most
  /// one call per animation frame. Returns an unsubscribe function.
  onInboxChange(cb: () => void): () => void {
    this.inboxListeners.add(cb);
    return () => {
      this.inboxListeners.delete(cb);
    };
  }

  private scheduleInboxChange(): void {
    if (this.inboxFrame !== null || this.inboxListeners.size === 0) return;
    this.inboxFrame = requestAnimationFrame(() => {
      this.inboxFrame = null;
      for (const cb of this.inboxListeners) cb();
    });
  }

  private inboxLabels() {
    return {
      terminal: t("terminal.defaultTitle"),
      browser: t("tree.browser"),
      subagent: t("tree.subagent"),
      git: t("git.title"),
      token_usage: t("usage.title"),
    };
  }

  /// The inbox row for one pane, or null when it is not in attention/done (or
  /// is not in a hydrated workspace).
  private buildInboxItem(paneId: Uuid, labels = this.inboxLabels()): InboxItem | null {
    const status = this.paneStatus.get(paneId) ?? "idle";
    if (status !== "attention" && status !== "done") return null;
    const wsId = this.workspaceOfPane(paneId);
    if (wsId === null) return null;
    const ws = this.config.workspaces.find((w) => w.id === wsId);
    const spec = ws ? findPane(ws.root, paneId) : null;
    if (!ws || !spec) return null;
    return {
      paneId,
      wsId,
      wsLabel: formatWorkspaceLabel(wsId, ws.name ?? null, this.workspacePosition(wsId)),
      paneLabel: paneLabel(spec, labels),
      agentKind: this._agents[paneId]?.lead?.kind ?? null,
      status,
      since: this.statusSince.get(paneId) ?? Date.now(),
    };
  }

  /// Every pane in attention/done across hydrated workspaces, attention first.
  inboxItems(): InboxItem[] {
    const labels = this.inboxLabels();
    const rows: InboxCandidate[] = [];
    for (const cache of this.paneCaches.values()) {
      for (const id of cache.keys()) {
        const item = this.buildInboxItem(id, labels);
        if (item) rows.push(item);
      }
    }
    return buildInbox(rows);
  }

  /// Recently read items with labels resolved now; panes that no longer exist
  /// are skipped.
  readHistory(): ReadItem[] {
    const labels = this.inboxLabels();
    const out: ReadItem[] = [];
    for (const e of this.readHistoryLog.list()) {
      const wsId = this.workspaceOfPane(e.paneId);
      if (wsId === null) continue;
      const ws = this.config.workspaces.find((w) => w.id === wsId);
      const spec = ws ? findPane(ws.root, e.paneId) : null;
      if (!ws || !spec) continue;
      out.push({
        ...e,
        wsLabel: formatWorkspaceLabel(wsId, ws.name ?? null, this.workspacePosition(wsId)),
        paneLabel: paneLabel(spec, labels),
      });
    }
    return out;
  }

  /// Jump to the next pane that needs the user. The target is computed before
  /// `focusPane`, because focusing clears the status the inbox is built from.
  focusNextAttention(): boolean {
    const target = nextAttention(this.inboxItems(), this.focusedPaneId);
    if (!target) return false;
    void this.focusPane(target);
    return true;
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
    this.noteAgentsForOffice(this._agents, next);
    // Arrival time says nothing about when the process list was sampled, so
    // only the scan heartbeat (`noteAgentScan`) advances `agentSnapshotAt`.
    this._agents = next;
    this.notifyTree();
  }

  /// The backend finished a process scan (whether or not the agents changed).
  /// `sampledAtMs`: when it read the process list (epoch ms), not when this
  /// event arrived; a scan sampled before a paste cannot vouch for the lead
  /// afterwards.
  noteAgentScan(sampledAtMs: number): void {
    if (Number.isFinite(sampledAtMs) && sampledAtMs > this.agentSnapshotAt) {
      this.agentSnapshotAt = sampledAtMs;
    }
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

  /// "Workspace › pane — agent" for the picker that chooses which agent
  /// receives a review.
  paneChoiceLabel(paneId: Uuid): string {
    const wsId = this.workspaceOfPane(paneId);
    const ws =
      wsId === null
        ? ""
        : formatWorkspaceLabel(wsId, this.getWorkspaceName(wsId), this.workspacePosition(wsId));
    const agent = this._agents[paneId]?.lead?.kind ?? "";
    return `${ws} › ${this.tabLabelFor(paneId)}${agent ? ` — ${agent}` : ""}`;
  }

  /// The kind of agent leading `paneId` right now ("claude", ...), if any.
  leadKindOf(paneId: Uuid): string | undefined {
    return this._agents[paneId]?.lead?.kind;
  }

  /// Paste `text` into a pane running an agent and focus it. `gone` when the
  /// pane is not a live terminal, has no lead agent, or (with `expectKind`)
  /// its agent is no longer of that kind; `waiting` while the agent is on a
  /// permission prompt or a dialog is showing, where a pasted block would
  /// answer it; `no-paste` when the app has not turned bracketed paste on;
  /// `failed` when a paste write was rejected. Nothing is written on a refusal.
  /// `sent`: pasted and Enter written. `pasted`: pasted, no Enter (`submit`
  /// was false, or the Enter was withheld because the agent changed or no
  /// agent snapshot newer than the paste confirmed it). The Enter is
  /// re-checked against the same conditions right before it is written.
  ///
  /// `origin`, when given, is shown in the office view as someone carrying
  /// the work over (`officeEvents`) once it is pasted.
  async sendToPane(
    paneId: Uuid,
    text: string,
    submit: boolean,
    expectKind?: string,
    origin?: { from: Uuid | null; kind: OfficeMessageKind },
  ): Promise<"sent" | "pasted" | "waiting" | "no-paste" | "gone" | "failed"> {
    const pane = this.findPaneById(paneId);
    if (!(pane instanceof TerminalPane)) return "gone";
    const lead = () => this._agents[paneId]?.lead;
    const fit = (): "gone" | "waiting" | null => {
      const l = lead();
      if (!l || (expectKind !== undefined && l.kind !== expectKind)) return "gone";
      return l.status === "waiting" ? "waiting" : null;
    };
    const refusal = fit();
    if (refusal) return refusal;
    // A dialog (trust, select menu) needs the user: a paste could answer it.
    if (pane.screenBlock() !== null) return "waiting";
    if (!pane.pasteText(text)) return "no-paste";
    if (!(await pane.pasteFlushed())) return "failed";
    if (origin) this.deliverToOffice(origin.from, paneId, origin.kind, text);
    await this.focusPane(paneId);
    if (!submit) return "pasted";
    const entered = await pane.submitAfterPaste(
      300,
      () => fit() === null && pane.screenBlock(text) === null,
      () => this.agentSnapshotAt,
    );
    return entered ? "sent" : "pasted";
  }

  /// Terminal panes (any workspace) whose directory is inside `dir`. The
  /// containment test is the backend's (`ypath` keys, rule 15).
  async terminalPanesWithin(dir: string): Promise<{ id: Uuid; hasAgent: boolean; shell: ShellFamily }[]> {
    const found: { id: Uuid; cwd: string; shell: string }[] = [];
    for (const [wsId, cache] of this.paneCaches) {
      const ws = this.config.workspaces.find((w) => w.id === wsId);
      if (!ws) continue;
      for (const [id, pane] of cache) {
        if (!(pane instanceof TerminalPane)) continue;
        const spec = findPane(ws.root, id);
        const cwd = (await api.getPaneCwd(id).catch(() => null)) ?? spec?.cwd ?? null;
        if (cwd) found.push({ id, cwd, shell: spec?.shell ?? "" });
      }
    }
    if (found.length === 0) return [];
    let inside: boolean[];
    try {
      inside = await fsApi.pathsWithin(dir, found.map((f) => f.cwd));
    } catch {
      return [];
    }
    return found
      .filter((_, i) => inside[i])
      .map((f) => ({
        id: f.id,
        hasAgent: !!this._agents[f.id]?.lead,
        shell: shellFamilyFromExecutable(this.shells.find((s) => s.name === f.shell)?.executable),
      }));
  }

  /// Resolves when the terminal pane `paneId` (just spawned) has shown its
  /// first prompt, or after the 10 s cap.
  async waitForFirstPrompt(paneId: Uuid): Promise<void> {
    const pane = this.findPaneById(paneId);
    if (pane instanceof TerminalPane) await pane.waitForFirstPrompt();
  }

  /// The shell family of pane `paneId`'s configured shell (for quoting).
  paneShellFamily(paneId: Uuid): ShellFamily {
    return shellFamilyFromExecutable(this.paneShellExecutable(paneId));
  }

  /// The executable of pane `paneId`'s configured shell, if known.
  private paneShellExecutable(paneId: Uuid): string | undefined {
    for (const [wsId] of this.paneCaches) {
      const ws = this.config.workspaces.find((w) => w.id === wsId);
      const spec = ws ? findPane(ws.root, paneId) : null;
      if (spec) return this.shells.find((s) => s.name === spec.shell)?.executable;
    }
    return undefined;
  }

  /// Type `text` into a plain shell pane WITHOUT pressing Enter, as a bracketed
  /// paste. Refused (never written) for text holding a control character (it
  /// could act as Enter or an escape sequence), a pane running an agent, a
  /// command or a dialog, and a pane whose app has not turned bracketed paste
  /// on (a program that is not a line editor).
  async typeIntoShell(
    paneId: Uuid,
    text: string,
  ): Promise<"ok" | "control" | "busy" | "no-paste" | "gone" | "failed"> {
    const pane = this.findPaneById(paneId);
    if (!(pane instanceof TerminalPane)) return "gone";
    // eslint-disable-next-line no-control-regex
    if (/[\u0000-\u001f\u007f-\u009f]/.test(text)) return "control";
    if (this._agents[paneId]?.lead || pane.status === "running" || pane.screenBlock() !== null) {
      return "busy";
    }
    if (!pane.bracketedPaste || !pane.pasteText(text)) {
      // A shell with no bracketed paste (cmd, say): the one fixed merge
      // command may go as plain keystrokes once the prompt is provably back.
      if (!(await pane.typePlain(text, this.paneShellExecutable(paneId)))) return "no-paste";
    } else if (!(await pane.pasteFlushed())) {
      return "failed";
    }
    await this.focusPane(paneId);
    return "ok";
  }

  /// Directory of the focused pane: the live cwd when the shell reports one,
  /// else the saved one. Null when there is no focused pane or no directory.
  async focusedCwd(): Promise<string | null> {
    const id = this.activePaneId();
    if (!id) return null;
    try {
      const live = await api.getPaneCwd(id);
      if (live) return live;
    } catch {
      /* not spawned yet or no OSC 7: use the saved cwd */
    }
    return this.getPaneSpec(id)?.cwd || null;
  }

  get agentTracking(): boolean {
    return this.config.agent_tracking ?? false;
  }

  /// Install/remove the Claude Code hooks, then record the choice. Rejects
  /// (setting unchanged) if the backend couldn't write settings.json.
  async setAgentTracking(enabled: boolean): Promise<HookDirIssue[]> {
    const issues = await api.setAgentTracking(enabled);
    this.config.agent_tracking = enabled;
    this.persistDebounced();
    // The workspace panel's "tracking is off" hint keys off this.
    this.notifyTree();
    return issues;
  }

  /// The window was hidden to the tray (`ymux://main-hidden`): nothing in it
  /// is on screen, whether or not the webview saw a DOM `blur`.
  noteWindowHidden(): void {
    this.windowFocused = false;
  }

  /// The window was shown again (`ymux://main-shown`, after the backend
  /// focused it; or `visibilitychange` → visible, with `document.hasFocus()`).
  /// Needed because a native `set_focus` often raises no DOM `focus` in
  /// WebView2, which would leave every pane counted unseen.
  noteWindowShown(focused: boolean): void {
    this.windowFocused = focused;
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
    const label = formatWorkspaceLabel(wsId, name, this.workspacePosition(wsId));
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

  /// Type dropped file `paths` into whichever terminal pane sits under the
  /// given viewport point, focusing it first, each path quoted for that
  /// pane's shell. Used by the file drag-and-drop handler so a drop lands in
  /// the pane the user aimed at rather than the focused one. Non-terminal
  /// panes (browser) and points outside any pane are ignored.
  dropPathsAt(clientX: number, clientY: number, paths: readonly string[]): void {
    const hit = document.elementFromPoint(clientX, clientY);
    const paneId = hit?.closest<HTMLElement>("[data-pane-id]")?.dataset.paneId;
    if (!paneId) return;
    const pane = this.paneCaches.get(this.activeId)?.get(paneId);
    if (!(pane instanceof TerminalPane)) return;
    pane.focus();
    pane.typePaths(paths);
  }

  /// Save the current config to disk. Debounced by 500 ms so rapid changes
  /// collapse into a single write.
  private persistDebounced(): void {
    // Every layout / pane-metadata mutation funnels through here — the tree follows it.
    this.notifyTree();
    if (this.saveTimer !== null) {
      clearTimeout(this.saveTimer);
    }
    // Through `flush()` so the timer's save is tracked in `inflightSave`
    // (and serialised behind any save already running) — otherwise
    // `flushPendingSave()` could not wait for it.
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      void this.flush();
    }, 500) as unknown as number;
  }

  /// Mirror the UI language into `activity.language` so the backend's rule-based
  /// summaries follow it. No-op when unchanged (or the config predates the field).
  setActivityLanguage(lang: string): void {
    const a = this.config.activity;
    if (!a || a.language === lang) return;
    a.language = lang;
    this.persistDebounced();
  }

  // ── Activity log / summarizer / webhooks (Settings -> Activity) ───────────
  // Plain config sections: the backend reads them from the saved config, so
  // each setter just stores and schedules a save. Secrets are NOT here.

  get activityConfig(): ActivityConfig {
    return this.config.activity ?? defaultActivity(getLang());
  }

  setActivityConfig(next: ActivityConfig): void {
    this.config.activity = next;
    funStore.setActivity(next.enabled);
    this.persistDebounced();
  }

  get summarizerConfig(): SummarizerConfig {
    return this.config.summarizer ?? defaultSummarizer();
  }

  setSummarizerConfig(next: SummarizerConfig): void {
    this.config.summarizer = next;
    this.persistDebounced();
  }

  get webhookDestinations(): readonly WebhookDestination[] {
    return this.config.webhooks ?? [];
  }

  setWebhookDestinations(next: WebhookDestination[]): void {
    this.config.webhooks = next;
    this.persistDebounced();
  }

  /// Display name of a workspace for the activity list; "" when it no longer exists.
  workspaceName(wsId: number): string {
    return this.config.workspaces.find((w) => w.id === wsId)?.name ?? "";
  }

  /// Await any debounced save so the backend's config matches the UI before
  /// it acts on it (spawn_pane resolves account ids against the saved config).
  /// No-op when nothing is pending.
  flushPendingSave(): Promise<void> {
    if (this.saveTimer === null) return this.inflightSave ?? Promise.resolve();
    return this.flush();
  }

  /// Flush pending save immediately. Used on window close. Saves are
  /// serialised: a new one waits for the one in flight, so two writes can
  /// never land out of order, and it serialises `this.config` only once it
  /// runs, so the later write always carries the newer state. Never rejects.
  async flush(): Promise<void> {
    if (this.saveTimer !== null) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    const prev = this.inflightSave;
    const p = (async () => {
      if (prev) await prev;
      try {
        await api.saveConfig(this.config);
        this.saveFailureShown = false;
      } catch (e) {
        console.error("saveConfig failed", e);
        // Once per failure streak: a save that keeps failing on every
        // keystroke must not stack notices; a later success re-arms it.
        if (!this.saveFailureShown) {
          this.saveFailureShown = true;
          this.notify(fill(t("app.saveFailed"), { error: describeError(e) }));
        }
      }
    })();
    this.inflightSave = p;
    await p;
    if (this.inflightSave === p) this.inflightSave = null;
  }

  /// Show a non-blocking, dismissible notice at the bottom right (replacing
  /// any previous one). For failures the user should know about but that
  /// need no decision — a failed save, a folder that could not be removed.
  notify(message: string): void {
    this.noticeEl?.remove();
    if (this.noticeTimer !== null) window.clearTimeout(this.noticeTimer);
    const box = document.createElement("div");
    box.className = "app-notice";
    box.setAttribute("role", "alert");
    const msg = document.createElement("span");
    msg.className = "app-notice__msg";
    msg.textContent = message;
    const x = document.createElement("button");
    x.type = "button";
    x.className = "app-notice__close";
    x.textContent = "×";
    x.setAttribute("aria-label", t("settings.close"));
    const dismiss = (): void => {
      if (this.noticeTimer !== null) window.clearTimeout(this.noticeTimer);
      this.noticeTimer = null;
      box.remove();
      if (this.noticeEl === box) this.noticeEl = null;
    };
    x.addEventListener("click", dismiss);
    box.append(msg, x);
    document.body.appendChild(box);
    this.noticeEl = box;
    this.noticeTimer = window.setTimeout(dismiss, 12_000);
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
        // The editor follows the same Ctrl +/-/0 (spec §0.6).
        if (pane instanceof TerminalPane || pane instanceof EditorPane) pane.setFontSize(next);
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
