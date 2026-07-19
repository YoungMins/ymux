<h1 align="center">ymux</h1>

<p align="center">
  <strong>English</strong> &nbsp;·&nbsp; <a href="./README.ko.md">한국어</a> &nbsp;·&nbsp; <a href="./README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.8.22-7fdbca?style=flat-square" alt="version 0.8.22" />
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
- **Numbered workspaces**: `Ctrl+Alt+1` .. `Ctrl+Alt+9` switch between the
  first nine workspaces. Add more at any time with the `+` button in the
  toolbar — there is no fixed limit — and remove one with the `×` that appears
  when you hover a workspace tab (the last remaining one can't be deleted).
  Every workspace saves its own layout. Panes stay alive across switches
  (tmux-style) so your REPLs and tails don't die. Double-click a workspace
  button to give it a custom name.
- **Git worktree panes**: the command palette's **"Open pane in new git
  worktree"** prompts for a branch name, creates a git worktree (default: a
  sibling `.ymux-worktrees/<branch>` dir next to the repo, override with
  `worktree_base_dir`), and opens a terminal pane already cd'd into it — an
  isolated checkout per AI agent or experiment. Closing the pane — or deleting
  the whole workspace — offers to remove the worktree again (branches and
  commits are never touched).
- **Persistent scrollback**: terminal output survives app restarts. Each
  pane's buffer (colors included) is restored beneath a dimmed *"session
  restored"* separator on the next launch. Toggle under **Settings → General**.
- **Agent status at a glance**: every terminal pane tracks its process —
  idle / running / done / needs-attention — from output activity and the
  bell / OSC 9 completion signal, shown as a colored pane border plus a dot on
  the workspace tab. When a CLI finishes out of sight you also get an OS
  notification and a short beep (toggleable in Settings).
- **Per-pane settings (⚙)**: the `⚙` button on each terminal opens a settings
  panel where you can set a **custom background color** (via native color picker)
  and manage **HotKey buttons** (single-line or batch multi-line commands bound
  to labelled buttons above the terminal). Background colors persist across
  restarts.
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
- **Support on Ko-fi**: a ☕ Support button next to `⚙` opens
  [ko-fi.com/youngminkim](https://ko-fi.com/youngminkim) in the system browser.
- **Clickable URLs**: `Ctrl+Click` on any `http://` or `https://` link inside
  a terminal opens it in your default browser.
- **Settings panel (⚙)**: WinUI 3-style modal with a left sidebar (General,
  Syntax Colors, Shortcuts, Tools, Config Files) and right content pane.
  Pick the language, edit yCode's syntax color palette with live color
  pickers, browse the built-in shortcut/tool reference, or jump straight
  to the underlying `theme.toml` in your default editor.
- **Command palette**: `Ctrl+Shift+P` opens a VS Code-style searchable
  command overlay. Fuzzy-match any built-in action by name or keybinding.
- **Per-workspace notes**: every workspace has its own notes button right
  next to the workspace number, plus `Ctrl+Alt+N` to toggle notes for the
  active workspace. Notes persist across sessions via `localStorage`, and
  the icon is highlighted in the accent color when notes exist.
- **Clipboard paste — text and images**: `Ctrl+V` pastes clipboard text into
  the focused terminal. If the clipboard holds an **image** (e.g. a
  `Win+Shift+S` screenshot), it is saved to a self-cleaning temp file and its
  quoted path is typed instead — hand screenshots straight to an in-pane AI
  CLI like Claude Code. Pasted images are pruned after 24 h (configurable via
  `paste_image_retention_hours`).
- **13-language i18n**: English, 한국어, 日本語, 中文, हिन्दी, Español,
  Français, العربية, Português, Русский, Türkçe, Deutsch, Tiếng Việt.
  Switch from the language selector in the bottom-right status bar.
- **MSI installer with PATH**: the MSI adds the install directory to the
  system PATH, so `ymux`, `ymon`, `ydir`, `ycode`, `ygit`, and `y` are immediately
  available from any terminal after install.
- **Lightweight**: Tauri binary + WebView2. Installer target < 10 MB.

### Companion TUI tools

Standalone binaries that run inside any ymux terminal pane. Install them
alongside ymux or use the `y` launcher (`y mon`, `y dir`, `y code`, `y git`).

| Tool | Command | Description |
|------|---------|-------------|
| **ymon** | `ymon` | htop/btop-style system monitor (CPU, memory, disk, processes) |
| **ydir** | `ydir` | Dual-pane file manager — navigate, copy/move/delete, run executables with args dialog |
| **ycode** | `ycode <file>` | TUI code editor — syntax highlighting (~200 languages incl. Svelte), `Ctrl+B` file-tree sidebar with dirty-buffer prompt, undo/redo, search, goto, Esc exit dialog, full CJK/emoji support |
| **ygit** | `ygit` | Git log & branch viewer — colored commit graph, branch list, checkout |
| **y** | `y help` | Launcher that lists and dispatches all y* tools |

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

`%APPDATA%\ymux\theme.toml` stores the shared color palette read by every
y* tool (yMux UI, yCode syntax highlighting, etc.). Edit it with the
**Settings → Syntax Colors** picker, or open the file directly via
**Settings → Config Files → Open**.

## Keyboard shortcuts

| Shortcut                    | Action                               |
|-----------------------------|--------------------------------------|
| `Ctrl+Shift+H`              | Split pane horizontally              |
| `Ctrl+Shift+V`              | Split pane vertically                |
| `Ctrl+Shift+W`              | Close focused pane                   |
| `Ctrl+Shift+Z`              | Zoom / unzoom focused pane           |
| `Ctrl+Shift+←/→`            | Swap pane with previous / next       |
| `Ctrl+Shift+R`              | Rename focused pane                  |
| `Ctrl+Shift+P`              | Open command palette                 |
| `Ctrl+Alt+N`                | Toggle notes for active workspace    |
| `Ctrl+V`                    | Paste clipboard text (image → temp-file path) |
| `Ctrl+F`                    | Search terminal scrollback           |
| `Ctrl+Tab`                  | Focus next pane                      |
| `Ctrl+Shift+Tab`            | Focus previous pane                  |
| `Ctrl+Alt+1` .. `Ctrl+Alt+9` | Switch workspace                    |
| Double-click workspace button | Rename workspace                    |
| `Ctrl+Click` on a URL       | Open link in default browser         |
| `⚙` button (toolbar)        | Open Settings (palette, shortcuts, syntax colors, config files) |

> **Tip:** the `⚙` button in the top-right corner of the toolbar opens the
> Settings modal — a WinUI 3-style sidebar/content layout where you can
> switch the display language, edit yCode's syntax highlighting palette
> with color pickers, and one-click open the underlying `theme.toml`
> config file.

## Status

Actively developed — new releases ship regularly with an automated
Linux-test + Windows-MSI CI pipeline behind every tag. See
[Releases](https://github.com/YoungMins/ymux/releases) for the changelog.
