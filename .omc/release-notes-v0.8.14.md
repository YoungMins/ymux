# ymux v0.8.14

Upstream fix for the scroll-ghost issue that v0.8.13's release notes flagged as "outside yCode's reach" — turns out it's a one-line xterm.js option plus loading the WebGL renderer.

## Fix

### 🎨 xterm.js terminal — no more ghost half-glyphs on Korean-locale Windows

Characters like `•`, `—`, `─`, `│`, `★` are classified `unicode-width = 1` by xterm.js and by every TUI tool we ship. But Korean-locale Windows draws their glyphs **2 cells wide** by falling through Cascadia Code to a system CJK font. The wide glyph overflows into the next cell and persists there after a scroll, leaving the "한 축씩 옆으로 제자리 그대로 남음" ghost effect users have been reporting.

xterm.js has `rescaleOverlappingGlyphs` for exactly this case — it squishes the oversized glyph back into its declared cell — but the flag is a no-op under the DOM renderer ymux was implicitly using. This release switches to the **WebGL renderer** (the `@xterm/addon-webgl` package was already in our deps, just never loaded) and turns the flag on.

Side benefit: WebGL is faster than DOM rendering for any TUI with frequent redraws (vim, top, yCode itself).

Also prepended **Cascadia Mono** to the font stack — strict monospace variant that ships alongside Cascadia Code with Windows Terminal, more reliable than Cascadia Code's ligature-enabled metrics for box-drawing chars.

## Compatibility

Affects every terminal pane, not just yCode. Plain shells, vim, top, etc. will all render the affected characters correctly now. No configuration changes required.

If WebGL initialization fails on your machine (very rare on Windows; ANGLE is universally available), the addon's `onContextLoss` handler disposes itself and xterm.js falls back to the DOM renderer — same behaviour as before this release, minus the rescaling.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.13...v0.8.14
