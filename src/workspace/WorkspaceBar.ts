// Slim top bar that shows numbered workspaces and a shell-picker dropdown for
// the default shell used when new panes are created.

import type { ShellProfile } from "../types";
import type { WorkspaceManager } from "./WorkspaceManager";
import { mountHelpButton } from "../help/HelpOverlay";

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
    btn.title = `Workspace ${i} (Ctrl+Alt+${i})`;
    btn.addEventListener("click", () => {
      void manager.activate(i);
      highlight();
    });
    wsGroup.appendChild(btn);
    buttons.set(i, btn);
  }

  const spacer = document.createElement("div");
  spacer.className = "workspace-bar__spacer";
  bar.appendChild(spacer);

  const shellPicker = document.createElement("select");
  shellPicker.className = "workspace-bar__shell";
  shellPicker.title = "Default shell for new panes";
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

  // "?" help button — always at the far right of the bar.
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
    }
  }

  highlight();

  // Expose an updater so main.ts can re-highlight after keyboard shortcuts.
  (bar as unknown as { __ymuxHighlight: () => void }).__ymuxHighlight = highlight;

  return () => {
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
