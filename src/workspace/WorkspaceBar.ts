import type { ShellProfile } from "../types";
import type { WorkspaceManager } from "./WorkspaceManager";
import { api } from "../ipc/bridge";
import { mountSettings } from "../settings/SettingsOverlay";
import {
  toggle as toggleNotes,
  hasNotes,
  onNotesChange,
} from "../notes/NotesOverlay";
import { t, onLangChange } from "../i18n/i18n";
import { promptWithBlur, confirmWithBlur } from "../browser/popupBlur";

function wsTooltip(id: number, manager: WorkspaceManager): string {
  const name = manager.getWorkspaceName(id);
  const base = name ? `${id}: ${name}` : `Workspace ${id}`;
  return `${base} (Ctrl+Alt+${id}) — ${t("workspace.dblclickRename")}`;
}

export function mountWorkspaceBar(
  host: HTMLElement,
  manager: WorkspaceManager,
  shells: ShellProfile[],
): () => void {
  const bar = document.createElement("div");
  bar.className = "workspace-bar";

  const wsGroup = document.createElement("div");
  wsGroup.className = "workspace-bar__group";
  bar.appendChild(wsGroup);

  const buttons = new Map<number, HTMLButtonElement>();
  const noteButtons = new Map<number, HTMLButtonElement>();
  const statusDots = new Map<number, HTMLElement>();

  const noteIconSvg = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><line x1="10" y1="9" x2="8" y2="9"/></svg>`;

  // "+" button that creates a new workspace at the lowest free id.
  const addBtn = document.createElement("button");
  addBtn.className = "workspace-bar__ws-add";
  addBtn.type = "button";
  addBtn.textContent = "+";
  addBtn.title = t("workspace.addWorkspace");
  addBtn.setAttribute("aria-label", t("workspace.addWorkspace"));
  addBtn.addEventListener("click", () => {
    void manager.addWorkspace(); // fires onWorkspacesChange → rebuild()
  });

  /// Build one workspace tab (switch button + note button + delete button) and
  /// register its buttons into the highlight maps.
  function makePair(id: number): HTMLElement {
    const pair = document.createElement("div");
    pair.className = "workspace-bar__ws-pair";

    const btn = document.createElement("button");
    btn.className = "workspace-bar__ws";
    btn.textContent = String(id);
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
    pair.appendChild(btn);
    buttons.set(id, btn);

    // Small dot badge showing the highest-priority pane status
    // (attention > running > done > idle) for this workspace's panes.
    const statusDot = document.createElement("span");
    statusDot.className = "ws-dot ws-dot--idle";
    btn.appendChild(statusDot);
    statusDots.set(id, statusDot);

    const noteBtn = document.createElement("button");
    noteBtn.className = "workspace-bar__note-btn";
    noteBtn.type = "button";
    noteBtn.innerHTML = noteIconSvg;
    noteBtn.title = `${t("notes.title")} — ${id}`;
    noteBtn.setAttribute("aria-label", `${t("notes.title")} — ${id}`);
    noteBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      toggleNotes(id, manager.getWorkspaceName(id));
    });
    pair.appendChild(noteBtn);
    noteButtons.set(id, noteBtn);

    // Delete button — hidden when only one workspace remains (can't delete the
    // last one). Revealed on hover via CSS.
    const delBtn = document.createElement("button");
    delBtn.className = "workspace-bar__ws-del";
    delBtn.type = "button";
    delBtn.textContent = "×";
    delBtn.title = t("workspace.deleteWorkspace");
    delBtn.setAttribute("aria-label", t("workspace.deleteWorkspace"));
    if (manager.workspaces.length <= 1) delBtn.style.display = "none";
    delBtn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      if (manager.workspaces.length <= 1) return;
      const name = manager.getWorkspaceName(id);
      const label = name && name !== `workspace-${id}` ? `${id}: ${name}` : `${id}`;
      const msg = t("workspace.deleteConfirm").replace("{name}", label);
      if (confirmWithBlur(msg)) {
        void manager.deleteWorkspace(id); // fires onWorkspacesChange → rebuild()
      }
    });
    pair.appendChild(delBtn);

    return pair;
  }

  /// Rebuild the whole tab list from the manager's current workspaces (sorted
  /// by id) plus the trailing "+" button. Called on any workspace add/delete.
  function rebuild(): void {
    buttons.clear();
    noteButtons.clear();
    statusDots.clear();
    while (wsGroup.firstChild) wsGroup.removeChild(wsGroup.firstChild);
    const sorted = [...manager.workspaces].sort((a, b) => a.id - b.id);
    for (const ws of sorted) wsGroup.appendChild(makePair(ws.id));
    wsGroup.appendChild(addBtn);
    highlight();
  }

  manager.onWorkspacesChange(rebuild);
  manager.onAttentionChange(() => highlight());
  manager.onPaneStatusChange = () => highlight();

  const cleanupNotesSub = onNotesChange(() => highlight());

  const spacer = document.createElement("div");
  spacer.className = "workspace-bar__spacer";
  bar.appendChild(spacer);

  const shellPicker = document.createElement("select");
  shellPicker.className = "workspace-bar__shell";
  shellPicker.title = t("workspace.shellTitle");
  for (const s of shells) {
    const opt = document.createElement("option");
    opt.value = s.name;
    opt.textContent = s.name;
    shellPicker.appendChild(opt);
  }
  shellPicker.addEventListener("change", () => {
    manager.setDefaultShell(shellPicker.value);
  });
  if (shells.length > 0) {
    shellPicker.value = shells[0].name;
  }
  bar.appendChild(shellPicker);

  const browserBtn = document.createElement("button");
  browserBtn.className = "workspace-bar__shell";
  browserBtn.type = "button";
  browserBtn.textContent = t("workspace.addBrowser");
  browserBtn.title = t("workspace.addBrowserTitle");
  browserBtn.style.cursor = "pointer";
  browserBtn.addEventListener("click", () => {
    void manager.splitFocusedBrowser("horizontal");
  });
  bar.appendChild(browserBtn);

  const kofiBtn = document.createElement("button");
  kofiBtn.className = "workspace-bar__icon-btn";
  kofiBtn.type = "button";
  kofiBtn.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17 8h2a2 2 0 0 1 0 4h-2"/><path d="M3 8h14v9a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4V8z"/><line x1="6" y1="2" x2="6" y2="6"/><line x1="10" y1="2" x2="10" y2="6"/><line x1="14" y1="2" x2="14" y2="6"/></svg>`;
  kofiBtn.title = "Support on Ko-fi";
  kofiBtn.addEventListener("click", () => {
    void api.openUrl("https://ko-fi.com/youngminkim").catch((e) =>
      console.warn("openUrl failed:", e),
    );
  });
  bar.appendChild(kofiBtn);

  const ghBtn = document.createElement("button");
  ghBtn.className = "workspace-bar__icon-btn";
  ghBtn.type = "button";
  ghBtn.innerHTML = `<svg width="15" height="15" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>`;
  ghBtn.title = "GitHub";
  ghBtn.addEventListener("click", () => {
    void api.openUrl("https://github.com/youngmins/ymux").catch((e) =>
      console.warn("openUrl failed:", e),
    );
  });
  bar.appendChild(ghBtn);

  const cleanupHelp = mountSettings(bar, manager);

  host.appendChild(bar);

  function highlight(): void {
    for (const [id, btn] of buttons) {
      btn.classList.toggle(
        "workspace-bar__ws--active",
        id === manager.activeIdValue,
      );
      const ws = manager.workspaces.find((w) => w.id === id);
      btn.classList.toggle("workspace-bar__ws--exists", !!ws);
      btn.classList.toggle(
        "workspace-bar__ws--attention",
        manager.workspaceHasAttention(id),
      );
      btn.title = wsTooltip(id, manager);
      const name = ws?.name;
      const isCustom = name && name !== `workspace-${id}` && name !== "main";
      // Show "1: 이름" so the user can see both the workspace number
      // (matching the Ctrl+Alt+N keybinding) and the custom name they
      // gave it. CSS handles ellipsis if the name overflows max-width.
      btn.textContent = isCustom ? `${id}: ${name}` : String(id);
      // Re-append the status dot — textContent above wipes all children.
      const dot = statusDots.get(id);
      if (dot) btn.appendChild(dot);
    }
    for (const [id, dot] of statusDots) {
      const status = manager.workspaceStatus(id);
      dot.className = `ws-dot ws-dot--${status}`;
      dot.title = status === "idle" ? "" : t(`status.${status}`);
    }
    for (const [id, noteBtn] of noteButtons) {
      const name = manager.getWorkspaceName(id);
      const isCustom = name && name !== `workspace-${id}` && name !== "main";
      const label = isCustom ? `${id}: ${name}` : String(id);
      noteBtn.title = `${t("notes.title")} — ${label}`;
      noteBtn.setAttribute("aria-label", `${t("notes.title")} — ${label}`);
      noteBtn.classList.toggle(
        "workspace-bar__note-btn--has-notes",
        hasNotes(id),
      );
    }
  }

  rebuild();

  const cleanupLang = onLangChange(() => {
    shellPicker.title = t("workspace.shellTitle");
    browserBtn.textContent = t("workspace.addBrowser");
    browserBtn.title = t("workspace.addBrowserTitle");
    kofiBtn.title = t("workspace.supportTitle");
    rebuild(); // refresh ws / add / delete button titles in the new language
  });

  (bar as unknown as { __ymuxHighlight: () => void }).__ymuxHighlight = highlight;

  return () => {
    cleanupLang();
    cleanupHelp();
    cleanupNotesSub();
    bar.remove();
  };
}

export function refreshWorkspaceBar(host: HTMLElement): void {
  const bar = host.querySelector<HTMLElement>(".workspace-bar");
  if (!bar) return;
  const updater = (bar as unknown as { __ymuxHighlight?: () => void })
    .__ymuxHighlight;
  updater?.();
}
