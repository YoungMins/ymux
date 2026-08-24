# ymux v0.9.0

ymux runs on macOS now. Apple Silicon, a `.dmg`, and — the part that took the actual work — live `cwd` inheritance on zsh without touching a single one of your dotfiles.

## Features

### 🍎 A macOS build

`ymux_0.9.0_macos_aarch64.dmg` in the Assets below. Apple Silicon, macOS 11 or newer. Open it, drag ymux to Applications.

Two things the MSI does for Windows users that the DMG can't do for you:

**Gatekeeper.** The app is ad-hoc signed but **not notarized**, so the first launch is refused with a "damaged" or "unidentified developer" message. Clear the download quarantine flag once:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

**The companion tools aren't on your `PATH`.** The Windows installer registers its directory; a `.app` bundle has no equivalent step. `ymon`, `ydir`, `ycode`, `ygit` and `y` all ship inside the bundle, so add it yourself:

```sh
export PATH="/Applications/ymux.app/Contents/MacOS:$PATH"
```

### 🐚 Shell integration that leaves your dotfiles alone

This is the part that made macOS more than a packaging exercise.

Splitting a pane opens the new one in the directory the parent shell is *currently* in. That rides entirely on OSC 7, and macOS shells don't emit it. Ship only the bundle and the headline feature of the app quietly degrades to "opens wherever the pane started" — working, wrong, and silent about it.

**zsh** hands an external launcher exactly one injection point: `ZDOTDIR`. And it doesn't add a file — it *replaces* where zsh looks for all four of `.zshenv`, `.zprofile`, `.zshrc` and `.zlogin`. Point it somewhere naively and you've deleted the user's aliases, their `PATH` edits, and their prompt.

So ymux generates a shim of all four, each re-sourcing your real counterpart before handing control back, and installs the OSC 7 reporter as a `precmd` hook *after* your `.zshrc` has run — appended rather than assigned, so a prompt framework that owns `precmd_functions` wholesale (starship, Powerlevel10k) doesn't drop it. `ZDOTDIR` is handed back to your own value before you get a prompt.

**bash** gets the `--rcfile` treatment the Git Bash profile already used on Windows, minus the MSYS drive-letter rewriting. `--rcfile` is only honoured for non-login shells, so the shim sources `~/.bash_profile` itself rather than losing it.

**fish** needs nothing — it has emitted OSC 7 on every `PWD` change since 3.1.

**sh, dash, ksh** get no integration: there's no portable hook point. Those panes work fine, they just open splits in the startup directory.

The generated files live in `~/Library/Application Support/ymux/` (`zsh-init/`, `bash-init.sh`), are rewritten on every shell detection, and are safe to delete.

### ⌘ Cmd, not Ctrl

Every ymux shortcut moves to `Cmd` on macOS. Not for the sake of convention — because `Ctrl` is load-bearing inside a terminal. Leaving `Ctrl+F` and `Ctrl+V` bound to ymux would have taken forward-char and literal-next away from your shell, in an app whose entire job is hosting that shell.

| Windows | macOS |
|---------|-------|
| `Ctrl+Shift+H` / `V` / `W` / `Z` / `R` / `P` | `Cmd+Shift+…` |
| `Ctrl+F`, `Ctrl+V`, `Ctrl+Click` | `Cmd+F`, `Cmd+V`, `Cmd+Click` |
| `Ctrl+Alt+N` | `Cmd+Opt+N` |
| `Ctrl+Alt+1` … `9` | `Cmd+1` … `9` |
| `Ctrl+Tab`, `Ctrl+Shift+Tab` | **unchanged** |

Two deliberate exceptions. Pane cycling stays on `Ctrl+Tab` because macOS reserves `Cmd+Tab` for the application switcher — the OS eats it before any webview sees a keydown. Workspace switching drops the `Alt`: `Cmd+1..9` is the near-universal mac idiom, and the `Alt` only existed to dodge a Windows-level interception of `Ctrl+Shift+digit`.

The Help overlay and the command palette render whichever set actually applies, so the hints match the keys.

### 🔍 Shell detection on macOS

Your login shell is listed first and becomes the default. After that, `zsh` / `bash` / `fish` / `sh` from `PATH`, then from `/opt/homebrew/bin`, `/usr/local/bin`, `/bin` and `/usr/bin` — the absolute fallbacks matter because launchd hands a GUI-launched bundle a bare `PATH`.

Entries dedupe on the resolved path, so `$SHELL=/bin/zsh` doesn't produce a second `zsh`. A Homebrew zsh sitting alongside the system one gets disambiguated by path in its name. Shells are spawned as login shells so `/usr/libexec/path_helper` runs and your `PATH` matches Terminal.app's.

## Fixes

### 🔗 `Ctrl+Click` on a URL did nothing outside Windows

`open_url` shelled out to `xdg-open` on every non-Windows host. That command does not exist on macOS, and the failure was swallowed — the click simply did nothing. It now goes through the `opener` crate, which was already a dependency for the Settings "open config file" action.

## Under the hood

- **`tauri.macos.conf.json`** overlays the base config with the `app` / `dmg` targets and the minimum OS version. Tauri merges it only when building for macOS, so the Windows MSI path — WiX template, vendored fragment, WebView2 bootstrapper — is byte-identical to v0.8.28.
- **`ShellProfile` gained `env`** (serde-default, so v5 configs load untouched). It carries `ZDOTDIR`, and is applied *before* the pane's own `env` so a pane can still override it.
- **`src/platform.ts`** owns the Cmd/Ctrl translation. New shortcut code must go through `hasMod(ev)` rather than reading `ev.ctrlKey`, or macOS users lose that key to the app instead of the shell.
- **`icons/icon.icns`** was added for the bundle. Regenerating the icon set reproduced the existing `icon.ico` byte-for-byte, which confirms the 32×32-first rule from the vendored-WiX notes still holds.
- **`scripts/build-tools.mjs`** sets the executable bit on the staged sidecars (Tauri copies them into `Contents/MacOS` verbatim) and honours `YMUX_TARGET_TRIPLE`, so an explicit `--target` can't drift from the triple the sidecars are named for.
- **CI** grew a macOS job. `build-windows` creates the GitHub release and runs first for exactly that reason — two parallel `tauri-action` jobs would race to create the same one. `build-macos` uploads its DMG onto it, and a new `finalize` job writes the release body once both are in, still running if the macOS job fails so a Windows-only release is never left with placeholder text. Both jobs now also attach their installer as a workflow artifact, so a `workflow_dispatch` test build produces something downloadable.
- **The signing trap that broke the first v0.9.0 build:** Tauri decides whether to sign with a certificate by checking that `APPLE_CERTIFICATE` is *present*, not that it holds anything — and a missing GitHub secret interpolates to an empty string, which is still present. The bundler duly tried to import an empty certificate and died inside `security import`. The Apple variables are now exported only when they actually carry a value.

## Known limitations on macOS

Worth knowing before you file them as bugs:

- **Apple Silicon only.** No Intel build ships. Universal is possible — it needs the sidecars built for both triples and `lipo`'d — but isn't done.
- **Not notarized**, hence the `xattr` step above. Adding a Developer ID certificate to the repo's existing secret names switches CI to real signing and notarization with no workflow edits.
- **No GPU row in the status bar.** GPU utilization is read through the Windows D3DKMT kernel interface; on macOS the list is simply empty. CPU, memory, disk and network all work.
- **The browser panes are untested on macOS.** They're a `#[cfg(target_os = "windows")]`-heavy area — the native pane's child-window ownership is Windows-only — and this release didn't exercise them. Terminal panes are what got verified.

## Compatibility

`CONFIG_VERSION` goes 5 → 6, which **drops the cached shell profiles and re-detects on first launch — on Windows too.** That's the point of the bump: the unix detector now emits shell-integration arguments that a v5 cache knows nothing about. Layouts, workspaces, scrollback, notes, hotkeys and per-pane colours are all untouched.

Windows users get no keybinding changes and no installer changes. The one visible effect is that first-launch re-detection.

## Install

Windows — grab the MSI from the Assets below. macOS — grab the DMG, then run the `xattr` command from the top of these notes.

Or build from source (`tauri build` produces the installer for whatever host you run it on; each has to be built on its own platform):

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

Verified: `cargo fmt --check` clean, `npx tsc --noEmit` clean, `vitest` — 63 tests passing, clippy clean across the workspace, `cargo test` — 68 in `ymux_lib` including a new end-to-end test that drives a real PTY through each detected shell profile and asserts the OSC 7 reporter hands back a live `cwd`. The Windows-only code paths were compile-checked from macOS with `cargo check --target x86_64-pc-windows-msvc`. The DMG was built, mounted, signature-verified, and the app launched and spawned a PTY zsh.

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.28...v0.9.0
