// Canonical shortcut reference shown in Settings → Shortcuts
// (`SettingsOverlay.ts`'s `renderShortcuts`). Pulled into its own module —
// rather than a local const in SettingsOverlay.ts — so `shortcutList.test.ts`
// can import it without dragging in the overlay's DOM/Tauri-heavy setup.
//
// Every row is written in the canonical Windows `Ctrl+…` form (CLAUDE.md
// rule 10); `shortcutLabel()` in `../platform` maps it to `Cmd+…` for macOS
// at render time. Keep this list in sync with `src/main.ts`'s global keydown
// handler and any pane-local keymap it documents — `shortcutList.test.ts`
// checks the main.ts side automatically for key/code literals it recognizes.

export interface ShortcutEntry {
  keys: string;
  tKey: string;
}

export const SHORTCUTS: ShortcutEntry[] = [
  // ── Workspaces / panes (src/main.ts global keydown handler) ──────────
  { keys: "Ctrl+Alt+1 … 9", tKey: "shortcut.switchWs" },
  { keys: "Ctrl+Shift+H", tKey: "shortcut.splitH" },
  { keys: "Ctrl+Shift+V", tKey: "shortcut.splitV" },
  { keys: "Ctrl+Shift+W", tKey: "shortcut.close" },
  { keys: "Ctrl+Shift+T", tKey: "shortcut.newTab" },
  { keys: "Ctrl+Shift+[", tKey: "shortcut.prevTab" },
  { keys: "Ctrl+Shift+]", tKey: "shortcut.nextTab" },
  { keys: "Ctrl+Tab", tKey: "shortcut.nextPane" },
  { keys: "Ctrl+Shift+Tab", tKey: "shortcut.prevPane" },
  { keys: "Ctrl+Shift+←/→", tKey: "shortcut.swapPane" },
  { keys: "Ctrl+Shift+Z", tKey: "shortcut.zoom" },
  { keys: "Ctrl+Shift+R", tKey: "shortcut.rename" },
  { keys: "Ctrl+Alt+N", tKey: "shortcut.notes" },
  { keys: "Ctrl+Shift+E", tKey: "filedock.toggle" },
  { keys: "Ctrl+Shift+P", tKey: "shortcut.palette" },
  { keys: "Ctrl++", tKey: "shortcut.fontIncrease" },
  { keys: "Ctrl+-", tKey: "shortcut.fontDecrease" },
  { keys: "Ctrl+0", tKey: "shortcut.fontReset" },

  // ── Terminal pane ──────────────────────────────────────────────────
  { keys: "Ctrl+F", tKey: "shortcut.search" },
  { keys: "Ctrl+Click", tKey: "shortcut.openLink" },
  { keys: "Ctrl+V", tKey: "shortcut.paste" },

  // ── Editor pane (src/editor/editorKeymap.ts, Mod-s / Mod-g) ───────────
  { keys: "Ctrl+S", tKey: "shortcut.save" },
  { keys: "Ctrl+G", tKey: "shortcut.gotoLine" },

  // ── Workspace panel ────────────────────────────────────────────────
  { keys: "Dbl-click WS", tKey: "shortcut.renameWs" },
];

/// Split a canonical `Ctrl+…` (or already mac-mapped `Cmd+…`) shortcut string
/// into its individual key names for per-key `<kbd>` rendering.
///
/// A plain `.split("+")` breaks the one shortcut whose *key itself* is `+`
/// (`Ctrl++`, font-size increase): it produces a trailing empty segment
/// instead of a literal `+` key. This treats a trailing `++` as "the last
/// key is Plus" before splitting, and otherwise behaves like a normal
/// `+`-delimited split.
export function splitShortcutKeys(spec: string): string[] {
  const endsWithPlusKey = spec.endsWith("++");
  const base = endsWithPlusKey ? spec.slice(0, -1) : spec;
  const parts = base
    .split("+")
    .map((seg) => seg.trim())
    .filter((seg) => seg.length > 0);
  if (endsWithPlusKey) parts.push("+");
  return parts;
}
