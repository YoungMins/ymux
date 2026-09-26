# ymux v0.13.0

A launcher for your AI agents, one ymux at a time, clickable Markdown paths, and fixes for splitting next to tab groups and for zsh on upgraded Macs.

## Features

### "+" launcher

A new **+** button in the top bar starts things in one click:

- **Agents installed on this machine.** ymux looks on your `PATH` and in the usual install folders (`~/.local/bin`, `~/.bun/bin`, npm's global folder, Homebrew, …) and lists what it finds: Claude Code, Codex, Gemini CLI, Kimi CLI, Qwen Code, Aider, Amp, GitHub Copilot CLI, Crush, Cline, Factory droid, Kiro CLI and opencode. Picking one opens a new tab in the current group with the agent already running.
- **Agents start with permission prompts bypassed** — marked ⚡ in the menu, with the exact command in the tooltip:
  - Claude Code `--dangerously-skip-permissions`
  - Codex `--dangerously-bypass-approvals-and-sandbox` (this also turns off Codex's sandbox)
  - Gemini, Kimi, Qwen, Copilot, Crush, Cline `--yolo`; Aider `--yes-always`; Amp `--dangerously-allow-all`; droid `--skip-permissions-unsafe`; Kiro `--trust-all-tools`
  - opencode has no such switch for its interactive screen, so it starts normally (shown as "no bypass").

  In this mode an agent edits files and runs commands without asking. Use it in folders where that's what you want.
- **Terminals** — your default shell or any other detected shell.
- **Files, Git and Browser** panes.
- **Rescan agents** picks up something you installed after ymux started.

Agents launched this way show up in the agent tree like any other. Because the command is saved with the tab, an agent that can't resume a session (Gemini, Kimi, …) starts a fresh session — again with its bypass flag — the next time ymux starts.

### One ymux at a time

Launching ymux while it's already running — from the Start menu, the taskbar, the Dock or `ymux` in a terminal — brings the running window back (including from the tray) instead of starting a second copy.

### Markdown paths open in the viewer

Ctrl+click (Cmd+click on macOS) a `.md` path in terminal or agent output and it opens rendered in the viewer tab next to that terminal. Other files still open with their usual app.

Paths inside brackets are clickable now too: `(src/a.md)`, `[notes](docs/a.md)`, `● Read(src/app.ts)`, `(main.rs:42:7)`. Paths that contain their own parentheses, like `src/(group)/page.tsx`, keep working.

## Fixes

- **Split did nothing on some panes.** Any pane below or to the right of a tab group couldn't be split — the shortcut, the palette and the right-click menu all silently did nothing, and each attempt started an invisible shell in the background. Splitting a tab now splits its whole group.
- **macOS: `claude` (and other tools) "command not found" in zsh after upgrading.** On Macs set up with an older ymux, a zsh pane could skip your `.zprofile` and `.zshrc`, losing `PATH` entries like `~/.local/bin` and Homebrew. The shell list is re-detected once on first launch to clear the stale setting; your panes keep their shells.
- Ctrl+Shift+T no longer does nothing right after switching workspaces.

## Upgrading on Windows

ymux keeps running in the tray when you close its window, so **choose Quit from the tray icon before installing this update.**
