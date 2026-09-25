# ymux v0.11.2

The first release since v0.10.1, and the biggest this project has shipped. v0.11.0 and v0.11.1 were tagged but never published — their macOS builds failed — so everything they contained is here, together with the fix that got the Mac build through.

In short: the `ydir` / `ycode` / `ygit` / `ymon` terminal tools are replaced by GUI panes that ymux renders itself, ymux now ships as a single program with no helper binaries, agents resume where they left off, file paths in terminal output are clickable, and every command ymux exposes is locked against the web pages it hosts.

## Breaking: the helper tools are gone

`ydir`, `ycode`, `ygit` and `ymon` no longer ship as standalone commands, and neither does `y`. Typing any of them now fails with "command not found".

What replaces them, as panes ymux renders alongside terminals and browsers:

- **Files pane** — browse, preview, rename, create, multi-select, copy/move, delete to trash, with a prompt before anything is overwritten. Opens from a terminal's right-click menu ("Files here") or the command palette; the right-side file dock now shows one instead of running `ydir` in a terminal.
- **Editor pane** — CodeMirror 6: syntax highlighting, find and replace, go to line, multi-cursor, selection and clipboard. Saves keep a file's line endings and BOM and are atomic, so a crash mid-save can't corrupt the file. Every way of closing an editor with unsaved changes asks first; a crash-safety draft, written two seconds after your last edit, covers the one path it can't intercept (macOS Cmd+Q). If an agent rewrites a file you have open, you're told instead of having your edits silently overwritten.
- **Git pane** — commit graph, branches with checkout, worktrees. Anything that could lose work — checking out over uncommitted changes, removing a worktree with changes or ignored files in it — says exactly what's at stake and makes Cancel the default. A worktree is never removed while another pane is still working inside it.

`ymon` has no in-app replacement: the status bar already shows the same CPU / RAM / GPU / disk / network figures, and the process list is better served by the OS task manager. `ycode`'s Markdown preview is deferred, not replaced.

**If you have a hand-written startup command or HotKey button that runs `ycode foo.rs` or similar, it will break.** Those are strings you wrote and aren't migrated automatically — open the file from the Files pane or the dock instead.

**If you had agent tracking turned on, there is nothing to do.** On first launch ymux replaces its own old `y` entries in `~/.claude/settings.json` with the new mechanism below. Every other hook and setting in that file is left exactly as it was, key order included, and the original is backed up once as `settings.json.ymux-bak`.

**Downgrading:** an older ymux doesn't recognise the new pane kinds and falls back to a temporary config rather than overwriting your real one — that session shows default layouts, and upgrading again restores everything.

## Features

### Agent session resume

Restart ymux while a pane is running Claude Code or Codex, and that pane picks the conversation back up instead of showing a static picture of the old scrollback: `claude --resume <id> --dangerously-skip-permissions` — the skip-permissions flag is always added, so you're not re-approving what you'd already approved — or `codex resume <id>`. Eligible for up to 24 hours after the session was last active.

This works even if agent tracking is off. ymux ties each session to the agent process that actually ran in that pane — from Claude Code's hooks when tracking is on, otherwise from the CLI's own session files and command line — so two panes in the same folder, or a Claude started in another terminal, never trade conversations. When ymux can't be certain which session a pane owned, it starts that pane fresh rather than guess. A resume that fails leaves the pane's old scrollback in place and isn't retried. Only flags that are safe to repeat are carried over from a pane's startup command; a prompt in it is not re-sent. Plain shell panes keep restoring their scrollback exactly as before.

### Agent tracking without a helper program

When **Settings → Track Claude Code agents** is on, ymux listens on a local port (`127.0.0.1` only) and registers it with Claude Code as an HTTP hook. Each terminal pane carries its pane id and a per-launch secret, and Claude Code sends both with every event, so ymux knows which pane a session belongs to and ignores anything it didn't issue. The agent tree shows each Claude session, its subagents, and whether it's working, waiting for your approval, or done.

Two things to know while tracking is on:

- **Claude sessions outside ymux send their hook events to that local port too**, since the hooks are part of your user-wide Claude Code settings. ymux discards anything without a pane's secret. Turning tracking off removes the hooks.
- **If ymux isn't running, other Claude sessions show a hook notice each turn** ("Stop hook error occurred · ctrl+o to see"). The session isn't blocked or slowed. Turn tracking off if you'd rather not see it.

A session's id arrives with your first prompt rather than when Claude starts, because Claude Code doesn't send HTTP hooks for session start. If you run two copies of ymux, the second leaves the port and your settings alone; its panes just don't feed the agent tree.

### Clickable file paths

`Ctrl+Click` (`Cmd+Click` on macOS) already opened URLs; it now also opens file paths a command printed — `cat`, `git log`, compiler errors (`file:line:col`), an agent's answer. Only documents, images and source files open with their default program; everything else — executables, scripts, installers, disk images, anything without an extension or with an execute bit — is revealed in the file manager instead. Paths on network shares are never probed, so hovering over output can't make Windows contact another machine.

### Workspace panel

Double-click a workspace's name to rename it in place. The active workspace's whole block — its row and every pane, tab and agent under it — is outlined, so it's obvious which one you're in. When agent tracking is off, the panel says so, with a button that turns it on.

### Locked against embedded web content

Pages you open in a browser pane could previously call ymux's own internal commands — including ones that start programs or type into a terminal. Every command now checks that the caller is ymux's own window before doing anything, ymux refuses to load if something else frames it, and the embedded browser forwards only a fixed set of harmless shortcuts — nothing that opens a terminal or closes a pane. The agent-tracking listener accepts connections from this machine only, needs a pane's secret, and uses a session id for resume only when the pane's own Claude process confirms it. These changes were reviewed independently before release.

## Fixed

- **macOS build.** The Git pane decided which worktree is "current" by comparing git's paths with the folder it was handed. git resolves symlinks and a shell's working directory often doesn't — every macOS temp folder is `/var/…` to the shell and `/private/var/…` to git — so on a Mac the check could miss, and the test for it failed the v0.11.0 and v0.11.1 Mac builds. ymux now asks git for the root before comparing.
- The in-app shortcut list now matches the real bindings (several were missing) and shows `Cmd` on macOS.

## Under the hood

- No helper binaries: `tools/`, `crates/yipc`, `crates/yversion`, the `externalBin` bundle entries, `scripts/build-tools.mjs` and CI's placeholder-sidecar step are gone. `cargo check -p ymux` needs nothing staged beside it.
- New backend modules: `fsx`/`fsops` (Files pane), `textfile` (EOL/BOM-safe reads, atomic writes), `drafts` (editor crash drafts), `fspath` (resolving and opening paths from terminal text), `ipc_guard` (the per-command caller check), `agent_sessions`/`agent_binding`/`agent_scan_disk` (resume), `hook_http` (the agent-tracking listener) — each unit-tested on Linux CI.
- `ipc_guard::tests::every_registered_command_starts_with_a_guard` parses the command registration and every command body, so a new command without a guard fails CI instead of shipping.
- CLAUDE.md gained rules 11–16 covering the settings that deliberately stay out of the layout merge, the Claude settings merge, the hook listener, hidden tabs, path comparison and the command guard.

## Compatibility

`CONFIG_VERSION` stays at 8 — no forced shell re-detection, no config migration. The new pane kinds (`files`, `editor`, `git`) are additive and load transparently.

## Install

Windows — the MSI from the Assets below. macOS — the DMG, then clear the quarantine flag once, since the app still isn't notarized:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

Verified before tagging: `cargo fmt --check`, clippy across the workspace, `npx tsc --noEmit`, `cargo test -p ymux --lib` — 450 passing on Windows (the 8 OSC 7 tests that fail only on Windows pass on Linux and macOS CI), `cargo test -p ytheme -p ypath` — 15, `npx vitest run` — 616.

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.10.1...v0.11.2
