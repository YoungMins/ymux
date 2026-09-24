<h1 align="center">ymux</h1>

<p align="center">
  <strong>English</strong> &nbsp;·&nbsp; <a href="./README.ko.md">한국어</a> &nbsp;·&nbsp; <a href="./README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.11.0-7fdbca?style=flat-square" alt="version 0.11.0" />
</p>

<p align="center">
  <a href="https://ko-fi.com/youngminkim">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Support on Ko-fi" />
  </a>
</p>

---

A lightweight, tmux-inspired terminal multiplexer for Windows and macOS.

https://github.com/user-attachments/assets/705fff59-0bda-4460-a87f-d7ba6f50993a

Built with Tauri 2 (Rust) + xterm.js, on WebView2 (Windows) and WKWebView
(macOS). Designed to stay small, fast, and native while giving you saved
layouts, per-pane working directories and startup commands, a pluggable shell
picker (cmd / PowerShell / pwsh / Git Bash / WSL on Windows; zsh / bash / fish
on macOS), and numbered workspaces that each remember their own layout.

## Install

### Windows

Download `ymux_*_x64_en-US.msi` from
[Releases](https://github.com/YoungMins/ymux/releases) and run it. The installer
bundles a WebView2 bootstrapper and adds the install directory to `PATH`, so
`ymux` can be launched from any terminal.

### macOS (Apple Silicon, macOS 11+)

Download `ymux_*_macos_aarch64.dmg` from
[Releases](https://github.com/YoungMins/ymux/releases), open it, and drag ymux
to Applications.

The app is signed ad-hoc but **not notarized**, so Gatekeeper blocks it on first
launch with a "damaged" or "unidentified developer" message. Clear the download
quarantine flag once:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

## Features

- **Layouts that persist**: recursive horizontal / vertical splits. Each pane
  remembers its shell, `cwd`, and an optional startup command.
- **Live cwd inheritance**: splitting a pane opens the new pane in the same
  working directory the parent shell is currently in — not the stale startup
  directory. Powered by OSC 7 escape-sequence tracking.
- **Shell auto-detection**: on Windows, scans for `cmd.exe`, Windows
  PowerShell, PowerShell 7 (`pwsh`), Git Bash, and WSL distros. On macOS, your
  login shell plus any zsh / bash / fish it finds (Homebrew installs included).
  zsh and bash are launched through a generated shell-integration shim that
  sources your own dotfiles first and then adds the OSC 7 hook, so live `cwd`
  inheritance works without you editing anything.
- **Numbered workspaces**: `Ctrl+Alt+1` .. `Ctrl+Alt+9` switch between the
  first nine workspaces. Add more at any time with the `+` button in the
  workspace panel — there is no fixed limit — and remove one with the `×`
  that appears when you hover a workspace row (the last remaining one can't
  be deleted). Every workspace saves its own layout. Panes stay alive across
  switches (tmux-style) so your REPLs and tails don't die. Double-click a
  workspace name to rename it; the active workspace's whole block is
  outlined so it's obvious which one you're in.
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
  bell / OSC 9 completion signal, shown as a colored pane border plus a
  status tint on its row in the workspace panel. When a CLI finishes out of
  sight you also get an OS notification and a short beep (toggleable in
  Settings).
- **Agent session resume**: restart ymux while a pane is running Claude Code
  or Codex, and that pane resumes the same conversation instead of just
  replaying a picture of the old scrollback — `claude --resume <id>
  --dangerously-skip-permissions` (always added, so you're not re-approving
  everything you already approved) or `codex resume <id>`. Eligible for up
  to 24 h after the session was last active. Works even without agent
  tracking turned on — it reads the CLI's own transcripts from disk to find
  the session id. Shell-only panes are unaffected and keep restoring
  scrollback as before.
- **Agent tree**: the workspace panel lists every pane (and tab) under its
  workspace, with the coding agents running in it — Claude Code sessions and
  their subagents (via hooks), plus Codex, Gemini, aider, and other CLIs (via
  a lightweight process scan). Click any row to jump straight to that pane.
  Claude Code hook tracking is off by default — enable it under
  **Settings → General**.
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
- **File dock**: `Ctrl+Shift+E` toggles a collapsible right-side files pane
  that follows the active pane's working directory as you `cd` around.
  Press Enter on a file there to open it in a reused editor tab instead of
  leaving the dock. A preview of the selected file or folder sits beneath
  the list; `Tab` shows and hides it. Drag the dock's left edge to resize
  it.
- **Pane zoom**: `Ctrl+Shift+Z` hides every other pane so you can focus.
  Press again to restore the split.
- **Scrollback search**: `Ctrl+F` opens a find bar on the focused terminal.
  Enter / Shift+Enter step through matches; Esc closes.
- **Rename panes**: `Ctrl+Shift+R` gives the focused pane a custom title.
- **Bottom-anchored prompt**: when a terminal's output is shorter than the
  pane, the prompt is drawn against the bottom edge instead of sitting near
  the top, so output grows upward. On by default; toggle under
  **Settings → General**.
- **Update notifications**: a background poller checks GitHub releases every
  6 hours and surfaces a dismissable banner when a newer version ships. No
  auto-install — you stay in control.
- **System monitor status bar**: a thin bottom bar streams live CPU / RAM /
  GPU / disk / network ↑↓ every 2 seconds. Values turn amber at 70% and red at
  90%. Multi-GPU and multi-disk rigs are handled (inline up to 3 entries, then
  collapsed with a tooltip breakdown).
- **Support on Ko-fi**: a ☕ Support button next to `⚙` opens
  [ko-fi.com/youngminkim](https://ko-fi.com/youngminkim) in the system browser.
- **Clickable links and paths**: `Ctrl+Click` on any `http://` or `https://`
  link inside a terminal opens it in your default browser. `Ctrl+Click` also
  works on filesystem paths a command printed (`cat`, `git log`, an agent's
  answer) — it opens the file with the OS default program. Executables and
  scripts are revealed in the file manager instead of run.
- **Settings panel (⚙)**: WinUI 3-style modal with a left sidebar (General,
  Syntax Colors, Shortcuts, Config Files) and right content pane.
  Pick the language, edit the editor pane's syntax color palette with
  color pickers, browse the built-in shortcut reference, or jump straight
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
- **Drag files in for their paths**: drop files or folders onto a terminal
  pane and their quoted paths are typed at the cursor — the pane you dropped
  on, not just the focused one. Nothing is executed until you press Enter.
- **13-language i18n**: English, 한국어, 日本語, 中文, हिन्दी, Español,
  Français, العربية, Português, Русский, Türkçe, Deutsch, Tiếng Việt.
  Switch from the language selector in the bottom-right status bar.
- **MSI installer with PATH**: the MSI adds the install directory to the
  system PATH, so `ymux` is available from any terminal right after install.
- **Lightweight**: Tauri binary + WebView2. Installer target < 10 MB.
- **Hardened against embedded web content**: every backend command checks the
  caller's origin, so a page inside a browser pane can't invoke ymux's own
  IPC. ymux refuses to load if it's ever framed by another page, and the
  built-in browser only forwards a small fixed set of harmless shortcuts —
  nothing that could spawn a process or close a pane.

### Tool panes

Files, editor and git views are panes rendered by ymux itself, alongside
terminals and browsers. Open one from a terminal's right-click menu (it splits
that pane and inherits its working directory) or from the command palette.

| Pane | Opens with | What it does |
|------|-----------|--------------|
| **Files** | Right-click → *Files here*, the file dock | Browse, preview, rename, create, multi-select, copy/move, overwrite prompts, delete to trash (with confirmation) |
| **Editor** | Right-click → *Split: editor pane*, Enter on a file | Syntax-highlighted text editor with save, find/replace, go to line, CRLF/BOM preserved, unsaved-changes guard, crash-safety drafts, external-change detection |
| **Git** | Right-click → *Git here* | Commit graph, branch list and checkout, worktree add/remove (with confirmation) |

The standalone `ydir` / `ycode` / `ygit` / `ymon` commands and the `y` launcher
no longer exist. The install directory still holds a small `y` binary: it is
the relay Claude Code hooks call to feed the agent tree, not a command to run
by hand.

## Development

Requires: Rust (stable), Node 20+, pnpm (or npm).

```sh
pnpm install
pnpm tauri dev          # run in dev mode
pnpm tauri build        # MSI on Windows, .app + .dmg on macOS
```

`tauri build` produces the installer for whatever host you run it on — the MSI
bundler is Windows-only and the DMG bundler is macOS-only, so each installer has
to be built on its own platform (which is what the release workflow does).

On Linux the Rust crate still `cargo check`s cleanly, so cross-platform logic can
be developed there; the desktop app itself is not shipped for Linux.

## Config

`config.toml` stores workspaces, layouts, and cached shell profiles. It is
rewritten on every structural change (debounced) and on app close.

`theme.toml` stores the color palette, including the editor pane's syntax
highlighting colors. Edit it with the **Settings → Syntax Colors**
picker, or open the file directly via **Settings → Config Files → Open**.

Both live in the ymux config directory:

| Platform | Path |
|----------|------|
| Windows  | `%APPDATA%\ymux\` |
| macOS    | `~/Library/Application Support/ymux/` |

On macOS the same directory also holds the generated shell-integration files
(`zsh-init/`, `bash-init.sh`). They are rewritten on every shell detection and
are safe to delete.

## Keyboard shortcuts

On macOS every `Ctrl` below becomes `Cmd`, which leaves `Ctrl` free for the
shell (`Ctrl+C`, `Ctrl+D`, `Ctrl+R`). Two exceptions are called out in the
table: pane cycling stays on `Ctrl+Tab` because macOS reserves `Cmd+Tab` for
the application switcher, and workspace switching drops the `Alt`.

| Shortcut (Windows)          | macOS              | Action                               |
|-----------------------------|--------------------|--------------------------------------|
| `Ctrl+Shift+H`              | `Cmd+Shift+H`      | Split pane horizontally              |
| `Ctrl+Shift+V`              | `Cmd+Shift+V`      | Split pane vertically                |
| `Ctrl+Shift+W`              | `Cmd+Shift+W`      | Close focused pane                   |
| `Ctrl+Shift+T`              | `Cmd+Shift+T`      | New tab in focused pane              |
| `Ctrl+Shift+[` / `]`        | `Cmd+Shift+[` / `]` | Previous / next tab in pane         |
| `Ctrl+Shift+Z`              | `Cmd+Shift+Z`      | Zoom / unzoom focused pane           |
| `Ctrl+Shift+←/→`            | `Cmd+Shift+←/→`    | Swap pane with previous / next       |
| `Ctrl+Shift+R`              | `Cmd+Shift+R`      | Rename focused pane                  |
| `Ctrl+Shift+P`              | `Cmd+Shift+P`      | Open command palette                 |
| `Ctrl+Alt+N`                | `Cmd+Opt+N`        | Toggle notes for active workspace    |
| `Ctrl+Shift+E`              | `Cmd+Shift+E`      | Toggle file dock                     |
| `Ctrl+V`                    | `Cmd+V`            | Paste clipboard text (image → temp-file path) |
| `Ctrl+F`                    | `Cmd+F`            | Search terminal scrollback (find / replace in an editor pane) |
| `Ctrl+S`                    | `Cmd+S`            | Save the file (editor pane)          |
| `Ctrl+G`                    | `Cmd+G`            | Go to line (editor pane)             |
| `Ctrl++` / `Ctrl+-`         | `Cmd++` / `Cmd+-`  | Increase / decrease terminal font size |
| `Ctrl+0`                    | `Cmd+0`            | Reset terminal font size             |
| `Ctrl+Tab`                  | `Ctrl+Tab`         | Focus next pane                      |
| `Ctrl+Shift+Tab`            | `Ctrl+Shift+Tab`   | Focus previous pane                  |
| `Ctrl+Alt+1` .. `Ctrl+Alt+9` | `Cmd+1` .. `Cmd+9` | Switch workspace                    |
| `Ctrl+Click` on a URL/path  | `Cmd+Click`        | Open link, or a printed file path, with the OS default program |
| Double-click workspace name  | —                  | Rename workspace                     |
| Drag a workspace row        | —                  | Reorder workspaces                   |
| Right-click in a terminal   | —                  | Context menu — copy/paste, split, open a files / editor / git pane |
| `⚙` button (toolbar)        | —                  | Open Settings (palette, shortcuts, syntax colors, config files) |

> **Tip:** the `⚙` button in the top-right corner of the toolbar opens the
> Settings modal — a WinUI 3-style sidebar/content layout where you can
> switch the display language, edit the editor pane's syntax highlighting
> palette with color pickers, and one-click open the underlying `theme.toml`
> config file.

## Status

Actively developed — new releases ship regularly with an automated
Linux-test + Windows-MSI + macOS-DMG CI pipeline behind every tag. See
[Releases](https://github.com/YoungMins/ymux/releases) for the changelog.
