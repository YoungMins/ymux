# ymux v0.8.12

Patch release for yCode rendering on buffers that contain Korean (or other CJK) text.

## Fixes

### 🖥️ yCode — wide-char ghost cells when scrolling

Scrolling a buffer with Korean text (arrow keys, PageDown, or mouse wheel) left behind half-glyph ghosts of the previous line. Root cause: a CJK glyph occupies two terminal cells; when the next frame put narrower content into the left cell, ratatui's buffer diff would skip the right cell because nothing had changed *there*, leaving the old wide-char's right-half marker on screen.

The fix pre-fills the editor body — and the markdown preview area — with an explicit background block so every cell carries an attribute. The diff now has something concrete to scrub the stale half-cell against, and the ghost is gone. The same treatment applies to each highlighted span (syntect only emits foreground colors, so spans inherited a transparent background that was part of the bug).

### 🖥️ yCode — cursor drifting left on lines mixing Hangul + ASCII

The cursor column was being passed to the terminal as a *character* index, but terminals position cursors by *display cell*. CJK characters are two cells wide, so on a line like `안녕 hello` the cursor would land several cells to the left of where you actually typed. yCode now translates char column → display column via `unicode-width` before placing the cursor.

## Compatibility

No config or behavioural changes — pure rendering fix. Safe to drop in over v0.8.11.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.11...v0.8.12
