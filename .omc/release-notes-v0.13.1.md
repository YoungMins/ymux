# ymux v0.13.1

Two macOS fixes.

## Fixes

- **Cmd+V pasted everything twice in a terminal.** The Edit menu's Paste and the terminal both handled the same paste. Now it lands exactly once — single-line, multi-line, and image paths pasted into Claude Code alike — and pasting in the middle of a half-typed Hangul syllable no longer garbles it. Right-click → Paste no longer pops up macOS's floating "Paste" button either.
- **Codex (and other npm / nvm / Homebrew installs) missing from the "+" menu.** Opened from Finder, ymux only sees a minimal `PATH`, so agents installed through npm weren't found. The launcher now also looks at the same `PATH` your login shell gets — the one Terminal.app uses — plus nvm, fnm, Volta and pnpm folders. If something you just installed isn't listed, use **Rescan agents**.

Windows behaviour is unchanged.

## Upgrading on Windows

ymux keeps running in the tray when you close its window, so **choose Quit from the tray icon before installing this update.**
