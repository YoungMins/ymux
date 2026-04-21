<h1 align="center">ymux</h1>

<p align="center">
  <strong>English</strong> &nbsp;·&nbsp; <a href="./README.ko.md">한국어</a> &nbsp;·&nbsp; <a href="./README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.7.0-7fdbca?style=flat-square" alt="version 0.7.0" />
</p>

<p align="center">
  <a href="https://ko-fi.com/youngminkim">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Support on Ko-fi" />
  </a>
</p>

---

A lightweight, tmux-inspired terminal multiplexer for Windows.

https://github.com/user-attachments/assets/705fff59-0bda-4460-a87f-d7ba6f50993a

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
- **HotKey buttons per pane**: bind frequently-used commands (single line or
  batch multi-line) to labelled buttons on top of every terminal. Click →
  the command is injected into the shell. Manage them from the `⚙` button.
- **Browser panes**: drop an iframe-based browser into any layout slot via the
  toolbar's `+ Browser` button. URL bar with back / forward / reload. The URL
  persists across workspace switches and app restarts, just like a terminal.
  > **Note:** the browser pane is implemented as an HTML `<iframe>`, so sites
  > that reject embedding via `X-Frame-Options` or CSP `frame-ancestors`
  > (e.g. github.com, google.com) will not load. It's designed for development
  > use — local dev servers, Storybook, internal dashboards, API docs,
  > localhost previews, etc. — not general web browsing.
- **Pane zoom**: `Ctrl+Shift+Z` hides every other pane so you can focus.
  Press again to restore the split.
- **Scrollback search**: `Ctrl+F` opens a find bar on the focused terminal.
  Enter / Shift+Enter step through matches; Esc closes.
- **Rename panes**: `Ctrl+Shift+R` gives the focused pane a custom title.
- **Update notifications**: a background poller checks GitHub releases every
  6 hours and surfaces a dismissable banner when a newer version ships. No
  auto-install — you stay in control.
- **System monitor status bar**: a thin bottom bar streams live CPU / RAM /
  GPU / disk / network ↑↓ every 2 seconds. Values turn amber at 70% and red at
  90%. Multi-GPU and multi-disk rigs are handled (inline up to 3 entries, then
  collapsed with a tooltip breakdown).
- **Support on Ko-fi**: a ☕ Support button next to `?` opens
  [ko-fi.com/youngminkim](https://ko-fi.com/youngminkim) in the system browser.
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
pnpm tauri build        # produce Windows MSI installer (run on Windows)
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
| `Ctrl+Shift+Z`              | Zoom / unzoom focused pane           |
| `Ctrl+Shift+R`              | Rename focused pane                  |
| `Ctrl+F`                    | Search terminal scrollback           |
| `Ctrl+Tab`                  | Focus next pane                      |
| `Ctrl+Shift+Tab`            | Focus previous pane                  |
| `Ctrl+Alt+1` .. `Ctrl+Alt+9` | Switch workspace                    |
| `Ctrl+Click` on a URL       | Open link in default browser         |
| `?` button (toolbar)        | Show / hide this shortcut reference  |

> **Tip:** the `?` button in the top-right corner of the toolbar opens a
> built-in reference popup where you can also switch the display language.

## Status

Early MVP. See `docs/` (TBD) for the roadmap.
