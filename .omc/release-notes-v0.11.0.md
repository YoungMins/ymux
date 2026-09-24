# ymux v0.11.0

The biggest release this project has shipped: the `ydir` / `ycode` / `ygit` / `ymon` CLI tools are gone, replaced by GUI panes rendered inside ymux itself — plus agent session resume, clickable file paths, and a hardened IPC surface.

## Breaking: the TUI tools are gone

`ydir`, `ycode`, `ygit` and `ymon` no longer ship as standalone binaries, and the `y` launcher's tool subcommands (`y mon`, `y code file.rs`, …) go with them. Typing any of them anywhere now fails with "command not found".

What replaces them, as panes ymux itself renders alongside terminals and browsers:

- **Files pane** — browse, preview, rename, create, multi-select, copy/move, delete to trash. Opens from a terminal's right-click menu ("Files here") or the command palette; the right-side file dock now hosts one instead of running `ydir --dock` in a PTY.
- **Editor pane** — CodeMirror 6, syntax highlighting, find/replace, go to line, multi-cursor, CRLF/BOM-preserving saves, an unsaved-changes guard on every close path (including macOS Cmd+Q), crash-safety drafts, and external-change detection.
- **Git pane** — commit graph, branch list with checkout, worktree add/remove with confirmation.

`ymon` has no in-app replacement: the status bar already streams the same CPU / RAM / GPU / disk / network numbers from the same backend, and the process list is better served by the OS task manager. The editor's Markdown preview and file-tree sidebar are also gone for now — deferred, not replaced.

One binary survives: `y`, shrunk to a single job — relaying Claude Code hook events into the agent tree. It keeps its name and its path, so existing agent-tracking hooks in `~/.claude/settings.json` keep working across the upgrade with no action from you.

**If you have a hand-written `startup_cmd` or HotKey button that runs `ycode foo.rs` or similar, it will break.** These are arbitrary strings you wrote and are not auto-migrated — point them at the Editor pane instead (or open the file from the Files pane / dock).

**Downgrading:** if you install an older ymux over this one, its config parser doesn't recognize the new pane kinds and falls back to a temporary config rather than overwriting your real one — that session shows default layouts, and upgrading again restores everything.

## Features

### Agent session resume

Restart ymux while a pane is running Claude Code or Codex, and that pane picks the conversation back up — `claude --resume <id> --dangerously-skip-permissions` / `codex resume <id>` — instead of showing a static picture of the old scrollback. This works even if you've never turned on agent tracking: ymux finds the session id by reading the CLI's own transcript files from disk, matched to the pane's working directory. Shell panes are untouched and keep restoring scrollback exactly as before.

### Clickable file paths

`Ctrl+Click` (`Cmd+Click` on macOS) already opened URLs; it now also opens filesystem paths a command printed — `cat`, `git log`, an agent's answer, whatever's sitting in the pane. Executables and scripts are revealed in the file manager rather than launched.

### Workspace panel polish

Double-click a workspace's name to rename it in place. The active workspace's whole block in the panel is now outlined, so it's obvious which one you're in at a glance.

### Hardened against embedded web content

Every Tauri command ymux exposes now checks the caller's origin before doing anything, so a page loaded in a browser pane can't reach ymux's own IPC surface. ymux refuses to load at all if something else frames it, and the embedded browser forwards only a small fixed allowlist of harmless keyboard shortcuts — nothing that spawns a process or closes a pane.

## Under the hood

- New backend modules: `fsx`/`fsops` (the Files pane's listing/sort/binary-sniff logic and its Tauri commands), `textfile` (EOL/BOM/encoding-safe reads and writes), `fspath` (resolving and opening a path lifted out of terminal text, with the reveal-vs-run policy), `ipc_guard` (the per-command origin guard), `agent_sessions`/`agent_scan_disk` (the resume feature's record store and disk-scan fallback), `drafts` and `scrollback` (crash-safety persistence, Tauri-free and unit-tested on Linux CI).
- `crates/yipc` and `tools/ylauncher` shrank to exactly what the hook relay needs; `crates/yversion` is gone — its only consumers were the retired TUI footers.
- `ipc_guard::tests::every_registered_command_starts_with_a_guard` parses `main.rs`'s command registration and every command body, so a new `#[tauri::command]` without a guard fails CI rather than shipping.

## Compatibility

`CONFIG_VERSION` stays at 8 — no forced re-detection, no config migration. The new pane kinds (`files`, `editor`, `git`) are additive and load transparently in a config written by this version; see the downgrade note above for going the other way.

## Install

Windows — the MSI from the Assets below. macOS — the DMG, then clear the quarantine flag once, since the app still isn't notarized:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

Verified: `cargo fmt --check`, `npx tsc --noEmit`, clippy across the workspace, `cargo test -p ymux --lib` — 351 (340 pass, 8 pre-existing Windows-only OSC 7 failures, 3 ignored), `cargo test -p ytheme -p yipc -p ypath -p ylauncher` — 33, `npx vitest run` — 582 across 43 files.

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.10.1...v0.11.0
