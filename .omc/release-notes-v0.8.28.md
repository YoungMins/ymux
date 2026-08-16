# ymux v0.8.28

Right-click finally does something useful in a terminal, new panes open in the shell you actually picked, and a settings bug that had been quietly reverting your choices is gone.

## Features

### 🖱️ A right-click menu built for a terminal

Right-clicking a pane used to raise the webview's own menu — Reload, Back, Inspect Element. Three things a terminal multiplexer has no use for, in place of the ones it does.

That menu is now suppressed across the whole app, and terminal panes put ymux's own in its place:

```
Copy            (greyed out with nothing selected)
Paste
─────────────────
Split pane horizontally
Split pane vertically
─────────────────
yDir   yMon   yCode   yGit
```

**Copy and Paste are there on purpose.** Removing the native menu would otherwise have taken the only mouse-driven route to the clipboard with it. Paste runs the same path as `Ctrl+V`, images included. Text inputs — the search bar, the notes pane — keep the native menu, because cut/copy/paste on a text field is exactly what you're reaching for there.

The menu is a DOM element rather than Tauri's native menu API, so it inherits the app's own theme instead of needing a separate palette per platform.

### 🧰 The companion tools, one click away

yDir, yMon, yCode and yGit launch straight from that menu. They run **in the pane you right-clicked**, the same way a HotKey bar button submits a command — they're ordinary CLIs on PATH, so that's the shortest distance between wanting yDir and having it, and `Ctrl+C` backs out.

The pane's workspace turns blue while the tool runs, like any other command. That needed care: a tool launched this way writes straight to the PTY without passing through the terminal's own input handling, which is precisely the gap that used to leave hotkey-launched commands showing as idle.

### 🐚 Default shell, in Settings and remembered

**Settings → General → Default shell** picks the shell new panes and workspaces open with. It's the same value as the toolbar picker — change either one and the other follows — but unlike the toolbar picker, which only ever lived in memory, this one survives a restart.

### 🎨 Status colours you can actually see

The workspace row tints introduced in v0.8.26 were mixed at 24% and read as a faint wash against the panel. Running and done are now at 46%, and the attention pulse swings 30% → 80%.

## Fixes

### ⚙️ Settings stopped silently reverting on restart

Getting the default shell to persist turned up why it wouldn't: **every plain setting the app's UI owns was being thrown away on save.**

Saving doesn't replace the stored config wholesale — it merges the incoming one field by field, and that merge only ever copied the schema version, the active workspace, the workspace layouts, and the detected shell list. Everything else fell on the floor. So **Notify on bell** and **Persist scrollback** worked for the rest of the session and then quietly came back on at the next launch, along with the git worktree base directory.

It looked complete from the UI's side — the toggle flipped, the value was written, the save fired — because the loss happened entirely on the other side of that call. The settings fields are now carried across, with a test pinning it so the next field added can't repeat this. The shell list stays deliberately excluded: it's a detection cache the backend owns, and a stale snapshot from the UI must not be able to clobber it.

If you had turned bell notifications *off*, this is the release where that finally sticks.

## Under the hood

- `Config.default_shell` is a plain `String` with a serde default (empty = first detected shell), following the same convention as the other optional-ish fields in the model. Additive, so no `CONFIG_VERSION` bump and existing `ymux.toml` files load untouched.
- Applying the default is just ordering the shell list so it comes first — every path that creates a pane already reads the head of that list, so nothing else needed changing.
- The context menu mounts on `document.body`, not inside the pane. The workspace host runs a capture-phase pointerdown handler that force-focuses whatever pane is under the cursor, and a menu living inside a pane would trip it on every click of its own items.

## Compatibility

Drop-in over v0.8.27. No config migration and no keybinding changes. One consequence worth knowing: hand-editing `ymux.toml` while ymux is running now gets overwritten on the next save for settings fields too, as has always been the case for layouts. Edit it with the app closed.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

Verified: `npx tsc --noEmit` clean, `npx vitest run` — 57 tests passing, `npx vite build` clean, `cargo fmt --check` clean, `cargo test -p ymux` — 3 new config tests among them, `cargo check -p ymux` (desktop) clean.

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.27...v0.8.28
