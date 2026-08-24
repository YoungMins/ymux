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
│       ├── config/         # Config model + store
│       ├── pty/            # PTY session management
│       ├── shell/          # Shell detection (detect.rs)
│       ├── sysmonitor.rs   # System monitor (desktop)
│       ├── updater.rs      # Update checker (desktop)
│       ├── webview.rs      # Native browser (desktop, experimental)
│       └── ipc_server.rs   # IPC server (desktop)
├── src/                    # Frontend (TypeScript)
│   ├── main.ts             # App entry point
│   ├── platform.ts         # IS_MAC + Cmd/Ctrl modifier abstraction
│   ├── style.css           # All CSS
│   ├── types.ts            # TypeScript mirror of Rust models
│   ├── i18n/i18n.ts        # 13-language translations
│   ├── ipc/bridge.ts       # Tauri IPC wrappers
│   ├── workspace/          # WorkspaceManager + WorkspaceBar
│   ├── terminal/           # TerminalPane + HotKeyBar
│   ├── browser/            # BrowserPane (iframe) + NativeBrowserPane
│   ├── layout/             # SplitContainer + LayoutTree
│   ├── palette/            # Command Palette (Ctrl+Shift+P)
│   ├── help/               # Help overlay (?)
│   ├── hotkey/             # HotKeyManager modal (⚙)
│   ├── statusbar/          # System monitor status bar
│   └── update/             # Update banner
├── crates/
│   ├── ytheme/             # Shared theme library
│   └── yipc/               # Inter-tool IPC protocol
├── tools/
│   ├── ymon/               # System monitor TUI
│   ├── ydir/               # File manager TUI
│   ├── ycode/              # Code editor TUI
│   └── ylauncher/          # `y` launcher CLI
├── scripts/
│   └── build-tools.mjs     # Build + stage sidecar binaries
└── .github/workflows/
    └── release.yml          # CI: test + build + release
```

## Development Commands

```sh
pnpm install                 # Install frontend deps
pnpm tauri dev               # Run in dev mode (hot reload)
pnpm tauri build             # MSI on Windows, .app + .dmg on macOS
cargo test --workspace       # ⚠ Don't use on Linux — pulls GTK
cargo test -p ytheme -p yipc -p ymon -p ydir -p ycode -p ylauncher
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
3. **`nodeToSpec()`** — `src/layout/LayoutTree.ts:56` → manual field copy
4. **`findAndMutatePane()`** — `src/workspace/WorkspaceManager.ts:603` → snapshot + write-back

Missing any of these causes the field to silently disappear during save/load.

### 3. TOML Serialization Gotcha

`Option<T>` fields inside `#[serde(tag = "kind")]` tagged enums **DO NOT round-trip through TOML**. The `toml` crate deserializes them as `None` even when the TOML file has the value.

**Workaround:** Use `String` with `#[serde(default)]` instead of `Option<String>`. Empty string = no value.

### 4. CI Sidecar Files

Tauri's build script validates `externalBin` paths even during `cargo check`. The CI workflow creates dummy empty files before the desktop check step. If you add new sidecar binaries, update:
- `src-tauri/tauri.conf.json` → `bundle.externalBin`
- `.github/workflows/release.yml` → dummy file creation loop
- `scripts/build-tools.mjs` → TOOLS array

### 5. Version Bump Checklist

Update ALL of these (they must match):
- `src-tauri/Cargo.toml` → `version`
- `src-tauri/tauri.conf.json` → `version`
- `package.json` → `version`
- `crates/yversion/src/lib.rs` → `VERSION` const (footer of ymon/ydir/ycode/ygit reads this)
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
| CLI on PATH | MSI writes the install dir into PATH | documented `~/.zshrc` export |
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

## TDD / Testing

### Quick run

```sh
pnpm test              # Full suite: fmt + tsc + clippy + tests
# or individually:
bash scripts/test.sh
```

### Test count (Rust 188 + frontend 63)

| Crate | Tests | What they cover |
|-------|-------|-----------------|
| ymux_lib | 68 | Config model + TOML round-trip, PTY, OSC 7, shell detect, macOS shell integration, updater, sysmonitor |
| ytheme | 7 | Theme TOML round-trip, hex parsing, defaults |
| yipc | 10 | Protocol serialization, server/client, multi-client, broken pipe |
| ymon | 11 | App state, tab cycling, scroll, memory values, process sort |
| ydir | 19 | File listing, navigation, copy/paste/delete, hidden, exec detection, run dialog |
| ycode | 69 | Buffer ops, undo/redo, cursor, commands, CJK, exit dialog |
| ylauncher | 4 | Tool discovery, PATH scanning |
| _frontend_ | 63 | vitest: layout tree, pane status, workspace reorder, drop paths, viewport sync, scrollback, platform shortcut mapping |

`ygit` has no tests yet. Counts drift — re-derive with
`cargo test -p <crate>` rather than trusting this table.

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
2. Builds the MSI on Windows (with sidecar tools) **and creates the release** —
   it goes first precisely so exactly one job ever creates it
3. Builds the arm64 `.dmg` on macOS and uploads it onto that release
4. Rewrites the release body with install info + auto-generated notes

Then: GitHub → Releases → Edit draft → Publish.
