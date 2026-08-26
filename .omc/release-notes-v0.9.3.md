# ymux v0.9.3

A one-line regression fix: `-`, `=`, `0` and `+` are typable again.

## Fixes

### ⌨️ Typing `-`, `=`, `0` or `+` did nothing

Introduced in v0.9.1 along with the font-size feature. The rule that keeps xterm's hands off `Ctrl`/`Cmd` + `+` / `-` / `0` was written *outside* the modifier check, so it matched those physical keys whether or not a modifier was held. A bare `-`, `=`, `0` or `+` — and their numpad twins — never reached the terminal at all.

This affected **every platform**, not just macOS. The rule now lives inside the modifier block where it always belonged.

## Known issue: Hangul (and other IME) input splits into jamo

Not fixed here, and worth being straight about: typing 한 in a pane produces ㅎㅏㄴ. The same applies to Kana and any other composed input.

This is an [upstream xterm.js issue on WebKit](https://github.com/xtermjs/xterm.js/issues/3575). xterm drives IME input through a hidden `<textarea>` that it repositions and clears as the terminal renders — and on WebKit, which is what a macOS `.app` runs, that resets the composition context, so each jamo commits on its own. Windows is unaffected because WebView2 is Chromium.

It is not something ymux can paper over from the outside: the textarea belongs to xterm, and the reset happens on every render. The realistic route is xterm 6.x, which this project has not moved to yet — that is a major upgrade and does not belong in a patch release.

If you need Korean in a pane today, composing it elsewhere and pasting with `Cmd+V` works.

## Compatibility

Drop-in over v0.9.2. No config migration, no schema change, no keybinding changes.

## Install

Windows — the MSI from the Assets below. macOS — the DMG, then clear the quarantine flag once, since the app still isn't notarized:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

Verified: `cargo fmt --check`, `tsc --noEmit`, clippy across the workspace, `vitest` — 68 tests, `cargo test` — 73 in `ymux_lib`.

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.9.2...v0.9.3
