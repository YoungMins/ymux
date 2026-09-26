# ymux v0.12.0

Two new features — a rendered Markdown preview and close-to-tray — and a macOS parity sweep that fixes the bugs Mac users hit most, starting with commands that took arguments failing outright.

## Features

### Close to tray (Windows and macOS)

The window's close button (×, Alt+F4, the red button on macOS) now hides ymux instead of quitting it. Shells and agents keep running, and a tray icon (the menu bar on macOS) stays behind: click it to bring the window back, or right-click for **Open** / **Quit**. On macOS, clicking the Dock icon also brings the window back.

To actually quit, use the tray's **Quit**, **Cmd+Q** on macOS, or **Quit ymux** in the command palette. All three ask about unsaved editor changes first, exactly as closing did before.

**Before installing an update on Windows, choose Quit from the tray** — closing the window no longer stops ymux, so the installer would find it still running.

### Markdown preview

`.md` / `.markdown` files opened from the file dock or the Files pane now open as a rendered preview (GitHub-flavoured: tables, task lists, strikethrough). A toolbar button switches between the preview and the editor; switching back renders what's in the buffer, unsaved edits included, and Ctrl+S (Cmd+S) saves from either view. Links in the preview open web pages in your browser, jump to headings, or open other `.md` files in the same pane. Remote images aren't loaded.

## macOS fixes

- **Commands with arguments work again.** Every space typed into a terminal reached the shell as a non-breaking space, so `brew` ran but `brew install ruby` failed with "command not found". This affected every macOS release since v0.10.0.
- **A pane keeps the shell it opened with.** The first pane ymux ever created didn't record its shell, so it followed the default: after the default changed, a zsh pane came back as bash on the next launch. It is now recorded once at startup. A pane that already flipped needs to be recreated once — its original shell was never saved. (Windows had the same bug.)
- **Cmd+Q asks about unsaved changes** instead of quitting on the spot, and every quit path — Dock Quit and logout included — now saves pane layouts and folders.
- **Cmd+W no longer quits ymux.**
- **UTF-8 by default.** Launched from Finder, shells ran in the C locale; ymux now sets `LANG` from your macOS language (falling back to `en_US.UTF-8`) when you haven't set one. Panes also get `TERM_PROGRAM=ymux`.
- **zsh history lands in `~/.zsh_history` again**, not inside ymux's shell-integration folder, and dotfiles that use `${ZDOTDIR:-$HOME}` work as they do in Terminal.app.
- **bash panes get the same `PATH` as Terminal.app** — they now read `/etc/profile` and your `.bash_profile` like a login shell.
- Shell-integration files are refreshed on every launch, so fixes like these reach existing installs.
- npm- and Homebrew-installed Claude Code and Gemini now appear in the agent tree.
- Deleting to Trash no longer asks for permission to control Finder.
- Files hidden in Finder (such as `~/Library`) are hidden in the Files pane.
- The terminal uses SF Mono / Menlo instead of falling back to Courier New.
- The status bar no longer lists duplicate system volumes or counts loopback traffic, and hides the GPU figure it can't read on a Mac.
- After a scrollback restore, the prompt sits right under the restored history instead of below a screen of blank lines.
- Tooltips show Cmd shortcuts.

## Security

- **Clicking a crafted link could run a command on Windows.** ymux opened links through `cmd /c start`, which runs anything after an `&` in a URL. Links — from the terminal and the new Markdown preview — are now handed to the OS directly.
- **Dropped and pasted file paths are quoted for the pane's shell** (bash/zsh, fish, PowerShell, cmd). They used to be wrapped in double quotes, so a `$` or backtick in a file name was expanded. For shells ymux doesn't know (WSL, Nushell, custom), a path is only typed when it contains nothing that shell could interpret.
- The main window can no longer be navigated away from ymux's own page, and Markdown files are sanitised before they are rendered.

## Also

- On Windows the tray icon's menu follows the app language, and a second ymux instance still starts normally (it gets its own tray icon).
