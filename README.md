# ymux

**English** | [한국어](./README.ko.md) | [日本語](./README.ja.md)

<p align="center">
  <a href="https://ko-fi.com/youngminkim">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Support on Ko-fi" />
  </a>
</p>

---

A lightweight, tmux-inspired terminal multiplexer for Windows.

Built with Tauri 2 (Rust) + WebView2 + xterm.js. Designed to stay small, fast, and
native on Windows while giving you saved layouts, per-pane working directories
and startup commands, a pluggable shell picker (cmd / PowerShell / pwsh / Git
Bash / WSL), and numbered workspaces that each remember their own layout.

## Features

- **Layouts that persist**: recursive horizontal / vertical splits. Each pane
  remembers its shell, `cwd`, and an optional startup command.
- **Live cwd inheritance**: splitting a pane opens the new pane in the same
  working directory the parent shell is currently in — not the stale startup
  directory. Powered by OSC 7 escape-sequence tracking.
- **Shell auto-detection**: scans the system for `cmd.exe`, Windows PowerShell,
  PowerShell 7 (`pwsh`), Git Bash, and WSL distros, and exposes them as
  selectable profiles.
- **Numbered workspaces**: `Ctrl+Alt+1` .. `Ctrl+Alt+9` switch between
  workspaces. Every workspace saves its own layout. Panes stay alive across
  switches (tmux-style) so your REPLs and tails don't die.
- **Clickable URLs**: `Ctrl+Click` on any `http://` or `https://` link inside
  a terminal opens it in your default browser.
- **Keyboard shortcut reference**: press `?` in the top-right corner of the
  toolbar for a built-in cheat sheet. Supports English, 한국어, and 日本語.
- **Lightweight**: Tauri binary + WebView2. Installer target < 10 MB.

## Development

Requires: Rust (stable), Node 20+, pnpm (or npm).

```sh
pnpm install
pnpm tauri dev          # run in dev mode
pnpm tauri build        # produce Windows installer (run on Windows)
```

On non-Windows hosts the Rust crate still `cargo check`s cleanly so you can work
on cross-platform logic from Linux/macOS, but a full `tauri build` and end-to-end
PTY verification must be done on Windows.

## Config

`%APPDATA%\ymux\config.toml` stores workspaces, layouts, and cached shell
profiles. It is rewritten on every structural change (debounced) and on app
close.

## Keyboard shortcuts

| Shortcut                    | Action                               |
|-----------------------------|--------------------------------------|
| `Ctrl+Shift+H`              | Split pane horizontally              |
| `Ctrl+Shift+V`              | Split pane vertically                |
| `Ctrl+Shift+W`              | Close focused pane                   |
| `Ctrl+Tab`                  | Focus next pane                      |
| `Ctrl+Shift+Tab`            | Focus previous pane                  |
| `Ctrl+Alt+1` .. `Ctrl+Alt+9` | Switch workspace                    |
| `Ctrl+Click` on a URL       | Open link in default browser         |
| `?` button (toolbar)        | Show / hide this shortcut reference  |

> **Tip:** the `?` button in the top-right corner of the toolbar opens a
> built-in reference popup where you can also switch the display language.

## Status

Early MVP. See `docs/` (TBD) for the roadmap.
