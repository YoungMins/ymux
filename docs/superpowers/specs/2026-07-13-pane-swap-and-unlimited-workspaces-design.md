# Pane Swap + Unlimited Workspaces — Design

Date: 2026-07-13

## Goal

Two user-requested features for ymux:

1. **Swap split-pane positions** via keyboard.
2. **Unlimited workspaces** (top bar) via a `+` button, replacing the hard cap of 9.

## Feature 1 — Swap pane positions (keyboard)

### Behaviour
- `Ctrl+Shift+ArrowLeft` / `Ctrl+Shift+ArrowRight` swap the focused pane with the
  previous / next pane in depth-first (DFS) order. Wraps at both ends.
- tmux `swap-pane -U/-D` semantics.
- Only the two leaf pane **nodes** trade positions in the layout tree. Pane `id`,
  cache entry, DOM element, and PTY are untouched, so terminal contents /
  scrollback survive and focus stays on the same pane.
- Single-pane workspace: no-op.

### Implementation
- `src/layout/LayoutTree.ts` → `swapPanes(root, idA, idB): LayoutNode` — rebuild the
  tree mapping the node at `idA`'s slot to `idB`'s node and vice-versa.
- `src/workspace/WorkspaceManager.ts` → `swapFocused(delta: 1 | -1)` — find the
  prev/next pane id in `panes(ws.root)` order (wrapping), call `swapPanes`,
  `renderWorkspace`, persist. Focus is unchanged (same id).
- `src/main.ts` → keydown handlers for `Ctrl+Shift+ArrowLeft/ArrowRight`.
- `src/terminal/TerminalPane.ts` → block those chords in
  `attachCustomKeyEventHandler` so they bubble to the window handler.
- Propagate per CLAUDE.md rule #6: Help overlay, Command Palette, i18n, README×3.

Rust side needs no change: swaps happen on the frontend and the whole tree is
persisted via `save_config`.

## Feature 2 — Unlimited workspaces (`+` button / delete)

### Behaviour
- Top bar renders **only workspaces that exist** (sorted by id) plus a trailing
  `+` button. No count cap.
- `+` → create a new workspace using the **lowest free positive id** (e.g. with
  {1,3} present, next is 2), seed one default-shell terminal pane, switch to it.
- Each tab shows a hover-revealed `×` delete button → confirm → dispose panes
  (kills their PTYs) and remove the workspace. The **last remaining** workspace
  cannot be deleted. Deleting the active workspace switches to a neighbour.
- `Ctrl+Alt+1..9` keybindings unchanged (ids >9 reachable by click only).

### Implementation
- `src/workspace/WorkspaceManager.ts`
  - `activate()`: drop the `id > MAX_WORKSPACES` cap (keep `id < 1` guard).
  - `addWorkspace(): number` — lowest-free id, create container/cache/config,
    activate, fire change callback.
  - `deleteWorkspace(id)` — dispose cached panes, remove container/cache/config/
    hydrated entry, re-activate a neighbour if it was active, fire change
    callback. Refuses to delete the last workspace.
  - `onWorkspacesChange(cb)` — registered by the bar to trigger a rebuild.
- `src/workspace/WorkspaceBar.ts` — replace the static `1..9` loop with a
  `rebuild()` that renders existing workspaces + `+` button + per-tab `×`.
- i18n: `workspace.addWorkspace`, `workspace.deleteWorkspace`,
  `workspace.deleteConfirm`.

### Notes storage edge
A reused id inherits the previous workspace's notes (notes are keyed by id).
Rare; left as-is for now.

## Out of scope (YAGNI)
- Directional / spatial pane swap; drag-and-drop reorder.
- Rust-side swap logic.

## Verification
- `npx tsc --noEmit`, `cargo clippy --workspace -- -D warnings`.
- `pnpm tauri dev`: swap left/right; add several workspaces past 9; delete
  active and inactive; confirm last-workspace delete is blocked.

---

# Feature 3 — AI CLI completion detection & notification

Detect when a long-running CLI (claude / codex / gemini) in a terminal pane
finishes, and alert the user via a tab/pane badge, an OS desktop notification,
and a sound.

## Detection
- `TerminalPane` hooks `term.onBell()` (BEL 0x07) and an OSC 9 handler
  (`ESC ] 9 ; <text> ST`). Claude Code rings the terminal bell on completion /
  when awaiting input; OSC 9 carries an optional message.
- New option `onAttention?: (message: string | null) => void` fired with the
  OSC 9 text (or `null` for a plain bell).
- **Dependency, stated honestly:** only fires if the CLI actually emits a
  bell / OSC 9. Claude Code supports it; codex/gemini depend on their own
  notification settings. An output-idle fallback can be added later.

## Routing (WorkspaceManager)
- `handleAttention(paneId, message)`:
  - **Suppress** if the pane is the focused pane in the active workspace and the
    app window is focused (the user is already watching it).
  - Gate on `config.notify_on_bell`.
  - Badge: add `.pane--attention` to the pane element; if the pane's workspace
    isn't active, mark that workspace (attention set) → `.workspace-bar__ws--attention`.
  - OS notification via `api.notify(title, body)`.
  - Sound via `beep()`.
- **Clear** attention on the pane when it regains focus, and on the workspace
  when it is activated.
- Track `windowFocused` via window focus/blur; expose `workspaceHasAttention(id)`
  and `onAttentionChange(cb)` (bar re-highlights), and `setNotifyOnBell(bool)`.

## Config / backend
- `Config.notify_on_bell: bool`, `#[serde(default = default_true)]` (additive →
  no CONFIG_VERSION bump). TS mirror in `types.ts`.
- `tauri-plugin-notification` added under the `desktop` feature; Rust `notify`
  command exposed over IPC; `notification:default` added to capabilities. The
  frontend calls it through `api.notify` (no new JS dependency).
- Sound: Web Audio oscillator beep in `src/util/beep.ts` (no asset).

## Settings
- A toggle in the Settings overlay bound to `notify_on_bell` (default on) via
  `manager.setNotifyOnBell`, plus i18n. Per-channel toggles are a later follow-up.

## Files
`TerminalPane.ts`, `WorkspaceManager.ts`, `WorkspaceBar.ts`, `style.css`,
`config/model.rs`, `types.ts`, `commands.rs`, `main.rs`, `Cargo.toml`,
`capabilities/default.json`, `ipc/bridge.ts`, `util/beep.ts`, Settings overlay,
`i18n.ts`.

## Verification (feature 3)
- `npx tsc --noEmit`, `cargo check --no-default-features --lib --tests -p ymux`
  (Linux-safe), desktop `cargo check`, `cargo clippy`.
- Manual: run `claude` in a background pane, let it finish → badge on its tab,
  OS notification, beep; no alert when watching the focused pane.
