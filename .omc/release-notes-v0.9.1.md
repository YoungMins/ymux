# ymux v0.9.1

Three bugs the first Mac build shipped with, all of which failed silently, plus the terminal font size you can finally change.

## Fixes

### 🎨 Every pane rendered in plain white

Nothing was setting `TERM` for the child process. `portable-pty` doesn't, ymux didn't, and a GUI-launched `.app` inherits launchd's environment — which has no `TERM` at all. Measured through a real PTY it came back empty:

```
TERM=[] COLORTERM=[]
```

With no `TERM`, zsh, `ls`, `git`, `grep` and every prompt framework conclude they aren't talking to a terminal and turn colour off. Nothing was broken about the rendering; the shells were never emitting colour in the first place.

Panes now get `TERM=xterm-256color` and `COLORTERM=truecolor`, set before the shell profile's own environment so either can still override them.

### ✏️ Renaming a pane or workspace did nothing

WKWebView only shows a JavaScript dialog if the host implements the matching `WKUIDelegate` method — and wry implements none of them. There's no error and no warning: `prompt()` just returns `null` and `confirm()` returns `false`. WebView2 provides these natively, which is why it never surfaced on Windows.

Eight call sites were affected, not the two that were visible:

- Pane rename (`Cmd+Shift+R`, and the palette entry)
- Workspace rename (double-click, and the palette entry)
- The worktree branch-name prompt
- Three confirmations — including **worktree removal, which was silently answering "no"** and leaving directories behind on disk

All of them now use an in-app dialog rendered as ordinary DOM, so it behaves the same on both platforms. It restores focus to whatever had it — otherwise the terminal stays blurred after a rename and your next keystroke goes nowhere — and swallows keydown in the capture phase, so `Cmd+Shift+W` can't close the pane behind the dialog you're typing in.

### 📂 Panes reopened at `/` instead of the last directory

Two causes stacked. The saved shell was `sh`, which had no OSC 7 integration, so no working directory was ever recorded for it — and with none recorded the shell inherited ymux's own cwd, which for a `.app` opened from Finder is the filesystem root.

POSIX shells read `$ENV` on interactive startup, the one hook point `sh` / `dash` / `ksh` all share, so they get an integration now too. And a pane with no usable cwd — never recorded, or pointing at a directory since deleted — falls back to the home directory rather than to whatever the OS handed the app.

## Features

### 🔠 Terminal font size

It was `fontSize: 13` hardcoded, with no setting and no shortcut.

| | Windows | macOS |
|---|---------|-------|
| Increase / decrease | `Ctrl++` / `Ctrl+-` | `Cmd++` / `Cmd+-` |
| Reset to 13 | `Ctrl+0` | `Cmd+0` |

Also a number input under **Settings → General**, and three command-palette entries. The size is global rather than per-pane — it's a legibility preference about your display, not a property of one shell — and it persists across restarts.

It applies to panes in *every* workspace, not just the visible one: panes stay alive in the background here, so touching only the foreground would leave the rest on the old size. Each pane re-fits afterwards, which is the load-bearing half — xterm keeps its row/column count when the glyph box changes, so without a re-fit the PTY is told the wrong size and long lines wrap in the wrong column until you resize the window.

The range is clamped to 6–40. `fit()` divides the container by the glyph box, so a size near zero produces an absurd column count and a huge one leaves no rows at all — and a held-down `Ctrl+-` is one keystroke away from either.

## Under the hood

- `Config.font_size` is additive with a serde default, so no `CONFIG_VERSION` bump — verified by running the built app against a config with no such key and watching it load as 13 rather than 0. `merge_layouts_from` copies it; omitting that is exactly the bug fixed in v0.8.28, and the test pinning that behaviour now covers the new field.
- `CONFIG_VERSION` 7 (from 6) for the shell change: a v6 cache holds an `sh` profile with an empty `env`, so those panes would have kept reporting no cwd until a re-detect. Cached shell profiles are dropped and re-detected on first launch; layouts, workspaces, scrollback and notes are untouched.
- The shell-integration test now asserts that *every* detected profile carries a hook, rather than skipping the ones that don't. A profile that quietly loses its integration is precisely how the `sh` case went unnoticed.
- Added a TOML round-trip test for `cwd` and `title` on panes nested inside splits — the shape that actually occurs once you split, and the one the tagged-enum caveat in `CLAUDE.md` warns about. They round-trip correctly; the persistence layer was never the problem here.

## Compatibility

Drop-in over v0.9.0 on both platforms. Shell profiles are re-detected once on first launch (Windows included) — that's the `CONFIG_VERSION` bump doing its job, and it's the only visible change for Windows users. No keybinding changes beyond the three new font-size bindings.

## Install

Windows — the MSI from the Assets below. macOS — the DMG, then clear the quarantine flag once, since the app still isn't notarized:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

Verified: `cargo fmt --check`, `tsc --noEmit`, clippy across the workspace, `vitest` — 68 tests, `cargo test` — 73 in `ymux_lib`. The `TERM` injection and the home-directory fallback both have regression tests that drive a real PTY. The built `.app` was run from `/` with `TERM` unset to reproduce the Finder environment: `lsof` confirms ymux itself sits at `/` while its child shell lands in `$HOME`.

The dialogs and the font-size shortcuts are GUI surfaces and were not machine-verified.

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.9.0...v0.9.1
