# ymux v0.8.11

Focused yCode update: a built-in markdown preview so `.md` files can be read in their rendered form without leaving the editor, plus the long-overdue removal of vim-style `:` commands.

## Highlights

### 📖 yCode — markdown preview (Alt+M)

Open any `.md` / `.markdown` file and hit **Alt+M** to toggle between the raw source and a styled preview rendered with `pulldown-cmark`. Preview mode is read-only — `↑↓ / PgUp / PgDn / Home / End` scroll, `Esc` or `Alt+M` returns to the source. The original cursor and scroll position are restored when you exit.

Rendered elements:
- Headings (H1–H6) with `#` markers in the teal accent
- **Bold**, *italic*, ~~strikethrough~~
- Inline `code` and fenced code blocks (with the language tag preserved)
- Ordered and unordered lists (incl. task list markers)
- Blockquotes with a `│` gutter
- Links rendered with the URL inline in muted text
- Horizontal rules

The title bar shows `[PREVIEW]` while the toggle is active so you always know which view you're in. Switching to a non-markdown file via the sidebar automatically drops preview mode.

> **Why Alt+M and not Ctrl+M?** Ctrl+M and the Enter key send the same byte (carriage return) in every terminal that doesn't speak the kitty keyboard protocol, so binding Ctrl+M would have made it impossible to insert a newline. Alt+M works everywhere.

### ✂️ yCode — `:` command mode removed

Typing `:` no longer drops the editor into a vim-style command bar. The few useful commands have direct shortcuts:

- **Ctrl+F** — find (still surfaces the input bar with the `find ` prefix)
- **Ctrl+G** — goto line (still surfaces the input bar with the `goto ` prefix)

If you typed `:q` or `:w` out of muscle memory before — use **Ctrl+Q** to quit and **Ctrl+S** to save instead.

## Compatibility

- Existing config files and themes are unchanged — no migration needed.
- Behavioural change: `:` is now a plain character in yCode and will be inserted into the buffer like any other key.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

Tests: `cargo test -p ycode` (63 / 63 pass), `cargo clippy -p ycode -- -D warnings` (clean).

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.10...v0.8.11
