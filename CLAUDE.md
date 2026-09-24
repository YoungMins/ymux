# CLAUDE.md — yMux Development Guide

## Project Structure

```
ymux/
├── Cargo.toml              # Workspace root
├── src-tauri/              # Main Tauri app (ymux)
│   ├── Cargo.toml          # ymux package (desktop feature gate)
│   ├── tauri.conf.json     # Tauri config (version, bundle, CSP)
│   ├── tauri.macos.conf.json # macOS overlay (dmg/app targets, min OS)
│   ├── capabilities/       # Tauri 2 permission config
│   ├── wix/                # WiX fragments (PATH registration)
│   ├── icons/              # App icons (.ico, .png)
│   └── src/
│       ├── main.rs         # Entry point (desktop only)
│       ├── lib.rs          # Library crate (all modules)
│       ├── commands.rs     # Tauri IPC commands (desktop)
│       ├── agents.rs       # Agent tree registry state machine (pure, not desktop-gated)
│       ├── agent_scan.rs   # 2s process-tree scan for agent CLIs (matcher pure; scan loop desktop)
│       ├── agent_hooks.rs  # Install/uninstall Claude Code hooks in ~/.claude/settings.json (merge fns pure; file IO desktop)
│       ├── config/         # Config model + store
│       ├── pty/            # PTY session management
│       ├── shell/          # Shell detection (detect.rs)
│       ├── sysmonitor.rs   # System monitor (desktop)
│       ├── updater.rs      # Update checker (desktop)
│       ├── webview.rs      # Native browser (desktop, experimental)
│       └── ipc_server.rs   # IPC server (desktop); routes agent-hook events
├── src/                    # Frontend (TypeScript)
│   ├── main.ts             # App entry point
│   ├── platform.ts         # IS_MAC + Cmd/Ctrl modifier abstraction
│   ├── style.css           # All CSS
│   ├── types.ts            # TypeScript mirror of Rust models
│   ├── i18n/i18n.ts        # 13-language translations
│   ├── ipc/bridge.ts       # Tauri IPC wrappers
│   ├── filedock/           # Right-side file dock hosting a FilesPane (FileDock, cwdFollow, dockModel)
│   ├── files/              # Files pane (FilesPane, fileModel, preview, clipboard)
│   ├── editor/             # Editor pane (EditorPane, CodeMirror 6 setup, eol, theme)
│   ├── git/                # Git pane (GitPane, graphLanes, worktreeFlow)
│   ├── workspace/          # WorkspaceManager + WorkspaceBar + agentTree (workspace panel tree)
│   ├── terminal/           # TerminalPane + HotKeyBar + bottomAnchor (bottom-anchored prompt)
│   ├── browser/            # BrowserPane (iframe) + NativeBrowserPane
│   ├── layout/             # SplitContainer + LayoutTree + PaneGroup/tabs (pane tab groups)
│   ├── palette/            # Command Palette (Ctrl+Shift+P)
│   ├── help/               # Help overlay (?)
│   ├── hotkey/             # HotKeyManager modal (⚙)
│   ├── statusbar/          # System monitor status bar
│   └── update/             # Update banner
├── crates/
│   ├── ytheme/             # Shared theme library
│   ├── yipc/               # Host <-> `y` IPC (server, Hello/Event/Ack, client)
│   └── ypath/              # Path comparison keys (NFC + syntax-based case folding)
├── tools/
│   └── ylauncher/          # `y` — the Claude Code hook relay (the only sidecar)
├── scripts/
│   └── build-tools.mjs     # Build + stage the `y` sidecar
└── .github/workflows/
    └── release.yml          # CI: test + build + release
```

## Development Commands

```sh
pnpm install                 # Install frontend deps
pnpm tauri dev               # Run in dev mode (hot reload)
pnpm tauri build             # MSI on Windows, .app + .dmg on macOS
cargo test --workspace       # ⚠ Don't use on Linux — pulls GTK
cargo test -p ytheme -p yipc -p ypath -p ylauncher
cargo test --no-default-features --lib -p ymux
cargo check --no-default-features --lib --tests -p ymux  # Linux safe
cargo fmt --all              # Format entire workspace
cargo clippy --workspace -- -D warnings
npx tsc --noEmit             # TypeScript type check
```

## Critical Rules

### 1. Feature Gate: `desktop`

The `ymux` crate uses `#[cfg(feature = "desktop")]` for Tauri-dependent modules:
- `commands.rs`, `updater.rs`, `sysmonitor.rs`, `webview.rs`, `ipc_server.rs`

**Always verify:** `cargo check --no-default-features --lib --tests -p ymux` must pass on Linux.

### 2. PaneSpec Field Sync (THE #1 SOURCE OF BUGS)

When adding a new field to `PaneSpec`, you MUST update **ALL 4 PLACES**:

1. **Rust model** — `src-tauri/src/config/model.rs` → `PaneSpec` struct + all constructors
2. **TypeScript type** — `src/types.ts` → `PaneSpec` interface
3. **`nodeToSpec()`** and **`paneNode()`** — `src/layout/LayoutTree.ts` → manual field copies (both directions)
4. **`findAndMutatePane()`** — `src/layout/LayoutTree.ts` → snapshot + write-back

Missing any of these causes the field to silently disappear during save/load.
`LayoutTree.test.ts`'s `editor_file_path_survives_save_load` walks all three
TS copies for `file_path`; extend it (or clone it) for a new field.

### 3. TOML Serialization Gotcha

`Option<T>` fields inside `#[serde(tag = "kind")]` tagged enums **DO NOT round-trip through TOML**. The `toml` crate deserializes them as `None` even when the TOML file has the value.

**Workaround:** Use `String` with `#[serde(default)]` instead of `Option<String>`. Empty string = no value.

### 4. CI Sidecar Files

Tauri's build script validates `externalBin` paths even during `cargo check`. The CI workflow creates dummy empty files before the desktop check step. Today there is exactly one sidecar, `y` (package `ylauncher`, see rule 13). If you add or remove one, update:
- `src-tauri/tauri.conf.json` → `bundle.externalBin`
- `.github/workflows/release.yml` → dummy file creation loop
- `scripts/build-tools.mjs` → TOOLS array

### 5. Version Bump Checklist

Update ALL of these (they must match):
- `src-tauri/Cargo.toml` → `version`
- `src-tauri/tauri.conf.json` → `version`
- `package.json` → `version`
- `README.md` / `README.ko.md` / `README.ja.md` → badge URL
- Run `cargo check` to regenerate `Cargo.lock`

### 6. xterm.js Key Handling

`attachCustomKeyEventHandler` in `TerminalPane.ts` blocks certain keys from reaching xterm so they bubble to ymux's global handler. When adding a new Ctrl+Shift+X shortcut:
1. Add it to main.ts keydown handler
2. Add `k === "x"` to the handler's block list in TerminalPane
3. Add to Help overlay (`HelpOverlay.ts` SHORTCUTS array)
4. Add to Command Palette (`commands.ts` builtinCommands)
5. Add i18n key for the description
6. Add to README keyboard shortcut tables (3 files)

### 7. i18n

All user-visible strings go through `src/i18n/i18n.ts`. 13 languages. When adding a key:
```typescript
"category.keyName": {
    en: "English", ko: "한국어", ja: "日本語",
    zh: "中文", hi: "हिन्दी", es: "Español",
    fr: "Français", ar: "العربية", pt: "Português",
    ru: "Русский", tr: "Türkçe", de: "Deutsch", vi: "Tiếng Việt",
},
```

### 8. CONFIG_VERSION

Bump `CONFIG_VERSION` in `src-tauri/src/config/model.rs` when:
- Shell detection args change (forces re-detection)
- Existing field semantics change

Do NOT bump for additive fields with `#[serde(default)]` — they load transparently.

### 9. Vendored WiX template

`src-tauri/wix/main.wxs` is a **copy of Tauri's stock MSI template** (extracted from
`@tauri-apps/cli` 2.10.1), wired in via `bundle.windows.wix.template`. It carries exactly
one deviation, marked by a comment: `ApplicationStartMenuShortcut` has no `Icon` attribute.

Why: `Icon="ProductIcon"` writes `C:\Windows\Installer\{ProductCode}\ProductIcon` into the
`.lnk`. Tauri mints a new ProductCode per version, so a major upgrade deletes that folder —
and any user copy of the shortcut (the **taskbar pin**, which no installer rewrites) is left
pointing at a missing file and renders as a blank page icon. Without the attribute the shell
resolves the icon from the target exe instead, which survives upgrades.

**When bumping the Tauri CLI:** re-extract the stock template, re-apply the one-line removal,
and diff — a stale vendored template silently loses new installer features. Extract with:

```powershell
$bin = "node_modules\@tauri-apps\cli-win32-x64-msvc\cli.win32-x64-msvc.node"
$s = [System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($bin))
$st = $s.IndexOf('<?if $(sys.BUILDARCH)="x86"?>')
$e  = $s.IndexOf("</Wix>", $s.IndexOf("ApplicationStartMenuShortcut"))
[System.IO.File]::WriteAllText("stock-main.wxs", $s.Substring($st, ($e + 6) - $st))
```

Also note: `src-tauri/icons/icon.ico` must stay **multi-resolution with 32×32 first** —
`tauri-codegen` takes `entries()[0]` verbatim as the window icon, so a 256-only `.ico`
gives the window a 256×256 icon. Regenerate with `pnpm tauri icon src-tauri/icons/icon.png -o <tmp>`
and copy the resulting `icon.ico`.

### 10. Platform support: Windows + macOS (arm64)

Two shipping platforms, one codebase. What differs, and where:

| Concern | Windows | macOS |
|---------|---------|-------|
| Bundle | MSI (WiX, vendored template) | `.app` + `.dmg`, arm64 only |
| Bundle config | `tauri.conf.json` | `+ tauri.macos.conf.json` (auto-merged by Tauri) |
| Icon | `icons/icon.ico` | `icons/icon.icns` |
| Shells | cmd / PowerShell / pwsh / Git Bash / WSL | `$SHELL` + zsh / bash / fish |
| OSC 7 hook | PROMPT / `--rcfile` | zsh `ZDOTDIR` shim, bash `--rcfile` |
| CLI on PATH | MSI writes the install dir into PATH (for `ymux`) | nothing — no bundled CLI is meant to be typed |
| Signing | none needed | ad-hoc (`APPLE_SIGNING_IDENTITY: '-'`), not notarized |

**The zsh shim is the subtle part.** zsh gives an external launcher exactly one
injection point — `ZDOTDIR` — and it swaps out *all four* startup files at once.
So `shell/detect.rs` generates `<config>/ymux/zsh-init/{.zshenv,.zprofile,.zshrc,.zlogin}`,
each of which re-sources the user's real counterpart before handing control
back. Break that and users silently lose their aliases and `PATH`. It is
covered by `pty::session::tests::macos_shell_integration_reports_live_cwd`,
which spawns a real PTY and asserts a live cwd comes back — run it on macOS,
because Linux CI cannot compile it.

**Keyboard.** All shortcuts are written in the canonical `Ctrl+…` form and
translated at runtime by `src/platform.ts`. Never compare `ev.ctrlKey`
directly in new shortcut code — use `hasMod(ev)`, or macOS users lose
`Ctrl+C`/`Ctrl+F` to the app instead of the shell. Two bindings deliberately
stay on Ctrl (`Ctrl+Tab`, `Ctrl+Shift+Tab`) because macOS reserves `Cmd+Tab`.

**Verifying the Windows path from macOS.** `cargo check` does not link, so the
Windows-only `#[cfg(windows)]` code can still be compile-checked locally:

```sh
rustup target add x86_64-pc-windows-msvc
cargo check --target x86_64-pc-windows-msvc --no-default-features --lib -p ymux
```

**Sidecar triples.** `scripts/build-tools.mjs` stages the sidecars under a
target-triple suffix. If you pass `--target` to `tauri build`, set
`YMUX_TARGET_TRIPLE` to the same value or the bundler fails with a confusing
"sidecar not found".

### 11. `agent_tracking` is backend-authoritative — don't add it to `merge_layouts_from`

Every other `Config` setting added since rule 8's `CONFIG_VERSION` note must be
copied in `Config::merge_layouts_from` (see the memory note: a setting missing
there silently reverts to its default on every restart). `agent_tracking` is
the deliberate exception: it is flipped only by `set_agent_tracking`, which
also installs/uninstalls the Claude Code hooks as a side effect, so a stale
frontend save overwriting it out-of-band would desync the config from the
actual hook state on disk. If you add a new bool/enum setting, copy it in
`merge_layouts_from` like the rest — only mirror this exception if the setting
is similarly owned by a backend side effect, not just because it's convenient.

### 12. The Claude Code hook settings merge is marker-based — never reorder or drop foreign hooks

`agent_hooks::install_hooks` / `uninstall_hooks` rewrite the user's
`~/.claude/settings.json` in place. Every hook ymux owns carries the
`--ymux-agent-hook` marker in its command string; install only touches entries
carrying it (refreshing the `y` path) or appends a new group, and uninstall
only removes entries carrying it, via `retain`/`retain_mut` — never `remove`,
which under `serde_json`'s `preserve_order` is a `swap_remove` and would
reorder the user's own keys and hooks. Any hook or settings key without the
marker must come back byte-for-byte. If you touch this file, run the
`install_preserves_foreign_hooks_and_key_order` / `uninstall_restores_foreign_settings_exactly`
tests before anything else — they exist specifically to catch an edit that
silently reorders or eats someone else's hook.

### 13. `y` is the Claude Code hook relay — its name and path are load-bearing

The `y*` TUI tools are gone (they are GUI panes now), but the `y` binary
stays, shrunk to one job: `y agent-hook <agent>` reads a hook payload on stdin
and forwards it to ymux over yipc (`tools/ylauncher/src/agent_hook.rs`).
`agent_hooks::install_hooks` writes its **absolute path** into every tracking
user's `~/.claude/settings.json`, and Claude Code runs it on every hook event.
So:

- **Don't rename or move it.** A new name or location leaves every tracking
  user's hooks calling a missing executable until the upgraded ymux first
  launches and `install_hooks` refreshes the path (the marker-based refresh
  from rule 12). Keeping the path fixed is why the TUI cut-over could ship in
  one release (spec `docs/superpowers/specs/2026-09-24-gui-tool-panes.md`,
  Step 5 amendment).
- **It must stay invisible to Claude Code**: print nothing on stdout, always
  exit 0, return at once without `YMUX_PANE_ID`/`YMUX_IPC`, wait at most
  300 ms for the host's `Ack`. The `CARGO_BIN_EXE_y` tests in
  `tools/ylauncher/tests/` pin this. Anything other than `agent-hook` is a
  usage error (stderr, exit 2) — there are no launcher subcommands any more.
- yipc is reduced to what this needs: the server, `Hello`/`Event`/`Ack`,
  `AGENT_HOOK_KIND`, `IpcClient`. Traffic is tool → host only.

### 14. A tab shown after being hidden needs a refit *and* a viewport resync

`PaneGroup` keeps every tab's `TerminalPane` mounted (`display: none` via
`.pane--tab-hidden`) rather than destroying it, so switching tabs is instant
and scrollback/PTY state survives. But re-parenting/un-hiding an element
resets `.xterm-viewport`'s `scrollTop` to 0 behind xterm's back (see the
memory note on this), and its box may have resized while hidden. `PaneGroup.update()`
schedules `pane.scheduleFit()` on the next animation frame for exactly this
reason — a plain `fit()` without the viewport resync leaves the next wheel
notch jumping to the top of scrollback. Any new code path that shows a
previously-hidden pane element (not just the tab strip) needs the same
`scheduleFit()` call, not a raw `fit()`.

### 15. Never compare two paths with `==` — use `ypath`

Paths reach ymux from producers that spell them differently: a shell's OSC 7
payload, `git worktree list --porcelain` (forward slashes and git's own
drive-letter case, even on Windows), a TOML config file, the filesystem
itself. Byte equality between any two of those is
wrong in at least three ways, and each one shows up as "the file dock keeps
jumping back to row 0":

- **Composition.** macOS reports decomposed (NFD) filenames. The same `한글`
  directory is different bytes depending on which side produced it.
- **Case.** `C:\Repo` and `c:\repo` are one directory; `/srv/A` and `/srv/a`
  are two. Which rule applies is a property of the **path's syntax**, never
  of `cfg!(windows)` — a Windows ymux drives WSL and SSH shells, and folding
  a case-sensitive root silently merges distinct files.
- **Separators.** `\` is a legal POSIX filename character, so it may only be
  folded to `/` once the path has proved Windows semantics.

`ypath::same_path(a, b)` / `ypath::comparison_key(p)` encode all three.
The key is **lossy and for comparison only** — never open it, store it in the
config, or hand it to `git`; keep the raw string for that. Applied today in
`pty::osc7::CwdChange`, `git/mod.rs`'s worktree tests, and `fsx::same_file`
(where it sits *before* `canonicalize`, which still catches symlinks, 8.3
names and genuinely case-insensitive volumes but costs syscalls and cannot
answer for a path that no longer exists).

On the frontend the mirror is deliberately partial: `src/filedock/cwdFollow.ts`
dedupes on `normalize("NFC")` only, because the Rust side has already applied
the full rule before any cwd is emitted, and a second, differently-opinionated
case rule in TS is the one way to make the two layers disagree.

### 16. Every `#[tauri::command]` starts with its guard

ymux's commands have **no ACL**: `build.rs` has no `AppManifest`, so Tauri
never checks capabilities for app commands, and every page ymux loads can
`invoke` them — an `eb-*` embedded browser always, and on Windows even a
`browser` pane's iframe (WebView2 injects the invoke key into subframes).
Without a guard, a website can call `spawn_pane` or `write_pane` — RCE.

So the first statement of every command is one of:

- `guard_local(&webview, &request, "<name>")?` (`src-tauri/src/fspath.rs`)
  — label `main` **and** an `Origin` derived from config. The default.
- `guard_embedded_child(&webview, &registry, "<name>")?`
  (`embedded_browser.rs`) — only for the commands in
  `ipc_guard::EMBEDDED_CHILD_COMMANDS` (today `child_webview_focused`,
  `forward_keystroke`). Anything on that list is callable by any website;
  it must take its identity from the caller's label and validate every
  argument. Don't add to it.

Enforced by `ipc_guard::tests::every_registered_command_starts_with_a_guard`
(`src-tauri/src/ipc_guard.rs`), which parses `main.rs`'s `generate_handler!`
and each command body — it runs under `cargo test --no-default-features --lib -p ymux`.
Don't "fix" this by adding an `AppManifest` or putting ymux commands in a
capability file: that switches ACL enforcement on for every command at once.
`eb-*` webviews deliberately have no capability at all — Tauri injects
`invoke` into every webview regardless, so none is needed.

Two things `guard_local` cannot see, handled elsewhere: ymux's own page
framed inside a browser pane (refused by CSP `frame-ancestors 'none'` and
`src/bootGuard.ts`, which must stay `main.ts`'s first import), and what a
forwarded keystroke *means* (`ipc_guard::forwarded_shortcut_key` /
`src/browser/forwardedKeys.ts` — an exact table, key derived from code,
never close-pane). Anything that creates a PTY must not be triggerable from
a web page either, so split (Ctrl+Shift+H/V) and new tab (Ctrl+Shift+T) are
not forwardable.

## TDD / Testing

### Quick run

```sh
pnpm test              # Full suite: fmt + tsc + clippy + tests
# or individually:
bash scripts/test.sh
```

### Test count (Rust 382, 8 failing on Windows + frontend 574)

Measured 2026-09-24 on Windows with `cargo test -p ymux --lib`,
`cargo test -p ytheme -p yipc -p ypath -p ylauncher` and `npx vitest run`.

| Crate | Tests | What they cover |
|-------|-------|-----------------|
| ymux_lib | 349 (338 pass, 8 fail on Windows, 3 ignored; 314 without `desktop`) | Config model + TOML round-trip, PTY, OSC 7 (incl. `CwdChange` respelling dedupe), shell detect, macOS shell integration, updater, sysmonitor, git log/branch/worktree porcelain (non-ASCII + cross-source path comparison, real-git round-trip), filesystem + text-file commands (`fsx`, `fsops`, `textfile`: EOL/BOM round-trip), command guards (`ipc_guard`), agent registry (`agents.rs`), process-tree agent scan (`agent_scan.rs`), Claude Code hook settings merge (`agent_hooks.rs`) |
| ytheme | 6 | Theme TOML round-trip, hex parsing, defaults |
| yipc | 7 on Windows (more on Unix) | Protocol serialization, retired message types rejected, server/client, broken pipe |
| ypath | 9 | NFC folding, drive/UNC/verbatim/WSL case rules, POSIX case sensitivity, backslash as a POSIX filename character |
| ylauncher (`y`) | 11 (5 unit + 6 integration) | `agent-hook` payload packing, the silent no-env no-op, relay to a live server, usage errors (exit 2) for anything else |
| _frontend_ | 574 (43 files) | vitest: layout tree, pane tabs, agent tree model, file dock (`cwdFollow`, `dockModel`), files pane models, editor models (EOL, close guard, drafts, keymap, headless CM6), git pane models (graph lanes, keys), bottom-anchored prompt, IME, pane status, workspace reorder, drop paths, viewport sync, scrollback, platform shortcut mapping |

**The 8 `ymux_lib` failures are Windows-only and pre-existing**, all in
`pty::osc7::tests`: the OSC 7 parser correctly decodes a `file://` URI's path,
but the tests assert Unix-style forward-slash paths (`"/tmp"`, `"/home/alice"`)
while `Path`/`PathBuf` on Windows normalizes them to backslashes (`"\tmp"`).
Not something introduced by this branch — run the same suite on Linux/macOS to
get a clean pass, or fix the tests to compare with a platform-appropriate
separator if you touch `osc7.rs`.

Counts drift — re-derive with `cargo test -p <crate>` / `npx vitest run`
rather than trusting this table.

### TDD workflow for new features

1. **Write a failing test first** — in the relevant crate's `#[cfg(test)]` module
2. **Run it**: `cargo test -p <crate> -- <test_name>`
3. **Implement** until the test passes
4. **Run full suite**: `pnpm test`
5. **Commit**

### What to test when adding a PaneSpec field

```rust
// In src-tauri/src/config/model.rs tests:
#[test]
fn new_field_toml_roundtrip() {
    let mut config = Config::default();
    let ws = config.workspace_mut(1);
    if let LayoutNode::Pane(ref mut p) = ws.root {
        p.new_field = "value".to_string();
    }
    let toml_str = toml::to_string_pretty(&config).unwrap();
    let loaded: Config = toml::from_str(&toml_str).unwrap();
    assert_eq!(loaded.workspaces[0].panes()[0].new_field, "value");
}
```

Also update the `panespec_all_fields_roundtrip` test to include the new field.

## Release Process

```sh
git checkout main && git pull
git merge claude/windows-tmux-tool-mKhjy
git tag v0.8.4
git push origin v0.8.4
```

CI automatically:
1. Runs tests on Linux (fast fail)
2. Builds the MSI on Windows (with the `y` sidecar) **and creates the release** —
   it goes first precisely so exactly one job ever creates it
3. Builds the arm64 `.dmg` on macOS and uploads it onto that release
4. Rewrites the release body with install info + auto-generated notes

Then: GitHub → Releases → Edit draft → Publish.
