# ymux v0.8.26

The workspace panel becomes something you can arrange, and its agent-status colour starts telling the truth about what your agents are actually doing.

## Features

### 🖐️ Drag a workspace to reorder it

The left panel listed workspaces in ascending id order, permanently. If you created them in one order and wanted to work in another, you rearranged nothing — you just remembered.

Now you drag a row where you want it. A press turns into a drag only after 4 pixels of travel, so click-to-switch and double-click-to-rename behave exactly as before; below that threshold nothing has moved and nothing is a drag. While you drag, the row goes translucent and an accent-coloured insertion line shows where it will land.

**Your `Ctrl+Alt+N` shortcuts do not shift under you.** Reordering moves rows, not ids — a workspace labelled `3` is still `Ctrl+Alt+3` no matter where it sits in the list, so the number you see is always the number you press. The order survives restarts.

The implementation is worth one note, because the obvious approach does not work here. ymux enables Tauri's native drag-drop to power drop-a-file-onto-a-terminal, and on Windows/WebView2 that switch **disables the HTML5 drag-and-drop API inside the webview entirely** — the standard `draggable` attribute would have silently done nothing. Reordering is built on pointer events instead, which the native handler doesn't touch. Persistence needed no new model field either: display order is now simply the order of the `workspaces` array, and TOML's `[[workspaces]]` preserves array order for free. No config migration, no `CONFIG_VERSION` bump.

### 🎨 Agent status colours the whole row, not a 5px dot

A workspace running an agent was marked by a five-pixel dot tucked into the bottom-right corner of its row. It was correct and nearly invisible — exactly the wrong trade for a signal whose entire job is to catch your eye from across the screen.

The status now tints the full row:

| State | Row |
|---|---|
| **running** — a command is working | blue |
| **done** — finished while you were watching | green |
| **attention** — finished while you were not | orange, pulsing |
| **idle** | untinted |

Which workspace is *active* is still unmistakable: the accent text and border sit on top of the tint rather than being replaced by it, so "which one am I in" and "which ones are busy" stay two separate readings of the same row. Hovering still brightens, tinted or not.

## Fixes

An audit of whether the indicator was ever *wrong* turned up three ways it was.

### 🔔 An agent finishing in a hidden workspace showed green, not orange

The distinction between `done` (you saw it finish) and `attention` (you didn't, go look) was decided by a pane-local `isFocused` flag — and nothing lowers that flag when you switch to another workspace or alt-tab to another application. The last pane you clicked stayed "focused" forever, from its own point of view.

So the single case the indicator exists for — **an agent finishing somewhere you aren't looking** — was classified as already-seen and painted the quiet green. The loud, pulsing orange fired mostly when you were staring right at the pane.

The correct predicate already existed: the bell notification has always suppressed itself with *window focused **and** this workspace visible **and** this pane focused*. The status classification now uses that same predicate instead of the pane's own opinion of itself.

### ⌨️ Hotkey-launched commands never showed as running

HotKey bar buttons write their command straight to the PTY via `writePane`. That path never passes through xterm's `onData`, which is where ymux detects "the user submitted something" — so a command you started from a hotkey ran to completion with its workspace sitting at idle the whole time.

### 🚀 `startup_cmd` panes never showed as running either

Same root cause: a pane's startup command is injected with a direct `writePane` once the terminal is ready, bypassing the same detection. A workspace restored with a long-running startup command looked idle until it rang the bell.

## Known limits, unchanged

The status machine is a heuristic and stays one — these are documented rather than silently patched:

- A TUI that keeps repainting after it finishes holds `running` until its bell or OSC 9 fires.
- A command with no output for more than 4 seconds (`sleep 10`, a long silent build step) decays to `idle` while still running.
- `done` green persists on an already-focused pane until you refocus it or submit the next command.

## Under the hood

- Reordering logic is two pure functions — `moveItem()` for the splice and `insertIndexFromMidpoints()` for hit-testing the drop position against row midpoints — covered by 10 unit tests, so the behaviour is pinned without a running window.
- Click-vs-drag suppression is cleared on the next `pointerdown` rather than a `setTimeout(0)`: the ordering between a timer and the trailing `click` event is not guaranteed, but the next press always is.
- The agent-status palette moved into `:root` as explicit variables (`--status-running-bg` and friends) written as literal `rgba()`, so the tints don't depend on a `color-mix()`-capable WebView2 runtime.
- The obsolete `ws-dot` rules and their keyframes are gone rather than left behind.

## Compatibility

Drop-in over v0.8.25. No config migration, no keybinding changes, no data changes. Existing `ymux.toml` files load unchanged; the first drag is what writes an order.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

Verified: `npx tsc --noEmit` clean, `npx vitest run` — 50 tests passing, `npx vite build` clean. No Rust code changed in this release.

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.25...v0.8.26
