import type { WorkspaceManager } from "./WorkspaceManager";
import { formatWorkspaceLabel } from "./workspaceLabel";
import { insertIndexFromMidpoints } from "./reorder";
import {
  toggle as toggleNotes,
  hasNotes,
  onNotesChange,
} from "../notes/NotesOverlay";
import { t, onLangChange } from "../i18n/i18n";
import { askText, askConfirm } from "../ui/Dialog";
import type { Uuid } from "../types";
import {
  buildAgentTree,
  isExpanded,
  parseExpanded,
  EXPANDED_KEY,
  type AgentRow,
  type ExpandedMap,
  type PaneRow,
  type TabRow,
  type TreeLabels,
} from "./agentTree";

const COLLAPSE_KEY = "ymux:workspace-panel:collapsed";

/// Vertical travel (px) before a press turns into a reorder drag. Below this a
/// press is still a plain click, so switching workspaces stays a single tap.
const DRAG_THRESHOLD_PX = 4;

const noteIconSvg = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><line x1="10" y1="9" x2="8" y2="9"/></svg>`;

function wsTooltip(id: number, manager: WorkspaceManager): string {
  const name = manager.getWorkspaceName(id);
  const base = name ? `${id}: ${name}` : `Workspace ${id}`;
  return `${base} (Ctrl+Alt+${id}) — ${t("workspace.dblclickRename")}, ${t("workspace.dragReorder")}`;
}

/// Read the persisted collapsed flag (default: expanded). localStorage may
/// throw in some webview contexts, so treat any failure as "expanded".
function readCollapsed(): boolean {
  try {
    return localStorage.getItem(COLLAPSE_KEY) === "1";
  } catch {
    return false;
  }
}

function writeCollapsed(collapsed: boolean): void {
  try {
    localStorage.setItem(COLLAPSE_KEY, collapsed ? "1" : "0");
  } catch {
    /* localStorage unavailable — collapse just won't persist */
  }
}

/// Per-workspace tree expansion, persisted under `ymux.workspaceTree.expanded`.
function readExpanded(): ExpandedMap {
  try {
    return parseExpanded(localStorage.getItem(EXPANDED_KEY));
  } catch {
    return {};
  }
}

function writeExpanded(map: ExpandedMap): void {
  try {
    localStorage.setItem(EXPANDED_KEY, JSON.stringify(map));
  } catch {
    /* localStorage unavailable — expansion just won't persist */
  }
}

function treeLabels(): TreeLabels {
  return {
    terminal: t("terminal.defaultTitle"),
    browser: t("tree.browser"),
    subagent: t("tree.subagent"),
  };
}

export function mountWorkspacePanel(
  host: HTMLElement,
  manager: WorkspaceManager,
): () => void {
  const panel = document.createElement("div");
  panel.className = "workspace-panel";
  if (readCollapsed()) panel.classList.add("workspace-panel--collapsed");

  const header = document.createElement("div");
  header.className = "workspace-panel__header";
  header.textContent = t("workspace.panelTitle");
  panel.appendChild(header);

  const list = document.createElement("div");
  list.className = "workspace-panel__list";
  panel.appendChild(list);

  const addBtn = document.createElement("button");
  addBtn.className = "workspace-panel__add";
  addBtn.type = "button";
  addBtn.textContent = "+";
  addBtn.title = t("workspace.addWorkspace");
  addBtn.setAttribute("aria-label", t("workspace.addWorkspace"));
  addBtn.addEventListener("click", () => {
    void manager.addWorkspace(); // fires onWorkspacesChange → rebuild()
  });
  panel.appendChild(addBtn);

  const buttons = new Map<number, HTMLButtonElement>();
  const noteButtons = new Map<number, HTMLButtonElement>();
  /// Rows in render order, so a row's array index *is* its position in
  /// `manager.workspaces` — what `moveWorkspace` takes.
  const rows: HTMLElement[] = [];
  /// Per-workspace container for pane + agent rows. A *sibling* after the
  /// workspace row, never inside it: `rows` must stay workspace-rows-only
  /// because its indices feed `moveWorkspace`, and a press inside a
  /// `.workspace-panel__row` would start a workspace drag.
  const childHosts = new Map<number, HTMLElement>();
  const carets = new Map<number, HTMLButtonElement>();
  let expanded: ExpandedMap = readExpanded();

  // ── Drag-to-reorder ────────────────────────────────────────────────
  // Pointer events, not the HTML5 drag-and-drop API: Tauri's native
  // drag-drop is enabled (main.ts's `onDragDropEvent` powers file-drop-onto-
  // terminal), and on Windows/WebView2 that disables HTML5 DnD inside the
  // webview entirely. Pointer events are unaffected by it.
  let drag: {
    fromIndex: number;
    row: HTMLElement;
    startY: number;
    started: boolean;
    insertBefore: number;
  } | null = null;
  /// Set when a real drag ends so the trailing `click` doesn't also read as a
  /// press that switches workspaces. Cleared on the next pointerdown rather
  /// than a timer — the click/timer ordering isn't guaranteed, the next
  /// pointerdown always is. Only matters when the drop was a no-op (no
  /// rebuild, so the pressed button is still in the DOM to receive the click).
  let dragJustEnded = false;

  function clearDropMarkers(): void {
    for (const r of rows) {
      r.classList.remove(
        "workspace-panel__row--drop-above",
        "workspace-panel__row--drop-below",
      );
    }
  }

  /// Recompute where the dragged row would land from the pointer's Y against
  /// each row's live midpoint, and draw the insertion line there.
  function updateDropTarget(y: number): void {
    if (!drag) return;
    const midpoints = rows.map((r) => {
      const box = r.getBoundingClientRect();
      return box.top + box.height / 2;
    });
    drag.insertBefore = insertIndexFromMidpoints(midpoints, y);
    clearDropMarkers();
    if (drag.insertBefore < rows.length) {
      rows[drag.insertBefore].classList.add("workspace-panel__row--drop-above");
    } else if (rows.length > 0) {
      rows[rows.length - 1].classList.add("workspace-panel__row--drop-below");
    }
  }

  const onDragMove = (ev: PointerEvent): void => {
    if (!drag) return;
    if (!drag.started) {
      if (Math.abs(ev.clientY - drag.startY) < DRAG_THRESHOLD_PX) return;
      drag.started = true;
      drag.row.classList.add("workspace-panel__row--dragging");
    }
    ev.preventDefault();
    updateDropTarget(ev.clientY);
  };

  const onDragEnd = (): void => {
    window.removeEventListener("pointermove", onDragMove);
    window.removeEventListener("pointerup", onDragEnd);
    window.removeEventListener("pointercancel", onDragEnd);
    const d = drag;
    drag = null;
    if (!d) return;
    d.row.classList.remove("workspace-panel__row--dragging");
    clearDropMarkers();
    if (!d.started) return;
    dragJustEnded = true;
    // Fires onWorkspacesChange → rebuild() when it actually moved something.
    manager.moveWorkspace(d.fromIndex, d.insertBefore);
  };

  /// Build one vertical row: switch button (label + status tint) with a note
  /// button and a delete button trailing it.
  function makeRow(id: number): HTMLElement {
    const row = document.createElement("div");
    row.className = "workspace-panel__row";
    row.addEventListener("pointerdown", (ev) => {
      dragJustEnded = false; // a fresh press always re-arms clicking
      if (ev.button !== 0) return;
      // Never start a drag off the delete button — that click must stay exact.
      if ((ev.target as HTMLElement | null)?.closest(".workspace-panel__del, .workspace-panel__caret")) {
        return;
      }
      const fromIndex = rows.indexOf(row);
      if (fromIndex < 0) return;
      drag = { fromIndex, row, startY: ev.clientY, started: false, insertBefore: fromIndex };
      window.addEventListener("pointermove", onDragMove);
      window.addEventListener("pointerup", onDragEnd);
      window.addEventListener("pointercancel", onDragEnd);
    });

    const caret = document.createElement("button");
    caret.className = "workspace-panel__caret";
    caret.type = "button";
    caret.addEventListener("click", (ev) => {
      ev.stopPropagation();
      if (dragJustEnded) return;
      expanded = { ...expanded, [String(id)]: !isExpanded(expanded, id) };
      writeExpanded(expanded);
      renderTree();
    });
    row.appendChild(caret);
    carets.set(id, caret);

    const btn = document.createElement("button");
    btn.className = "workspace-panel__ws";
    btn.textContent = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
    btn.title = wsTooltip(id, manager);
    btn.addEventListener("click", () => {
      if (dragJustEnded) return;
      void manager.activate(id);
      highlight();
    });
    btn.addEventListener("dblclick", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      const current = manager.getWorkspaceName(id) ?? "";
      void askText(t("workspace.renamePrompt"), current).then((next) => {
        if (next !== null) {
          manager.renameWorkspace(id, next);
          highlight();
        }
      });
    });
    row.appendChild(btn);
    buttons.set(id, btn);

    const noteBtn = document.createElement("button");
    noteBtn.className = "workspace-panel__note-btn";
    noteBtn.type = "button";
    noteBtn.innerHTML = noteIconSvg;
    noteBtn.title = `${t("notes.title")} — ${id}`;
    noteBtn.setAttribute("aria-label", `${t("notes.title")} — ${id}`);
    noteBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      if (dragJustEnded) return;
      toggleNotes(id, manager.getWorkspaceName(id));
    });
    row.appendChild(noteBtn);
    noteButtons.set(id, noteBtn);

    const delBtn = document.createElement("button");
    delBtn.className = "workspace-panel__del";
    delBtn.type = "button";
    delBtn.textContent = "×";
    delBtn.title = t("workspace.deleteWorkspace");
    delBtn.setAttribute("aria-label", t("workspace.deleteWorkspace"));
    if (manager.workspaces.length <= 1) delBtn.style.display = "none";
    delBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      if (manager.workspaces.length <= 1) return;
      const label = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
      const msg = t("workspace.deleteConfirm").replace("{name}", label);
      void askConfirm(msg).then((ok) => {
        if (ok) void manager.deleteWorkspace(id); // → onWorkspacesChange → rebuild()
      });
    });
    row.appendChild(delBtn);

    return row;
  }

  function statusTitle(label: string, key: string | null): string {
    return key === null ? label : `${label} — ${t(key)}`;
  }

  function makePaneRow(pane: PaneRow): HTMLElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "workspace-panel__pane";
    btn.dataset.status = pane.status;
    btn.textContent = pane.label;
    btn.title = statusTitle(pane.label, pane.status === "idle" ? null : `status.${pane.status}`);
    btn.addEventListener("click", () => {
      void manager.focusPane(pane.paneId).then(highlight);
    });
    return btn;
  }

  function makeTabRow(tab: TabRow): HTMLElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "workspace-panel__tab";
    if (tab.active) btn.classList.add("workspace-panel__tab--active");
    btn.dataset.status = tab.status;
    btn.textContent = tab.label;
    btn.title = statusTitle(tab.label, tab.status === "idle" ? null : `status.${tab.status}`);
    // Shows the tab first (it may be hidden), then focuses it.
    btn.addEventListener("click", () => {
      void manager.focusPane(tab.paneId).then(highlight);
    });
    return btn;
  }

  function makeAgentRow(paneId: Uuid, agent: AgentRow): HTMLElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = `workspace-panel__agent workspace-panel__agent--depth${agent.depth}`;
    btn.dataset.status = agent.status;
    const dot = document.createElement("span");
    dot.className = "workspace-panel__agent-dot";
    dot.textContent = agent.status === "done" ? "✓" : "●";
    const name = document.createElement("span");
    name.className = "workspace-panel__agent-name";
    name.textContent = agent.label;
    const state = document.createElement("span");
    state.className = "workspace-panel__agent-state";
    state.textContent = t(`agent.status.${agent.status}`);
    btn.append(dot, name, state);
    const base = statusTitle(agent.label, `agent.status.${agent.status}`);
    btn.title = agent.tool ? `${base} (${agent.tool})` : base;
    btn.addEventListener("click", () => {
      void manager.focusPane(paneId).then(highlight);
    });
    return btn;
  }

  /// Re-render only the pane/agent rows under each workspace. Workspace rows
  /// are left alone, so an agent update mid-drag never detaches the row
  /// being dragged.
  function renderTree(): void {
    const tree = buildAgentTree(
      manager.workspaces,
      manager.agents,
      (id) => manager.paneStatus.get(id) ?? "idle",
      treeLabels(),
      manager.paneLabels,
    );
    for (const ws of tree) {
      const host = childHosts.get(ws.wsId);
      if (!host) continue;
      const open = isExpanded(expanded, ws.wsId);
      const caret = carets.get(ws.wsId);
      if (caret) {
        caret.textContent = open ? "▾" : "▸";
        caret.title = t(open ? "tree.collapse" : "tree.expand");
        caret.setAttribute("aria-label", caret.title);
        caret.setAttribute("aria-expanded", String(open));
      }
      host.replaceChildren();
      host.style.display = open ? "" : "none";
      if (!open) continue;
      for (const pane of ws.panes) {
        host.appendChild(makePaneRow(pane));
        for (const agent of pane.agents) host.appendChild(makeAgentRow(pane.paneId, agent));
        for (const tab of pane.tabs) {
          host.appendChild(makeTabRow(tab));
          for (const agent of tab.agents) host.appendChild(makeAgentRow(tab.paneId, agent));
        }
      }
    }
  }

  function rebuild(): void {
    buttons.clear();
    noteButtons.clear();
    childHosts.clear();
    carets.clear();
    rows.length = 0;
    while (list.firstChild) list.removeChild(list.firstChild);
    // Render in `config.workspaces` order — that array *is* the user's order,
    // set by drag-to-reorder and persisted by TOML's `[[workspaces]]`.
    for (const ws of manager.workspaces) {
      const row = makeRow(ws.id);
      rows.push(row);
      list.appendChild(row);
      const children = document.createElement("div");
      children.className = "workspace-panel__children";
      childHosts.set(ws.id, children);
      list.appendChild(children);
    }
    highlight();
    renderTree();
  }

  function highlight(): void {
    for (const [id, btn] of buttons) {
      const status = manager.workspaceStatus(id);
      btn.classList.toggle("workspace-panel__ws--active", id === manager.activeIdValue);
      btn.textContent = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
      // The whole row is tinted by status (idle = no tint); CSS keys off this.
      btn.dataset.status = status;
      // "idle" has no i18n key — it's the resting state, so no status suffix.
      btn.title =
        status === "idle"
          ? wsTooltip(id, manager)
          : `${wsTooltip(id, manager)} — ${t(`status.${status}`)}`;
    }
    for (const [id, noteBtn] of noteButtons) {
      const label = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
      noteBtn.title = `${t("notes.title")} — ${label}`;
      noteBtn.setAttribute("aria-label", `${t("notes.title")} — ${label}`);
      noteBtn.classList.toggle("workspace-panel__note-btn--has-notes", hasNotes(id));
    }
  }

  manager.onWorkspacesChange(rebuild);
  manager.onPaneStatusChange = () => {
    highlight();
    renderTree();
  };
  const cleanupTree = manager.onTreeChange(renderTree);
  const cleanupNotesSub = onNotesChange(() => highlight());
  const cleanupLang = onLangChange(() => {
    header.textContent = t("workspace.panelTitle");
    addBtn.title = t("workspace.addWorkspace");
    addBtn.setAttribute("aria-label", t("workspace.addWorkspace"));
    rebuild();
  });

  host.appendChild(panel);
  rebuild();

  (panel as unknown as { __ymuxHighlight: () => void }).__ymuxHighlight = highlight;

  return () => {
    cleanupTree();
    cleanupLang();
    cleanupNotesSub();
    window.removeEventListener("pointermove", onDragMove);
    window.removeEventListener("pointerup", onDragEnd);
    window.removeEventListener("pointercancel", onDragEnd);
    panel.remove();
  };
}

/// Re-run the panel's highlight pass (active state, labels, status tint,
/// has-notes) — mirrors the old refreshWorkspaceBar so main.ts's keyboard
/// paths can force an update.
export function refreshWorkspacePanel(host: HTMLElement): void {
  const panel = host.querySelector<HTMLElement>(".workspace-panel");
  if (!panel) return;
  (panel as unknown as { __ymuxHighlight?: () => void }).__ymuxHighlight?.();
}

/// Collapse/expand the panel and persist the choice. The width change is
/// instant (CSS toggles display), so on the next animation frame we refit the
/// active workspace's terminals — the same fit + ConPTY-resize path a window
/// resize uses — preventing mis-sized / garbled TUI redraws.
export function toggleWorkspacePanel(manager: WorkspaceManager): void {
  const panel = document.querySelector<HTMLElement>(".workspace-panel");
  if (!panel) return;
  const collapsed = panel.classList.toggle("workspace-panel--collapsed");
  writeCollapsed(collapsed);
  requestAnimationFrame(() => manager.refitActive());
}
