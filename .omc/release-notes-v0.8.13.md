# ymux v0.8.13

Honest correction to the v0.8.12 yCode rendering fix. The previous "fix" targeted the wrong cause; this release rolls it back and patches the part actually within yCode's reach.

## What happened

v0.8.12 added explicit background-fill widgets to every yCode rendering path on the theory that the scroll ghost was a ratatui buffer-diff issue with already-wide CJK glyphs. After more testing, the same ghosts reproduce on v0.8.10 with the same markdown file, so this is not a v0.8.12 regression and the bg-fill changes were not solving the user's problem.

The real cause is a width mismatch: characters like `—`, `•`, `─`, `│`, `├`, `★` are classified as `unicode-width=1` by ratatui, but Korean-locale Windows Terminal / xterm.js render the glyphs as **2 cells** wide. ratatui's per-cell diff is consistent with its own width data, so when scrolling shifts content, the "extra" cell from the wide-rendered glyph is left untouched and persists as a ghost next to the new content.

## Fixes

### 📄 yCode — markdown preview no longer emits ambiguous-width markers

`markdown.rs` now renders list bullets as `*` (not `•`), horizontal rules as `-` (not `─`), and blockquote prefixes as `| ` (not `│ `). The preview view stops introducing the offending glyphs, so its own rendering stays clean regardless of terminal font.

### ⏪ Roll back the v0.8.12 background-fill churn

`draw_editor` and `draw_markdown_preview` go back to their v0.8.11 form: no `Block` bg fill, no per-span `.bg()`, no editor-body bg on the Paragraph. The `visual_column` cursor positioning fix from v0.8.12 is kept — that one's still useful for CJK lines and doesn't touch the hot rendering path.

### Still outside yCode's reach: source-view ghosts on user-typed ambiguous chars

If your `.md` file contains `—`, `•`, `─`, `│`, etc. typed by you, the source view will still show scroll ghosts on Korean-locale terminals with East Asian fonts. yCode renders what's in the file; it can't substitute the user's content. Two workarounds:

- **Change the ycode terminal font** to a Western monospace (Consolas, JetBrains Mono, DejaVu Sans Mono, Cascadia Mono) that renders these glyphs as single cells. This is the quickest fix.
- **A future ymux frontend change** to configure xterm.js's unicode handling and font metrics. Tracked separately.

## Compatibility

No config or behavioral changes. Drop-in replacement for v0.8.12. The cursor positioning improvement for Hangul+ASCII lines remains.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

Tests: `cargo test -p ycode` (63/63), `cargo clippy -p ycode -- -D warnings` (clean).

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.12...v0.8.13
