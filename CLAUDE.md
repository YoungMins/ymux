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
│       ├── main.rs             # Entry point (desktop only)
│       ├── lib.rs              # Library crate (all modules)
│       ├── commands.rs         # Tauri IPC commands (desktop)
│       ├── agents.rs           # Agent tree registry state machine (pure, not desktop-gated)
│       ├── agent_scan.rs       # 2s process-tree scan for agent CLIs (matcher pure; scan loop desktop)
│       ├── agent_scan_disk.rs  # Disk-scan fallback for session resume: finds an agent's transcript by cwd match (pure)
│       ├── agent_sessions.rs   # Resumable agent sessions: resume argv/command, freshness window (pure)
│       ├── agent_hooks.rs      # Install/uninstall Claude Code http hooks in ~/.claude/settings.json (merge fns pure; file IO desktop)
│       ├── hook_http.rs        # Loopback receiver for those hooks: auth, parsing, port choice, listener (pure std)
│       ├── config/             # Config model + store
│       ├── drafts.rs           # Editor pane crash-safety drafts, debounced by pane id (pure)
│       ├── error.rs            # Crate-wide YmuxError / YmuxResult
│       ├── fspath.rs           # Resolve + open a path lifted from terminal output; reveal-vs-run policy (pure)
│       ├── fsops.rs            # Filesystem #[tauri::command]s: list/create/rename/copy/move/delete (desktop)
│       ├── fsx.rs              # Pure decisions behind the Files pane: listing, sort, binary sniff, same-file (pure)
│       ├── git/                # Git pane backend: log/branch/worktree porcelain parsing + commands
│       ├── ipc_guard.rs        # Per-command origin/label guard (rule 16); no page can invoke a command unguarded (pure)
│       ├── clipboard_image.rs  # Read a pasted image off the OS clipboard directly (desktop)
│       ├── paste_images.rs     # Save + time-prune pasted clipboard images (pure)
│       ├── pty/                # PTY session management
│       ├── scrollback.rs       # Persist/restore per-pane terminal scrollback (pure)
│       ├── shell/              # Shell detection (detect.rs)
│       ├── textfile.rs         # EOL/BOM/encoding-safe text read/write (pure)
│       ├── sysmonitor.rs       # System monitor (desktop)
│       ├── updater.rs          # Update checker (desktop)
│       ├── webview.rs          # Native browser (desktop, experimental)
│       ├── embedded_browser.rs # Child-webview browser panes via Window::add_child (desktop)
│       └── settings.rs         # Settings panel commands: theme load/save, open config dir (desktop)
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
│   ├── terminal/           # TerminalPane + HotKeyBar + bottomAnchor (bottom-anchored prompt), pathLinks (clickable paths)
│   ├── browser/            # BrowserPane (iframe) + NativeBrowserPane
│   ├── layout/             # SplitContainer + LayoutTree + PaneGroup/tabs (pane tab groups)
│   ├── palette/            # Command Palette (Ctrl+Shift+P)
│   ├── menu/               # Terminal right-click context menu (ContextMenu.ts)
│   ├── notes/              # Per-workspace notes overlay (NotesOverlay.ts)
│   ├── settings/           # Settings panel (⚙): general, syntax colors, shortcuts (shortcutList.ts), config files
│   ├── hotkey/             # HotKeyManager modal (⚙)
│   ├── statusbar/          # System monitor status bar
│   ├── ui/                 # Small shared UI primitives (Dialog.ts)
│   ├── util/               # Small shared utilities (beep.ts)
│   └── update/             # Update banner
├── crates/
│   ├── ytheme/             # Shared theme library
│   └── ypath/              # Path comparison keys (NFC + syntax-based case folding)
├── scripts/
│   └── test.sh             # fmt + tsc + vitest + clippy + tests (Linux-safe)
└── .github/workflows/
    └── release.yml          # CI: test + build + release
```

## Development Commands

```sh
pnpm install                 # Install frontend deps
pnpm tauri dev               # Run in dev mode (hot reload)
pnpm tauri build             # MSI on Windows, .app + .dmg on macOS
cargo test --workspace       # ⚠ Don't use on Linux — pulls GTK
cargo test -p ytheme -p ypath
cargo test --no-default-features --lib -p ymux
cargo check --no-default-features --lib --tests -p ymux  # Linux safe
cargo fmt --all              # Format entire workspace
cargo clippy --workspace -- -D warnings
npx tsc --noEmit             # TypeScript type check
```

## Critical Rules

### 1. Feature Gate: `desktop`

The `ymux` crate uses `#[cfg(feature = "desktop")]` for Tauri-dependent modules:
- `commands.rs`, `updater.rs`, `sysmonitor.rs`, `webview.rs`,
  `fsops.rs`, `clipboard_image.rs`, `embedded_browser.rs`, `settings.rs`

`hook_http.rs` is deliberately *not* gated: it is pure `std`, so its auth
decision and a real-socket round trip run on Linux CI.

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

### 4. No sidecar binaries

ymux bundles no `externalBin`: the last sidecar, the `y` hook relay, was
replaced by Claude Code http hooks (rule 13), and `scripts/build-tools.mjs`,
the CI dummy-sidecar step and `YMUX_TARGET_TRIPLE` went with it. If you ever
add one back, remember that Tauri's build script validates `externalBin` paths
even during `cargo check`/`cargo test` (CI then needs dummy files before the
desktop check), that the bundler looks for `<name>-<target-triple>[.exe]`
(so a `--target` build needs a matching staging triple), and that anything
written into another tool's config by absolute path is stranded by a rename.
Prefer an in-process listener or a GUI pane.

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
3. Add a row to `src/settings/shortcutList.ts`'s `SHORTCUTS` array (rendered
   by the Settings panel's Shortcuts section, `src/settings/SettingsOverlay.ts`).
   `shortcutList.test.ts` re-derives the `key ===`/`ev.code ===` literals
   main.ts's keydown handler matches on and fails if one has no row here —
   read its header comment for exactly what it does and doesn't catch
   (`src/help/HelpOverlay.ts`, the old standalone reference this table used
   to live in, is deleted — nothing has mounted it since `SettingsOverlay.ts`
   replaced its `?` button)
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

### 11. `agent_tracking` / `agent_hook_port` are backend-authoritative — don't add them to `merge_layouts_from`

Every other `Config` setting added since rule 8's `CONFIG_VERSION` note must be
copied in `Config::merge_layouts_from` (see the memory note: a setting missing
there silently reverts to its default on every restart). `agent_tracking` is
the deliberate exception: it is flipped only by `set_agent_tracking`, which
also installs/uninstalls the Claude Code hooks as a side effect, so a stale
frontend save overwriting it out-of-band would desync the config from the
actual hook state on disk. `agent_hook_port` is the same kind of exception:
it is the literal port in the installed hooks' URL (rule 13), chosen and
persisted only by `commands::start_hook_receiver`. If you add a new bool/enum setting, copy it in
`merge_layouts_from` like the rest — only mirror this exception if the setting
is similarly owned by a backend side effect, not just because it's convenient.

### 12. The Claude Code hook settings merge is marker-based — never reorder or drop foreign hooks

`agent_hooks::install_hooks` / `uninstall_hooks` rewrite the user's
`~/.claude/settings.json` in place. Every hook ymux owns is an `http` entry
whose URL is a loopback host with the `/ymux-agent-hook` path (any port); the
retired `y` command entries are recognised by their `--ymux-agent-hook`
marker. Install only touches ymux entries (refreshing the port, dropping the
retired ones) or appends a new group, and uninstall only removes ymux entries
of either shape, via `retain`/`retain_mut` — never `remove`,
which under `serde_json`'s `preserve_order` is a `swap_remove` and would
reorder the user's own keys and hooks. Any hook or settings key without the
marker must come back byte-for-byte — including a foreign `http` hook on
loopback, and the user's own `allowedHttpHookUrls`/`httpHookAllowedEnvVars`
items. If you touch this file, run the
`install_preserves_foreign_hooks_and_key_order` / `uninstall_restores_foreign_settings_exactly`
tests before anything else — they exist specifically to catch an edit that
silently reorders or eats someone else's hook.

### 13. Claude Code hooks arrive over loopback HTTP — token-gated, empty-bodied, fixed port

Agent tracking has no helper binary. `agent_hooks::install_hooks` writes a
`type: "http"` handler under every event in `HOOK_EVENTS`:

```json
{ "type": "http", "url": "http://127.0.0.1:<port>/ymux-agent-hook", "timeout": 2,
  "headers": { "X-Ymux-Pane": "${YMUX_PANE_ID}", "X-Ymux-Token": "${YMUX_HOOK_TOKEN}" },
  "allowedEnvVars": ["YMUX_PANE_ID", "YMUX_HOOK_TOKEN"] }
```

and `hook_http` (pure std, not desktop-gated, tested with a real listener)
receives it. `commands::start_hook_receiver` wires accepted events into
`apply_agent_hook` — the same `HookEvent` the old `y` relay produced, so
`agents.rs`, the tree and the resume binding are unchanged. The rules:

- **Loopback only.** Bind `127.0.0.1`, never `0.0.0.0`, and write `127.0.0.1`
  (not `localhost`, which may resolve to `::1` first) into the URL. On Windows
  `bind_loopback` sets `SO_EXCLUSIVEADDRUSE` (defence in depth: on Windows 11
  a same-address `SO_REUSEADDR` squat is refused anyway, and a wildcard
  squatter loses every `127.0.0.1` connection to the specific socket).
- **Hooks are user-level, so every Claude session sends to the port.** While
  tracking is on, Claude sessions *outside* ymux POST their hook events —
  prompts, tool input and output — to `127.0.0.1:<port>` too. A running ymux
  answers them with a quiet 204 and reads nothing (no token); while ymux is
  closed, whatever process holds the port receives them. Turning tracking off
  removes the hooks. This is why the port must never move while a ymux uses
  it (next bullets) and why the README says so.
- **Bounded.** The token is checked only after the head arrives, so any local
  user can connect: each connection has a whole-request deadline
  (`hook_http::LIMITS`, 3 s, re-applied before every read), at most 32 are
  served at once (extras are closed on accept), only a quiet 204 drains an
  unread body, and nothing in the path may panic (release aborts on panic;
  `hostile_heads_and_bodies_never_panic` fuzzes it).
- **Token + pane.** ymux mints a fresh token per run (244 CSPRNG bits)
  (`hook_http::new_token`) and injects `YMUX_HOOK_TOKEN` into every PTY next to
  `YMUX_PANE_ID`; Claude Code interpolates both into the headers. Wrong token
  (constant-time compare) or a pane this ymux doesn't own → 403. An `Origin`
  header → 403: browsers attach one to every cross-origin POST and can't set
  the custom headers without a preflight, so a web page in a browser pane
  can't forge events; Claude Code's client never sends one (verified against a
  live 2.1 run). An empty token → **204, ignored** — a Claude session
  started outside ymux, or in a pane of a ymux without a receiver, and a
  non-2xx would put a hook error into it. An empty token authorises nothing.
  `GET /ymux-agent-hook/ping` answers a fixed `ymux-agent-hook/1` with no
  token (browsers still refused by `Origin`). See `hook_http::authorize` for
  the full order.
- **2xx means an empty body.** Answer 204 with no body on success. A 2xx JSON
  body is parsed by Claude Code as hook output (decisions, context) and any
  other 2xx body is an error. The response goes out before the registry is
  touched, so Claude never waits on ymux's locks; an oversized authenticated
  body is also a quiet 204 (dropped), never an error.
- **The port is fixed.** Claude Code interpolates env vars into header values
  only, never into the URL, so the port is a literal in the user's
  `settings.json`. It is chosen once, kept in `Config::agent_hook_port`
  (backend-owned, not in `merge_layouts_from` — rule 11) and reused every
  launch. If it is taken, `choose_port` pings it first: **another ymux →
  this instance runs without a receiver** (its panes get an empty token and
  no hook events; the process scan still lists their agents) and leaves the
  port and `settings.json` alone — moving them would send the first
  instance's token to a port anyone can take once it exits. No
  single-instance plugin: that would change launch behaviour. Only a
  non-ymux holder makes it take an OS-assigned port; release builds persist
  it and the startup refresh rewrites the hooks' URL. Debug
  builds (`tauri dev`, sharing the live config) neither persist nor refresh
  unless `YMUX_DEV_AGENT_HOOKS=1`.
- **A hook's session id is a claim, not a fact.** Anything holding the
  token can POST any id, and a resumed Claude runs with
  `--dangerously-skip-permissions` (deliberately, for every Claude resume —
  don't change that). So `SessionTracker::observe` records a hook-borne id
  only when the pane's own Claude process vouches for it: the pane is already
  bound to it, or the scan's process there names it (pid file or argv), or a
  transcript for it is in the pane's cwd (`ypath::same_path`). Unvouched ids
  still drive the agent tree's live status; they just never steer a resume.
- **No `SessionStart`.** Claude Code runs only `command`/`mcp_tool` handlers
  for it (and `Setup`), so the lead and the hook session id first arrive with
  `UserPromptSubmit`; the process scan and transcript fallback cover the gap.
- **Restriction knobs.** `allowedHttpHookUrls` / `httpHookAllowedEnvVars`
  default to unset (nothing blocked). If the user-level settings already
  define either, install adds the exact `http://127.0.0.1:<port>/ymux-agent-hook`
  (replaced when the port changes — never a `:*` wildcard, which would loosen
  the user's own restriction) / the two env names, and uninstall removes
  exactly those; neither key is ever created (defining `allowedHttpHookUrls`
  would block every other http hook).
  Managed or project-level lists are out of ymux's reach.
- **ymux not running.** The hooks then fail with a connection error, which
  Claude Code treats as non-blocking but *reports* (e.g. "Stop hook error
  occurred · ctrl+o to see" per turn). That is the cost of having no relay
  binary; the answer is turning tracking off before uninstalling.
- **Migration.** Install removes every retired `y … --ymux-agent-hook` command
  entry (`agent_hooks::LEGACY_MARKER`), `SessionStart` included; uninstall
  removes both shapes. Keep that until no supported upgrade path can still
  carry `y` entries.

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

### Test count (Rust 455, 8 failing on Windows + frontend 616)

Measured 2026-09-24 on Windows with `cargo test -p ymux --lib`,
`cargo test -p ytheme -p ypath` and `npx vitest run`.

| Crate | Tests | What they cover |
|-------|-------|-----------------|
| ymux_lib | 440 (429 pass, 8 fail on Windows, 3 ignored; 405 without `desktop`: 396 pass, 8 fail, 1 ignored) | Config model + TOML round-trip, PTY, OSC 7 (incl. `CwdChange` respelling dedupe), shell detect, macOS shell integration, updater, sysmonitor, git log/branch/worktree porcelain (non-ASCII + cross-source path comparison, real-git round-trip), filesystem + text-file commands (`fsx`, `fsops`, `textfile`: EOL/BOM round-trip), command guards (`ipc_guard`), resumable agent sessions (`agent_sessions.rs`, `agent_scan_disk.rs`: resume-argv building, selector stripping, transcript disk scan), agent registry (`agents.rs`), process-tree agent scan (`agent_scan.rs`), Claude Code http-hook settings merge + `y` migration (`agent_hooks.rs`), hook receiver auth/parsing/port choice + live listener (`hook_http.rs`) |
| ytheme | 6 | Theme TOML round-trip, hex parsing, defaults |
| ypath | 9 | NFC folding, drive/UNC/verbatim/WSL case rules, POSIX case sensitivity, backslash as a POSIX filename character |
| _frontend_ | 616 (44 files) | vitest: layout tree, pane tabs, agent tree model, file dock (`cwdFollow`, `dockModel`), files pane models, editor models (EOL, close guard, drafts, keymap, headless CM6), git pane models (graph lanes, keys), bottom-anchored prompt, IME, pane status, workspace reorder, drop paths, viewport sync, scrollback, platform shortcut mapping, Settings shortcut list vs. main.ts's keydown handler (`shortcutList.test.ts`) |

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
2. Builds the MSI on Windows **and creates the release** —
   it goes first precisely so exactly one job ever creates it
3. Builds the arm64 `.dmg` on macOS and uploads it onto that release
4. Rewrites the release body with install info + auto-generated notes

Then: GitHub → Releases → Edit draft → Publish.
