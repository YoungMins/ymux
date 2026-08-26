# ymux v0.9.2

The bundled y* tools now work from a pane on macOS without you configuring anything.

## Fixes

### 🧰 `ydir`, `ymon`, `ycode`, `ygit` and `y` were "command not found" on macOS

They ship inside the app bundle at `Contents/MacOS` — but nothing ever told the shell that. On Windows the MSI writes the install directory into the system `PATH`; a macOS `.app` has no equivalent step and cannot register anything. So the tools were there the whole time, a few directories away, and invisible.

v0.9.0's answer was a line in the README asking you to paste an `export` into your dotfiles. That is really the app declining to solve a problem it is uniquely placed to solve: it knows exactly where its own sidecars live.

ymux now prepends its own directory to the `PATH` of every pane it opens, alongside the `YMUX_IPC` injection that was already there. Nothing to configure.

- **Prepended, not appended**, so a bundled tool wins over an older copy left on `PATH` by a previous install.
- **Skipped when already present**, so `PATH` doesn't grow a duplicate entry for anyone who added the export themselves.

One symptom worth naming, because it made the failure confusing: `ylauncher` already had a fallback that looks next to its own executable, so **`y dir` worked while `ydir` did not.**

## Under the hood

Two things would each have quietly made this a no-op, so both were measured rather than assumed:

- macOS login shells run `/usr/libexec/path_helper` from `/etc/zprofile`, which rebuilds `PATH` from `/etc/paths` and `/etc/paths.d`. It preserves pre-existing entries, moving them after the system ones — so the injected directory survives the rebuild.
- `current_exe()` inside a bundle resolves to `Contents/MacOS/ymux`, whose parent is the directory holding the sidecars. Confirmed by building a probe against the same code path, copying it into a real bundle, and running it with a launchd-level `PATH`: all five tools resolve.

The `PATH`-building helper is a pure function with unit tests covering prepend order, the already-present case, and an empty inherited `PATH`.

## Compatibility

Drop-in over v0.9.1. No config migration, no schema change, no keybinding changes. Windows is unaffected in practice — the installer already puts that directory on the system `PATH`, and the injection skips it as already present.

README (EN/KO/JA) now states that the tools work inside ymux out of the box, and keeps the `export` line only for using them from **other** terminals — Terminal.app, iTerm, an editor's built-in shell:

```sh
export PATH="/Applications/ymux.app/Contents/MacOS:$PATH"
```

## Install

Windows — the MSI from the Assets below. macOS — the DMG, then clear the quarantine flag once, since the app still isn't notarized:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

Verified: `cargo fmt --check`, `tsc --noEmit`, clippy across the workspace, `vitest` — 68 tests, `cargo test` — 73 in `ymux_lib`.

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.9.1...v0.9.2
