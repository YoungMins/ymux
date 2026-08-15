// A single shared right-click menu, rendered as DOM rather than through
// Tauri's native menu API so it inherits the app's theme variables and needs
// no per-platform styling.
//
// The element is appended to `document.body`, never inside a `.pane`:
// WorkspaceManager installs a capture-phase `pointerdown` handler on the
// workspace host that force-focuses the terminal under the cursor, and a menu
// living inside a pane would trip it on every click of its own items.

export interface ContextMenuItem {
  label: string;
  onSelect: () => void;
  /// Rendered greyed out and unclickable — used for "Copy" with no selection.
  disabled?: boolean;
}

export type ContextMenuEntry = ContextMenuItem | "separator";

/// Gap kept between the menu and the viewport edge when it has to be nudged
/// back on screen.
const EDGE_MARGIN_PX = 4;

let current: HTMLElement | null = null;
let teardown: (() => void) | null = null;

export function closeContextMenu(): void {
  teardown?.();
  teardown = null;
  current?.remove();
  current = null;
}

export function isContextMenuOpen(): boolean {
  return current !== null;
}

/// Open the menu at viewport coordinates (`clientX`/`clientY` of the event).
/// Any menu already open is replaced.
export function showContextMenu(
  x: number,
  y: number,
  entries: ContextMenuEntry[],
): void {
  closeContextMenu();
  if (entries.length === 0) return;

  const menu = document.createElement("div");
  menu.className = "context-menu";
  menu.setAttribute("role", "menu");

  for (const entry of entries) {
    if (entry === "separator") {
      const sep = document.createElement("div");
      sep.className = "context-menu__separator";
      menu.appendChild(sep);
      continue;
    }
    const item = document.createElement("button");
    item.type = "button";
    item.className = "context-menu__item";
    item.setAttribute("role", "menuitem");
    item.textContent = entry.label;
    if (entry.disabled) {
      item.disabled = true;
    } else {
      item.addEventListener("click", () => {
        closeContextMenu();
        entry.onSelect();
      });
    }
    menu.appendChild(item);
  }

  // Mount hidden first so the size is measurable before it's positioned —
  // otherwise a menu opened near the right or bottom edge visibly jumps.
  menu.style.visibility = "hidden";
  document.body.appendChild(menu);
  const { width, height } = menu.getBoundingClientRect();
  const left = Math.max(
    EDGE_MARGIN_PX,
    Math.min(x, window.innerWidth - width - EDGE_MARGIN_PX),
  );
  const top = Math.max(
    EDGE_MARGIN_PX,
    Math.min(y, window.innerHeight - height - EDGE_MARGIN_PX),
  );
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
  menu.style.visibility = "";
  current = menu;

  // Capture phase so a click outside dismisses the menu before the thing
  // under the cursor reacts to it.
  const onPointerDown = (ev: PointerEvent): void => {
    if (!(ev.target instanceof Node) || !menu.contains(ev.target)) {
      closeContextMenu();
    }
  };
  const onKeyDown = (ev: KeyboardEvent): void => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      closeContextMenu();
    }
  };
  const onDismiss = (): void => closeContextMenu();

  window.addEventListener("pointerdown", onPointerDown, true);
  window.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("blur", onDismiss);
  window.addEventListener("resize", onDismiss);
  teardown = () => {
    window.removeEventListener("pointerdown", onPointerDown, true);
    window.removeEventListener("keydown", onKeyDown, true);
    window.removeEventListener("blur", onDismiss);
    window.removeEventListener("resize", onDismiss);
  };
}
