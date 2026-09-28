# ymux v0.13.2

Fixes for pasting and agent detection on macOS, and clickable paths in Korean, Japanese and Chinese output. This release includes everything from v0.13.1, which was built but not published.

## Fixes

- **Paths followed by a Korean, Japanese or Chinese particle are clickable.** These languages attach particles directly to the word before them, as in `.omc/release-notes.md에 있고`, so ymux treated `…md에` as the path. That file doesn't exist, so no link appeared. ymux now also tries the path without the trailing Hangul, kanji or kana and links whichever one exists. A file whose name really ends in those characters still opens. Paths open with Ctrl+click (Cmd+click on macOS).
- **macOS: Cmd+V pasted everything twice in a terminal.** The Edit menu's Paste and the terminal both handled the same paste. Now it pastes exactly once. That covers single-line text, multi-line text, and image paths pasted into Claude Code. Pasting while a Hangul syllable is still half-typed no longer garbles it. Right-click → Paste no longer shows macOS's floating "Paste" button.
- **macOS: Codex and other npm, nvm or Homebrew installs were missing from the "+" menu.** When opened from Finder, ymux only sees a minimal `PATH`, so it didn't find agents installed through npm. The launcher now also checks the `PATH` your login shell gets (the one Terminal.app uses), plus the nvm, fnm, Volta and pnpm folders. If an agent you just installed isn't listed, use **Rescan agents**.

## Upgrading on Windows

When you close its window, ymux keeps running in the tray. **Choose Quit from the tray icon before installing this update.**
