# Workspace Left Panel — Design

**Date:** 2026-07-19
**Status:** Approved, ready for implementation plan

## Goal

Move the workspace list out of the top bar into a dedicated **full-height left
panel**, make the list **scroll** when there are many workspaces, and let the
user **collapse/expand** the panel. The remaining top-bar controls (shell
picker, browser button, Ko-fi, GitHub, Settings) stay in the top bar.

## Motivation

The top bar lays workspace tabs out horizontally, so a long list crowds the bar
and competes for width with the right-side controls. A vertical left panel gives
the list its own scrollable column and room to show workspace names.

## Layout

`#app` changes from a vertical column to a top-level horizontal row:

```
#app (flex row, 100vh)
├── .workspace-panel        ← NEW: full window height, fixed width ~160px
│   ├── .workspace-panel__header   (title, sticky top)
│   ├── .workspace-panel__list     (scrollable: overflow-y auto)
│   └── .workspace-panel__add       ("+", sticky bottom)
└── .app-main (flex column)  ← the existing vertical stack
    ├── .workspace-bar        (shell picker, browser, Ko-fi, GitHub, Settings, + panel toggle)
    ├── .workspace-host       (panes)
    └── (status bar, mounted into .app-main)
```

The panel spans the full window height; the top bar and status bar sit only to
its right, inside `.app-main`. When the panel is collapsed it is fully hidden
(`display: none`) and `.app-main` takes the full width.

## Components

### New: `src/workspace/WorkspacePanel.ts`

Owns the vertical workspace list. Absorbs from `WorkspaceBar.ts`:
- the per-workspace controls (switch button, note button, delete button, status
  dot), rebuilt as a **vertical row** (`makeRow` replacing `makePair`),
- `rebuild()` (list rebuild on add/delete, sorted by id),
- `highlight()` (active state, names, status dots, has-notes state),
- the manager wiring: `manager.onWorkspacesChange(rebuild)`,
  `manager.onPaneStatusChange = () => highlight()`, and the `onNotesChange`
  subscription,
- the `+` add button (now sticky at the panel bottom).

Signature: `mountWorkspacePanel(host, manager): () => void` (returns cleanup).
Exports a `refreshWorkspacePanel(host)` mirroring today's
`refreshWorkspaceBar(host)` — locates `.workspace-panel` and calls its stored
`__ymuxHighlight`.

### Slimmed: `src/workspace/WorkspaceBar.ts`

Keeps only the right-side controls (shell picker, browser button, Ko-fi,
GitHub, Settings) plus a **new panel-toggle button** at the bar's far left.
Loses `wsGroup`, `makePair`, `rebuild`, and the workspace-list half of
`highlight`. `refreshWorkspaceBar` is retained but delegates to
`refreshWorkspacePanel` so `main.ts:101`/`:112` need no logic change (only the
import, if we re-export).

### `src/main.ts`

- Build the DOM as `#app > [.workspace-panel, .app-main]`; mount the top bar,
  host, and status bar inside `.app-main` (today they mount into `#app`).
- `mountWorkspacePanel(panelEl, manager)` alongside `mountWorkspaceBar`.
- Keep `refreshWorkspaceBar(app)` calls working (they resolve to the panel
  refresh); pass the right host element.

## Row layout (per workspace)

All controls always visible (no hover reveal), full-width row:

```
● 1: main        ✎  ×
```

- status dot (`ws-dot ws-dot--<status>`, existing classes),
- `id: name` label (or just the number when no custom name; ellipsis on
  overflow),
- note button (`✎`), toggles the workspace's notes,
- delete button (`×`), hidden when only one workspace remains.

Clicking the row switches to that workspace; double-click renames (unchanged
behavior, moved from the horizontal tab).

## Collapse behavior

- A single toggle button lives at the **far left of the top bar** (always
  visible, the one source of truth — the panel header shows only a title, no
  second toggle).
- Toggling sets/clears a collapsed state: panel `display: none` when collapsed.
- The collapsed state persists in `localStorage` (mirroring the notes-overlay
  persistence pattern), restored on startup.

## Resize handling (critical)

Terminals here refit **only** on the `window` `resize` event
(`main.ts` → `manager.refitActive()` → `pane.scheduleFit()` →
`fit.fit()` → `api.resizePane()`); terminal panes have **no** per-pane
`ResizeObserver`. Collapsing the panel changes the terminal container width
**without** a window resize, so terminals would be left mis-sized and TUI apps
(e.g. Claude Code) could garble on their next redraw.

**Fix:** after the toggle changes the panel width, call `manager.refitActive()`
inside a `requestAnimationFrame` (so layout has settled), running the exact same
fit + ConPTY-resize path a window resize uses — a proven-clean path. The panel
width change is applied **instantly** (no CSS width transition) to avoid racing
the rAF refit. Only the active workspace's panes need refitting; hidden
workspaces refit on activation via the existing path.

## i18n

Two new keys across all 13 languages:
- `workspace.panelTitle` — the panel header label (e.g. "Workspaces").
- `workspace.togglePanel` — the toggle button's title/aria-label.

## Testing

- **vitest:** pure list logic where extractable (sorted rebuild order, row
  label formatting `id` vs `id: name`). The DOM-mount and toggle paths are
  covered by `tsc` + manual GUI smoke.
- **Manual GUI smoke** (`pnpm tauri dev`, human): (1) list renders vertically
  and scrolls past ~15 workspaces; (2) collapse/expand hides/shows the panel and
  persists across restart; (3) **toggle does not garble** a running TUI app
  (open Claude Code in a pane, collapse + expand, confirm clean redraw); (4)
  add/delete/rename/notes/status-dot still work from the panel.

## Out of scope (YAGNI)

- Drag-to-resize panel width.
- Reordering workspaces by drag.
- A collapsed "rail" mode (icons-only) — collapse is a full hide.
- Moving the shell picker / browser / Settings controls out of the top bar.

## Files

- Create: `src/workspace/WorkspacePanel.ts`
- Modify: `src/workspace/WorkspaceBar.ts` (slim to right-side controls + toggle)
- Modify: `src/main.ts` (layout restructure, mount panel)
- Modify: `src/style.css` (`.workspace-panel*` styles, `.app-main`, collapsed state)
- Modify: `src/i18n/i18n.ts` (two new keys × 13 languages)
- Possibly add: `src/workspace/WorkspacePanel.test.ts` (row-label / sort logic)
