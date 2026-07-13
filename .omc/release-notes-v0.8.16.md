# ymux v0.8.16

Real fix for the "first letter of words remains after scrolling" ghost users have been reporting on markdown files — diagnosed empirically this time, not speculatively patched.

## Fix

### 📄 yCode — drop empty syntect spans for markdown

The ghost was specific to `.md` files (not `.rs`, `.ts`, etc.) because syntect's markdown highlighter emits **zero-length styled spans** at scope boundaries:

- Heading end: `"# Hello World"` → `["#", " ", "Hello World", ""]` — trailing empty span
- Inline code close: `"... ``code``"` → `[..., "`", "code", "`", ""]` — trailing empty span
- Empty lines: `""` → `[""]` — single empty span

Each empty span has no character content but carries a style. ratatui's per-frame cell diff routes the style onto positions that don't correspond to real cells, then fails to clear them when the line's content changes under a scroll. Code files don't reproduce this because their highlighters don't emit empty boundary spans.

`Highlighter::highlight_range` now filters `t.is_empty()` after the trailing-newline strip, so the renderer only sees spans that actually paint a character. New regression test `markdown_highlight_drops_empty_spans` keeps this from creeping back in. The sum-of-chars assertion in that test confirms no real characters are removed.

This is the fix the speculative chain from v0.8.12 → v0.8.13 → v0.8.14 → v0.8.15 should have been from the start. Apologies for the noisy release sequence — locally probing syntect's output is what finally surfaced the root cause.

## Note on the older ambiguous-width ghost

If your markdown file still contains `•`, `—`, `─`, `│`, etc. and you're on a Korean-locale Windows with a CJK fallback font, those characters can still draw 2 cells wide in the terminal even though they're classified single-width. That's the limitation v0.8.13's release notes called out — a font issue, not a yCode bug. The fix in this release is unrelated and is independent of which font you use.

## Compatibility

Pure rendering fix. No config changes, no behavior changes outside the markdown highlight path. Safe drop-in over v0.8.15.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

Tests: `cargo test -p ycode` (64/64), `cargo clippy -p ycode -- -D warnings` (clean).

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.15...v0.8.16
