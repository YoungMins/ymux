import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import { toggle as toggleNotes } from "../notes/NotesOverlay";
import { t } from "../i18n/i18n";
import { askText } from "../ui/Dialog";
import { toggleFileDock } from "../filedock/FileDock";
import { api } from "../ipc/bridge";

export interface CommandDef {
  id: string;
  label: () => string;
  keybinding?: string;
  action: () => void | Promise<void>;
}

export function builtinCommands(manager: WorkspaceManager): CommandDef[] {
  return [
    {
      id: "pane.splitH",
      label: () => t("shortcut.splitH"),
      keybinding: "Ctrl+Shift+H",
      action: () => void manager.splitFocused("horizontal"),
    },
    {
      id: "pane.splitV",
      label: () => t("shortcut.splitV"),
      keybinding: "Ctrl+Shift+V",
      action: () => void manager.splitFocused("vertical"),
    },
    {
      id: "pane.worktree",
      label: () => t("worktree.command"),
      action: () => void manager.openWorktreePane("horizontal"),
    },
    {
      id: "pane.close",
      label: () => t("shortcut.close"),
      keybinding: "Ctrl+Shift+W",
      action: () => void manager.closeFocused(),
    },
    {
      id: "pane.newTab",
      label: () => t("shortcut.newTab"),
      keybinding: "Ctrl+Shift+T",
      action: () => void manager.newTabInFocused(),
    },
    {
      id: "pane.prevTab",
      label: () => t("shortcut.prevTab"),
      keybinding: "Ctrl+Shift+[",
      action: () => manager.stepTabInFocused(-1),
    },
    {
      id: "pane.nextTab",
      label: () => t("shortcut.nextTab"),
      keybinding: "Ctrl+Shift+]",
      action: () => manager.stepTabInFocused(1),
    },
    {
      id: "tab.rename",
      label: () => t("tab.rename"),
      action: async () => {
        const id = manager.activePaneId();
        if (id) await manager.promptRenameTab(id);
      },
    },
    {
      id: "tab.closeOthers",
      label: () => t("tab.closeOthers"),
      action: async () => {
        const id = manager.activePaneId();
        if (id) await manager.closeOtherTabs(id);
      },
    },
    {
      id: "pane.zoom",
      label: () => t("shortcut.zoom"),
      keybinding: "Ctrl+Shift+Z",
      action: () => manager.toggleZoomFocused(),
    },
    {
      id: "pane.rename",
      label: () => t("shortcut.rename"),
      keybinding: "Ctrl+Shift+R",
      action: async () => {
        const current = manager.getFocusedTitle() ?? "";
        const next = await askText(t("app.paneTitle"), current);
        if (next !== null) manager.renameFocused(next);
      },
    },
    {
      id: "font.increase",
      label: () => t("shortcut.fontIncrease"),
      keybinding: "Ctrl++",
      action: () => manager.bumpFontSize(1),
    },
    {
      id: "font.decrease",
      label: () => t("shortcut.fontDecrease"),
      keybinding: "Ctrl+-",
      action: () => manager.bumpFontSize(-1),
    },
    {
      id: "font.reset",
      label: () => t("shortcut.fontReset"),
      keybinding: "Ctrl+0",
      action: () => manager.resetFontSize(),
    },
    {
      id: "pane.search",
      label: () => t("shortcut.search"),
      keybinding: "Ctrl+F",
      action: () => manager.toggleSearchOnFocused(),
    },
    {
      id: "pane.focusNext",
      label: () => t("shortcut.nextPane"),
      keybinding: "Ctrl+Tab",
      action: () => manager.cycleFocus(1),
    },
    {
      id: "pane.focusPrev",
      label: () => t("shortcut.prevPane"),
      keybinding: "Ctrl+Shift+Tab",
      action: () => manager.cycleFocus(-1),
    },
    {
      id: "pane.swapPrev",
      label: () => t("shortcut.swapPane"),
      keybinding: "Ctrl+Shift+←",
      action: () => manager.swapFocused(-1),
    },
    {
      id: "pane.swapNext",
      label: () => t("shortcut.swapPane"),
      keybinding: "Ctrl+Shift+→",
      action: () => manager.swapFocused(1),
    },
    {
      id: "notes.toggle",
      label: () => t("shortcut.notes"),
      keybinding: "Ctrl+Alt+N",
      action: () => {
        const wsId = manager.activeIdValue;
        toggleNotes(wsId, manager.getWorkspaceName(wsId));
      },
    },
    {
      id: "pane.splitFiles",
      label: () => t("files.splitCommand"),
      action: () => void manager.splitFocusedFiles("horizontal"),
    },
    {
      id: "pane.splitGit",
      label: () => t("git.splitCommand"),
      action: () => void manager.splitFocusedGit("horizontal"),
    },
    {
      id: "pane.splitEditor",
      label: () => t("editor.split"),
      action: () => void manager.splitFocusedEditor("horizontal"),
    },
    {
      id: "filedock.toggle",
      label: () => t("filedock.toggle"),
      keybinding: "Ctrl+Shift+E",
      action: () => toggleFileDock(),
    },
    {
      id: "workspace.rename",
      label: () => t("workspace.renamePrompt"),
      action: async () => {
        const wsId = manager.activeIdValue;
        const current = manager.getWorkspaceName(wsId) ?? "";
        const next = await askText(t("workspace.renamePrompt"), current);
        if (next !== null) manager.renameWorkspace(wsId, next);
      },
    },
    {
      id: "workspace.new",
      label: () => t("workspace.addWorkspace"),
      action: () => void manager.addWorkspace(),
    },
    {
      id: "workspace.delete",
      label: () => t("workspace.deleteWorkspace"),
      action: () => void manager.deleteWorkspace(manager.activeIdValue),
    },
    ...Array.from({ length: 9 }, (_, i) => ({
      id: `workspace.${i + 1}`,
      label: () => `${t("shortcut.switchWs")} ${i + 1}`,
      keybinding: `Ctrl+Alt+${i + 1}`,
      action: () => void manager.activate(i + 1),
    })),
    {
      // The real quit (closing the window only hides it to the tray): the
      // same unsaved-editors prompt as the tray's Quit.
      id: "app.quit",
      label: () => t("app.quit"),
      action: () => void api.quitApp().catch((e) => console.warn("quit_app failed:", e)),
    },
  ];
}

export function fuzzyMatch(query: string, text: string): boolean {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  let qi = 0;
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) qi++;
  }
  return qi === q.length;
}
