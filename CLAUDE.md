# CLAUDE.md — yMux Development Guide

## Project Structure

```
ymux/
├── Cargo.toml              # Workspace root
├── src-tauri/              # Main Tauri app (ymux)
│   ├── Cargo.toml          # ymux package (desktop feature gate)
│   ├── tauri.conf.json     # Tauri config (version, bundle, CSP)
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
pnpm tauri build             # Build MSI installer (Windows only)
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

## Testing

| Crate | Tests | What they cover |
|-------|-------|-----------------|
| ymux_lib | 43 | Config model, PTY, OSC 7, shell detect, updater, sysmonitor |
| ytheme | 7 | Theme TOML round-trip, hex parsing, defaults |
| yipc | 10 | Protocol serialization, server/client, multi-client, broken pipe |
| ymon | 11 | App state, tab cycling, scroll, memory values, process sort |
| ydir | 10 | File listing, navigation, copy/paste/delete, hidden files |
| ycode | 32 | Buffer ops, undo/redo, cursor movement, commands, CJK support |
| ylauncher | 4 | Tool discovery, PATH scanning |

Run all: `cargo test -p ytheme -p yipc -p ymon -p ydir -p ycode -p ylauncher && cargo test --no-default-features --lib -p ymux`

## Release Process

```sh
git checkout main && git pull
git merge claude/windows-tmux-tool-mKhjy
git tag v0.8.4
git push origin v0.8.4
```

CI automatically:
1. Runs tests on Linux (fast fail)
2. Builds MSI on Windows (with sidecar tools)
3. Creates Draft release with auto-generated notes
4. Attach MSI artifact

Then: GitHub → Releases → Edit draft → Publish.
