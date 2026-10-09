<h1 align="center">ymux</h1>

<p align="center">
  <strong>English</strong> &nbsp;·&nbsp; <a href="./README.ko.md">한국어</a> &nbsp;·&nbsp; <a href="./README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.13.6-7fdbca?style=flat-square" alt="version 0.13.6" />
</p>

<p align="center">
  <a href="https://ko-fi.com/youngminkim">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Support on Ko-fi" />
  </a>
</p>

---

A lightweight, tmux-inspired terminal multiplexer for Windows and macOS.

https://github.com/user-attachments/assets/705fff59-0bda-4460-a87f-d7ba6f50993a

ymux splits one window into as many terminals as you need, saves the layout per
workspace, and keeps shells, scrollback and AI coding agents organized. It is
built with Tauri 2 (Rust) and xterm.js, on WebView2 (Windows) and WKWebView
(macOS).

## Install

### Windows

**Microsoft Store** (available once the listing is published):
<https://apps.microsoft.com/detail/9N20Z9NKD2Q1>. The Store build uses the
system Evergreen WebView2 runtime (built into Windows 11, delivered by Windows
Update on Windows 10) and registers a `ymux` app execution alias, so `ymux`
runs from any terminal.

**MSI**: download `ymux_*_x64_en-US.msi` from
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

### Terminals and layouts

- **Layouts that persist**: recursive horizontal / vertical splits. Each pane
  remembers its shell, `cwd`, and an optional startup command.
- **Pane tabs**: `Ctrl+Shift+T` adds a tab to the focused pane; hidden tabs keep
  running.
- **Live cwd inheritance**: splitting a pane opens the new pane in the directory
  the parent shell is in now, not its startup directory (OSC 7 tracking).
- **Shell auto-detection**: on Windows, `cmd.exe`, Windows PowerShell,
  PowerShell 7 (`pwsh`), Git Bash and WSL distros. On macOS, your login shell
  plus any zsh / bash / fish it finds (Homebrew included). zsh and bash start
  through a generated shim that sources your own dotfiles first and then adds
  the OSC 7 hook.
- **Persistent scrollback**: each pane's buffer (colors included) is restored
  beneath a dimmed *"session restored"* separator on the next launch. Toggle
  under **Settings → General**.
- **Pane zoom, rename, search**: `Ctrl+Shift+Z` hides every other pane,
  `Ctrl+Shift+R` renames the focused pane, `Ctrl+F` opens a find bar on the
  terminal (Enter / Shift+Enter step through matches).
- **Bottom-anchored prompt**: when output is shorter than the pane, the prompt
  sits at the bottom edge and output grows upward. On by default; toggle under
  **Settings → General**.
- **Clickable links and paths**: `Ctrl+Click` opens `http://` / `https://` links
  in your default browser, and filesystem paths a command printed with the OS
  default program. Executables and scripts are revealed in the file manager
  instead of run.
- **Paste text and images**: `Ctrl+V` pastes text. If the clipboard holds an
  image (for example a `Win+Shift+S` screenshot), it is saved to a self-cleaning
  temp file and its quoted path is typed instead. Pruned after 24 h
  (`paste_image_retention_hours`).
- **Drag files in**: drop files or folders onto a terminal and their quoted paths
  are typed at the cursor. Nothing runs until you press Enter.
- **Per-pane settings (⚙)**: set a custom background color and HotKey buttons
  (single- or multi-line commands on labelled buttons above the terminal).
- **Close to tray**: closing the window hides ymux to the tray (Windows
  notification area / macOS menu bar) and keeps every shell and agent running.
  Click the tray icon, or launch ymux again, to bring it back (one ymux runs at
  a time). Quit from the tray menu, `Cmd+Q` on macOS, or **Quit ymux** in the
  command palette (unsaved editors are still asked about).

### Workspaces

- **Numbered workspaces**: `Ctrl+Alt+1` .. `Ctrl+Alt+9` switch between the first
  nine. Add more with the `+` button in the workspace panel and remove one with
  the `×` on its row (the last one can't be deleted). Each workspace saves its
  own layout, and panes stay alive across switches so REPLs and tails don't die.
- **Workspace panel**: a collapsible, scrollable list on the left. Double-click a
  name to rename; drag a row to reorder. The active workspace is outlined.
- **Per-workspace notes**: a notes button beside each workspace number, plus
  `Ctrl+Alt+N` for the active workspace. Stored in `localStorage`.
- **Git worktree panes**: the command palette's **"Open pane in new git
  worktree"** asks for a branch name, creates a worktree (default: a sibling
  `.ymux-worktrees/<branch>` directory, override with `worktree_base_dir`) and
  opens a terminal in it. Closing the pane, or deleting the workspace, offers to
  remove the worktree again; branches and commits are never touched.

### AI agents

- **Agent status at a glance**: every terminal pane shows idle / running / done /
  needs-attention, from output activity and the bell / OSC 9 completion signal,
  as a colored pane border plus a tint on its workspace row. When a CLI
  finishes out of sight you get an OS notification and a short beep (toggle in
  Settings).
- **Launcher**: the top **+** menu lists installed agent CLIs, your shells and
  the GUI panes. Choose **Rescan** to refresh the list.
- **Agent tree**: the workspace panel lists every pane and tab with the agents
  running in it: Claude Code sessions and their subagents (via hooks), plus
  Codex, Gemini, aider and other CLIs (via a lightweight process scan). Click a
  row to jump to that pane.
- **Session resume**: restart ymux while a pane runs Claude Code or Codex and
  the pane resumes the same conversation: `claude --resume <id>
  --dangerously-skip-permissions` (always added, so you aren't re-approving what
  you already approved) or `codex resume <id>`. Eligible for 24 h after the
  session was last active. Works without agent tracking, by reading the CLI's
  own transcripts from disk. Shell-only panes keep restoring scrollback.
- **Claude Code hook tracking** (off by default, **Settings → General**): adds
  user-level HTTP hooks to `~/.claude/settings.json`. While it is on, *every*
  Claude Code session on the machine, including ones outside ymux, sends its
  hook events (prompts, tool input and output) to ymux's port on `127.0.0.1`.
  ymux ignores events that don't come from its own panes; turning tracking off
  removes the hooks. If ymux is not running while tracking is on, Claude Code
  reports those hooks as failed (for example "Stop hook error occurred"), so
  turn tracking off before you uninstall ymux. ymux ships no helper binary.

### Side panel: Files and Usage

One panel on the right, shared by all workspaces. Icons at the top switch
between two views, and each keeps its state while hidden. The open state, view
and width persist; drag the left edge to resize.

- **Files**: toggle with `Ctrl+Shift+E`. Follows the active pane's working
  directory as you `cd`. Enter on a file opens it in a reused editor tab, and a
  preview of the selected file or folder sits beneath the list (`Tab` shows and
  hides it).
- **Usage**: open it from the toolbar's usage icon, the **+** launcher
  (*AI token usage*) or the command palette. It shows:
  - local token counts for Claude Code and Codex, read from their session logs,
    for the last 5 hours, last 7 days or all recorded history, grouped by AI
    provider, model or project;
  - remaining 5-hour and weekly quota with a reset countdown, shown as
    remaining or used (your choice). Quotas are fetched directly from Anthropic
    and OpenAI using the sign-in token those CLIs already stored on your PC
    (see [Privacy](#privacy)).

  It refreshes every 10 seconds while visible. Local totals exclude web chats
  and other devices and can include previously used accounts, so they are not
  the same as your subscription quota. When a quota can't be fetched, a recorded
  snapshot is shown and labelled stale.

### Built-in panes

Rendered by ymux itself, next to terminals. Open them from a terminal's
right-click menu (splits that pane and inherits its working directory), the **+**
launcher, or the command palette.

| Pane | What it does |
|------|--------------|
| **Files** | Browse, preview, rename, create, multi-select, copy/move, overwrite prompts, delete to trash (with confirmation) |
| **Editor** | Syntax-highlighted text editor: save, find/replace, go to line, CRLF/BOM preserved, unsaved-changes guard, crash-safety drafts, external-change detection |
| **Git** | Commit graph, branch list and checkout, worktree add/remove (with confirmation) |
| **Browser** | Iframe browser with URL bar and back / forward / reload; the URL persists across switches and restarts |

> **Note:** the browser pane is an HTML `<iframe>`, so sites that refuse
> embedding via `X-Frame-Options` or CSP `frame-ancestors` (for example
> github.com, google.com) won't load. It is meant for local dev servers,
> Storybook, internal dashboards and docs, not general browsing.

### Productivity

- **Command palette**: `Ctrl+Shift+P`, fuzzy-match any built-in action by name
  or keybinding.
- **System monitor**: a bottom status bar streams CPU / RAM / GPU / disk /
  network every 2 seconds. Values turn amber at 70% and red at 90%.
- **Update notifications**: a background check of GitHub releases every 6 hours
  shows a dismissable banner. Nothing is installed automatically.
- **Support on Ko-fi**: the ☕ Support button next to `⚙` opens
  [ko-fi.com/youngminkim](https://ko-fi.com/youngminkim).

### Customization

- **Settings (⚙)**: General, Syntax Colors, Shortcuts and Config Files sections.
  Pick the language, edit the editor's syntax palette, browse the shortcut
  reference, or open `theme.toml` directly.
- **13 languages**: English, 한국어, 日本語, 中文, हिन्दी, Español, Français,
  العربية, Português, Русский, Türkçe, Deutsch, Tiếng Việt. Switch from the
  selector in the bottom-right status bar.
- **Hardened against embedded web content**: every backend command checks the
  caller's origin, so a page in a browser pane can't invoke ymux's IPC. ymux
  refuses to load if framed by another page, and the built-in browser forwards
  only a small fixed set of harmless shortcuts.

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
| `Ctrl+Shift+E`              | `Cmd+Shift+E`      | Toggle the side panel (Files view)   |
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
| Double-click workspace name | —                  | Rename workspace                     |
| Drag a workspace row        | —                  | Reorder workspaces                   |
| Right-click in a terminal   | —                  | Context menu: copy/paste, split, open a files / editor / git pane |
| `⚙` button (toolbar)        | —                  | Open Settings (language, shortcuts, syntax colors, config files) |

The Usage view has no shortcut: open it from the toolbar's usage icon, the **+**
launcher or the command palette.

## Configuration

`config.toml` stores workspaces, layouts and cached shell profiles. It is
rewritten on every structural change (debounced) and on app close.

`theme.toml` stores the color palette, including the editor's syntax colors.
Edit it with **Settings → Syntax Colors**, or open the file via
**Settings → Config Files → Open**.

Both live in the ymux config directory:

| Platform | Path |
|----------|------|
| Windows  | `%APPDATA%\ymux\` |
| macOS    | `~/Library/Application Support/ymux/` |

On macOS the same directory also holds the generated shell-integration files
(`zsh-init/`, `bash-init.sh`). They are rewritten on every shell detection and
are safe to delete.

## Privacy

ymux has no accounts, analytics or telemetry, and runs no servers. Layouts,
scrollback, notes and settings stay on your PC. The only requests ymux makes on
its own are the GitHub update check and, if you use the Usage view, quota
requests to Anthropic and OpenAI that send the token those CLIs already stored
locally to the provider that issued it. See the full
[privacy policy](./packaging/microsoft-store/PRIVACY.md).

## Development

Requires Rust (stable), Node 20+ and pnpm.

```sh
pnpm install
pnpm tauri dev          # run in dev mode
pnpm tauri build        # MSI on Windows, .app + .dmg on macOS
pnpm test               # fmt + tsc + clippy + tests
npx tsc --noEmit        # TypeScript type check
cargo clippy --workspace -- -D warnings
cargo test -p ytheme -p ypath
cargo test --no-default-features --lib -p ymux
```

`tauri build` produces the installer for the host you run it on (the MSI bundler
is Windows-only and the DMG bundler macOS-only), which is what the release
workflow does. On Linux the Rust crate still checks cleanly with
`cargo check --no-default-features --lib --tests -p ymux`, but the desktop app is
not shipped for Linux.

Maintainers: the Microsoft Store MSIX package is built with
`scripts/package-msix.ps1`; see
[packaging/microsoft-store/README.md](./packaging/microsoft-store/README.md).

## Status

Actively developed. Every tag goes through a Linux test, Windows MSI and macOS
DMG pipeline. See [Releases](https://github.com/YoungMins/ymux/releases) for the
changelog.

## License

[MIT](./LICENSE) © 2026 Kim YoungMin
