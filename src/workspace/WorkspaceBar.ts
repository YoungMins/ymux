import type { ShellProfile } from "../types";
import type { WorkspaceManager } from "./WorkspaceManager";
import { api } from "../ipc/bridge";
import { mountHelpButton } from "../help/HelpOverlay";
import { t, onLangChange } from "../i18n/i18n";

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
  for (let i = 1; i <= 9; i++) {
    const btn = document.createElement("button");
    btn.className = "workspace-bar__ws";
    btn.textContent = String(i);
    btn.title = wsTooltip(i, manager);
    btn.addEventListener("click", () => {
      void manager.activate(i);
      highlight();
    });
    btn.addEventListener("dblclick", (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      const current = manager.getWorkspaceName(i) ?? "";
      const next = window.prompt(t("workspace.renamePrompt"), current);
      if (next !== null) {
        manager.renameWorkspace(i, next);
        highlight();
      }
    });
    wsGroup.appendChild(btn);
    buttons.set(i, btn);
  }

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
  kofiBtn.className = "workspace-bar__kofi";
  kofiBtn.type = "button";
  kofiBtn.textContent = "☕ Support";
  kofiBtn.title = t("workspace.supportTitle");
  kofiBtn.addEventListener("click", () => {
    void api.openUrl("https://ko-fi.com/youngminkim").catch((e) =>
      console.warn("openUrl failed:", e),
    );
  });
  bar.appendChild(kofiBtn);

  const cleanupHelp = mountHelpButton(bar);

  host.appendChild(bar);

  function highlight(): void {
    for (const [id, btn] of buttons) {
      btn.classList.toggle(
        "workspace-bar__ws--active",
        id === manager.activeIdValue,
      );
      const ws = manager.workspaces.find((w) => w.id === id);
      btn.classList.toggle("workspace-bar__ws--exists", !!ws);
      btn.title = wsTooltip(id, manager);
      const name = ws?.name;
      const isCustom = name && name !== `workspace-${id}` && name !== "main";
      // Show "1: 이름" so the user can see both the workspace number
      // (matching the Ctrl+Alt+N keybinding) and the custom name they
      // gave it. CSS handles ellipsis if the name overflows max-width.
      btn.textContent = isCustom ? `${id}: ${name}` : String(id);
    }
  }

  highlight();

  const cleanupLang = onLangChange(() => {
    shellPicker.title = t("workspace.shellTitle");
    browserBtn.textContent = t("workspace.addBrowser");
    browserBtn.title = t("workspace.addBrowserTitle");
    kofiBtn.title = t("workspace.supportTitle");
  });

  (bar as unknown as { __ymuxHighlight: () => void }).__ymuxHighlight = highlight;

  return () => {
    cleanupLang();
    cleanupHelp();
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
