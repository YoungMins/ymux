import type { WorkspaceManager } from "./WorkspaceManager";
import { formatWorkspaceLabel, sortWorkspacesById } from "./workspaceLabel";
import {
  toggle as toggleNotes,
  hasNotes,
  onNotesChange,
} from "../notes/NotesOverlay";
import { t, onLangChange } from "../i18n/i18n";
import { promptWithBlur, confirmWithBlur } from "../browser/popupBlur";

const COLLAPSE_KEY = "ymux:workspace-panel:collapsed";

const noteIconSvg = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><line x1="10" y1="9" x2="8" y2="9"/></svg>`;

function wsTooltip(id: number, manager: WorkspaceManager): string {
  const name = manager.getWorkspaceName(id);
  const base = name ? `${id}: ${name}` : `Workspace ${id}`;
  return `${base} (Ctrl+Alt+${id}) — ${t("workspace.dblclickRename")}`;
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
  const statusDots = new Map<number, HTMLElement>();

  /// Build one vertical row: switch button (label + status dot) with a note
  /// button and a delete button trailing it.
  function makeRow(id: number): HTMLElement {
    const row = document.createElement("div");
    row.className = "workspace-panel__row";

    const btn = document.createElement("button");
    btn.className = "workspace-panel__ws";
    btn.textContent = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
    btn.title = wsTooltip(id, manager);
    btn.addEventListener("click", () => {
      void manager.activate(id);
      highlight();
    });
    btn.addEventListener("dblclick", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      const current = manager.getWorkspaceName(id) ?? "";
      const next = promptWithBlur(t("workspace.renamePrompt"), current);
      if (next !== null) {
        manager.renameWorkspace(id, next);
        highlight();
      }
    });

    const statusDot = document.createElement("span");
    statusDot.className = "ws-dot ws-dot--idle";
    btn.appendChild(statusDot);
    statusDots.set(id, statusDot);
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
      if (confirmWithBlur(msg)) {
        void manager.deleteWorkspace(id); // fires onWorkspacesChange → rebuild()
      }
    });
    row.appendChild(delBtn);

    return row;
  }

  function rebuild(): void {
    buttons.clear();
    noteButtons.clear();
    statusDots.clear();
    while (list.firstChild) list.removeChild(list.firstChild);
    for (const ws of sortWorkspacesById(manager.workspaces)) {
      list.appendChild(makeRow(ws.id));
    }
    highlight();
  }

  function highlight(): void {
    for (const [id, btn] of buttons) {
      btn.classList.toggle("workspace-panel__ws--active", id === manager.activeIdValue);
      btn.title = wsTooltip(id, manager);
      btn.textContent = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
      const dot = statusDots.get(id);
      if (dot) btn.appendChild(dot); // textContent above wiped children
    }
    for (const [id, dot] of statusDots) {
      const status = manager.workspaceStatus(id);
      dot.className = `ws-dot ws-dot--${status}`;
      dot.title = status === "idle" ? "" : t(`status.${status}`);
    }
    for (const [id, noteBtn] of noteButtons) {
      const label = formatWorkspaceLabel(id, manager.getWorkspaceName(id));
      noteBtn.title = `${t("notes.title")} — ${label}`;
      noteBtn.setAttribute("aria-label", `${t("notes.title")} — ${label}`);
      noteBtn.classList.toggle("workspace-panel__note-btn--has-notes", hasNotes(id));
    }
  }

  manager.onWorkspacesChange(rebuild);
  manager.onPaneStatusChange = () => highlight();
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
    cleanupLang();
    cleanupNotesSub();
    panel.remove();
  };
}

/// Re-run the panel's highlight pass (active state, labels, status dots,
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
