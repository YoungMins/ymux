# ymux v0.8.15

Reverts v0.8.14's speculative xterm.js renderer change. v0.8.14 was tagged without local verification and introduced a layout regression. This release returns the terminal to its v0.8.13 behavior and folds in install-hygiene fixes the user discovered while validating the revert.

## Reverted

### ↩️ v0.8.14 xterm.js WebGL renderer switch + `rescaleOverlappingGlyphs`

v0.8.14 loaded `@xterm/addon-webgl` and turned on `rescaleOverlappingGlyphs` on the theory that it would fix the long-standing ambiguous-width scroll ghost in yCode. In practice it left the scroll ghost in place (still triggered by the user's fallback fonts) **and** introduced new symptoms:

- Line-number gutter shifted one cell to the left — tens digit clipped off-screen.
- Body text's first character(s) truncated after the first mouse-wheel scroll.
- Repeated rendering ghosts on continuous back-and-forth scrolling.

`TerminalPane.ts` is back to its v0.8.13 form: DOM renderer, original `Cascadia Code, Consolas, ...` font stack, no rescale flag.

## Known limitation (still)

`•`, `—`, `–`, `─`, `│`, `├`, `★` and similar **ambiguous-width** characters are classified `unicode-width = 1` but Korean-locale Windows draws their glyphs **2 cells wide** via the system's CJK fallback font. ratatui and xterm.js both assume 1-cell, so the wide glyph overflows and persists as a "ghost" in the adjacent cell after scrolling. This is the same effect users saw in v0.8.10 / v0.8.11 / v0.8.13 — it is **not** a yCode bug.

Practical mitigation: change the ycode terminal font to a Western monospace with full glyph coverage (Consolas, JetBrains Mono, Cascadia Mono, DejaVu Sans Mono). A future ymux release may add an in-app font picker; that's a separate piece of work.

## Install hygiene

The user hit `Cannot find module '@tauri-apps/cli-win32-x64-msvc'` while validating this release locally — the known npm/pnpm optional-dependencies bug on Windows ([npm/cli#4828](https://github.com/npm/cli/issues/4828)). Two changes shipped to make fresh installs reliable:

- `.npmrc` with `node-linker=hoisted` so pnpm produces a flat `node_modules` and Windows resolves Tauri's native CLI binding.
- `@tauri-apps/cli-win32-x64-msvc` listed explicitly in `devDependencies` so the platform binding isn't optional-only.

If you've cloned ymux before and hit this error, `Remove-Item -Recurse -Force node_modules` followed by `pnpm install` after pulling this commit should sort it out.

## Compatibility

Pure revert + install fixes. No yCode source changes since v0.8.13. Existing configs and themes load unchanged.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.14...v0.8.15
