# GUI tool panes: retiring the TUI sidecars

Date: 2026-09-24 · Status: design, not yet planned

The `y*` family — `ydir`, `ycode`, `ygit`, `ymon` and the `y` launcher — stops
shipping as standalone CLI binaries. Their functionality returns as **GUI panes
rendered by ymux's own frontend**, alongside the terminal and browser panes that
already exist.

**The decision is made.** The user has accepted losing "run `ydir` in any
terminal". This document does not re-open it; it works out what that costs and
in what order to pay it.

Scale, for calibration: ~7,200 lines of Rust TUI (ydir 2,791 · ycode 3,067 ·
ymon 726 · ygit 615) plus 292 lines of launcher come out; three new pane
implementations and a new backend filesystem/git surface go in. It is
the largest single change the project has attempted.

**One sidecar survives** — see §5 Step 5. `y` is not only a launcher: it is the
relay that carries Claude Code hook events into the agent tree, and the
absolute path to it is written into every tracking user's
`~/.claude/settings.json`. Dropping it without a replacement breaks the agent
tree *and* leaves those users' Claude Code invoking a missing binary on every
hook event. *(As implemented: `y` keeps its name and path and shrinks to the
hook relay alone — see the Step 5 amendment.)*

Five sub-projects, in this order:

1. **Backend surface** — filesystem and git commands. No UI change.
2. **Files pane** (`ydir`) — and the file dock stops hosting a PTY.
3. **Editor pane** (`ycode`) — and the viewer-tab flow stops spawning one.
4. **Git pane** (`ygit`).
5. **Cut-over** — the hook relay, then the sidecars leave the bundle, CI,
   `build-tools.mjs` and the docs.

---

## 0. Pane model — the decisions that bind every part

### 0.1 New `PaneKind` variants

`src-tauri/src/config/model.rs:431` today:

```rust
pub enum PaneKind { Terminal, Browser, NativeBrowser, EmbeddedBrowser }
```

Three variants are added: `Files`, `Editor`, `Git`. Serde renders
them lowercase, matching the existing `native_browser` / `embedded_browser`
style, so TOML carries `pane_kind = "files" | "editor" | "git"`.

Adding enum variants is additive and the field already carries
`#[serde(default, rename = "pane_kind")]` (`model.rs:467`). A config written by
an older ymux has no new value in it and loads unchanged.

**Why three and not one.** A single `PaneKind::Tool` with a discriminating
`PaneSpec` field would push the switch from a Rust enum the compiler checks
into a string nobody checks, and it would make `createPane`'s dispatch
(`src/workspace/WorkspaceManager.ts:470`) two levels deep instead of one. The
existing code already proves the wide-enum shape works: browser panes are three
separate variants.

### 0.2 `PaneSpec` fields — exactly one addition

Rule 2 (the 4-place sync) is the project's documented #1 source of bugs, so the
budget is one field.

| Pane kind | What it needs | Where it comes from |
|---|---|---|
| `files` | the directory shown | **`cwd`** — already exists |
| `git` | the repo | **`cwd`** — already exists |
| `editor` | the open file | **new `file_path`** |

```rust
/// Absolute path of the file an `Editor` pane has open. Empty = untitled
/// (an editor pane restored with no file shows its empty state).
/// A `String`, not an `Option<String>`: rule 3 — `Option<T>` inside a
/// `#[serde(tag = "kind")]` tagged enum does not round-trip through TOML,
/// and `PaneSpec` is flattened into `LayoutNode::Pane`.
#[serde(default)]
pub file_path: String,
```

Rule 3 is the reason this is not `Option<String>`, and the reason it matches
the shape of `bg_color` and `worktree_path` next to it.

The four places (rule 2), plus the test:

1. `src-tauri/src/config/model.rs` — the struct and all three constructors
   (`new_default`, `placeholder`, `new_browser`), plus new `new_files(cwd)`,
   `new_editor(path)`, `new_git(cwd)`.
2. `src/types.ts:27` — `PaneSpec.file_path?: string`.
3. `src/layout/LayoutTree.ts:56` — `nodeToSpec()` manual field copy.
4. `src/workspace/WorkspaceManager.ts` — `findAndMutatePane()`'s snapshot +
   write-back. The two halves are at `:1689` (snapshot) and `:1701`
   (write-back); CLAUDE.md rule 2 cites `:603`, which has drifted.
5. `panespec_all_fields_roundtrip` in `model.rs`'s test module.

### 0.3 `CONFIG_VERSION` does **not** move

It stays at 8 (`model.rs:39`). Rule 8 says bump for shell-detection changes or
changed semantics of an existing field; this is neither. The new variants and
`file_path` are purely additive with serde defaults, which rule 8 explicitly
exempts.

There is also nothing to migrate, and this was **verified rather than
assumed** — it is the single fact that decides whether `CONFIG_VERSION` moves:

- The tool context menu (`src/workspace/WorkspaceManager.ts:717`) calls
  `pane.runCommand(tool.command)` — it **types the command into the running
  shell's PTY**. Nothing is written to the spec.
- The viewer tab and the file dock run their tool through `createPane`'s
  `argv` parameter, which reaches `TerminalPane.spawn()` (`:529`) and then
  `SpawnArgs.argv` (`src-tauri/src/commands.rs:41`). **`argv` is a spawn-time
  argument and has no `PaneSpec` field**, so `ycode <path>` and `ydir --dock`
  are never serialised. `WorkspaceManager.ts:85-88` says the same thing from
  the other side: the viewer-tab registry is runtime-only, and after a restart
  that tab is an ordinary terminal pane on the default shell.

So no `ydir`/`ycode`/`ymon`/`ygit` string ymux itself wrote is in anyone's
config, and there is nothing for `migrate()` to rewrite. **If this is ever
found to be wrong** — if some path does persist a tool argv — the answer is a
`CONFIG_VERSION` 9 bump with a `migrate()` that rewrites those panes to
`pane_kind = "editor"` / `file_path`, because that is rule 8's "existing field
semantics change". Re-verify the three bullets above before starting step 3.

What *can* be in a user's config is a hand-written `startup_cmd` or a
`HotKeyDef.command` that runs `ycode foo.rs`. Those are user data with
arbitrary content; ymux will not pattern-match and rewrite them. They will stop
working, which §6 records as a known break.

### 0.4 No new `Config` settings in v1

Deliberate. Every `Config` setting must be copied in
`Config::merge_layouts_from` (`model.rs:168`) or it silently reverts on every
restart (rule 11 and the memory note). Editor preferences (tab width, word
wrap, font) are real wants, but each one is another line in that function and
another way to lose a setting. They are deferred (§2.3) and land together, once,
after the panes ship.

### 0.5 Panes without a PTY, inside `PaneGroup`

`PaneGroup` (`src/layout/PaneGroup.ts`) assumes terminal children: it owns one
shared `HotKeyBar` whose buttons write to a PTY, and builds children with
`ownChrome: false`.

The three GUI panes implement `Pane` (`src/layout/Pane.ts`) exactly as
`BrowserPane` does — `element`, `focus()`, `scheduleFit()`, `spawn()`,
`dispose(permanent?)` — so `SplitContainer.render()` needs no change; it already
operates on the interface (`src/layout/SplitContainer.ts:14`).

Inside a tab group:

- The **hotkey bar is hidden** when the active tab is a GUI pane. `HotKeyBar`
  writes to a PTY that does not exist; showing dead buttons is worse than
  showing none. `PaneGroup.update()` toggles a `.pane-group--no-hotkeys` class
  and skips `hotkeyBar.bind()`.
- **GUI panes accept `ownChrome`** the same way `TerminalPane` does, so the
  group's title row is the only title.
- Rule 14 applies unchanged: a GUI pane shown after being hidden still gets
  `scheduleFit()` on the next animation frame. For CodeMirror that is
  `view.requestMeasure()`; for the files/git panes it is a no-op, but
  the call site stays uniform.

### 0.6 Keyboard precedence — decided once, here

Rule 6 exists because `TerminalPane.attachCustomKeyEventHandler` must *block*
keys so they bubble to ymux's global handler. A GUI pane with its own keymap
creates the same problem, and CodeMirror creates the worst case: its keymaps
call `preventDefault` and stop propagation on a match.

**The rule: ymux globals always win.** Every GUI pane attaches its key handling
so that a chord in the global set never reaches the pane. For `EditorPane` that
means filtering CM6's default extensions rather than fighting them — the keymap
is built by removing bindings whose chord collides, not by layering a capture
handler CM6 cannot see.

Collisions to resolve against `src/help/HelpOverlay.ts`'s `SHORTCUTS` array
(the canonical list). Known ones, from CM6's `defaultKeymap`, `foldKeymap`,
`historyKeymap` and `searchKeymap`:

| Chord | ymux | CM6 | Resolution |
|---|---|---|---|
| `Ctrl+Shift+[` / `]` | prev/next tab | `foldCode` / `unfoldCode` | ymux wins; fold rebinds to `Ctrl+Alt+[` / `]` |
| `Ctrl+Shift+Z` | zoom pane | redo (and `Cmd+Shift+Z` on macOS via `platform.ts`) | ymux wins; redo stays on `Ctrl+Y` |
| `Ctrl+F` | ymux search | `openSearchPanel` | **CM6 wins when an editor pane has focus** — the one deliberate inversion, because ymux's `Ctrl+F` searches terminal scrollback, which an editor pane does not have |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | font size | — | ymux wins; also applies to the editor's font |
| `Ctrl+V` | paste | — | pane-local |
| `Ctrl+Shift+W` / `T` / `H` / `V` / `E` / `P` / `R` | pane and workspace management | — | ymux wins |

**This table is a starting point, not the answer.** Enumerating the real CM6
keymaps against `SHORTCUTS` is an explicit task of step 3 (§5), and every
binding the editor pane keeps must go through rule 6's full 6-step checklist
(main.ts handler, pane block list, Help overlay, Command Palette, i18n key,
three READMEs). `platform.ts`'s `hasMod(ev)` is used throughout — never a raw
`ev.ctrlKey` — or macOS users lose these chords to the app (rule 10).

---

## 1. Backend surface

New Rust, no UI change. Ships on its own and nothing is deleted.

### 1.1 Module layout and the `desktop` gate (rule 1)

The project's established split is: **pure logic ungated, IO and Tauri gated**
(`agents.rs` vs `agent_scan.rs`, `agent_hooks.rs`'s merge fns vs its file IO).
The new code follows it, so `cargo check --no-default-features --lib --tests -p ymux`
keeps passing on Linux and the parsers stay unit-testable there.

| New/changed file | Gated? | Contents |
|---|---|---|
| `src-tauri/src/fsx.rs` | **no** | `DirEntryInfo`, sorting/filtering, hidden-file rule, size formatting, `is_probably_binary(&[u8])` |
| `src-tauri/src/textfile.rs` | **no** | `Eol` detection + restoration, BOM handling, `ContentStamp` (mtime + hash), `decode`/`encode`, `MAX_EDIT_BYTES` |
| `src-tauri/src/fsops.rs` | **desktop** | the filesystem commands (read_dir, read/write, mkdir/rename/copy/move/delete) |
| `src-tauri/src/git/mod.rs` | **no** (parsers) / **desktop** (commands) | add `parse_log_porcelain`, `parse_branch_list`; `log()`, `branches()`, `checkout()` |
| `src-tauri/src/sysmonitor.rs` | **desktop** | unchanged — no Monitor pane, no new metrics surface |

`fsops.rs` is gated because it takes a `Webview` (see §1.5) and returns through
`#[tauri::command]`. Its *decisions* — what to list, how to sort, whether a
write is safe — live in `fsx.rs`/`textfile.rs` and are not.

### 1.2 Filesystem commands

```rust
struct DirEntryInfo { name, path, is_dir, is_symlink, size, modified_ms }

fs_list_dir(path: String, show_hidden: bool) -> YmuxResult<Vec<DirEntryInfo>>
fs_roots()                                   -> YmuxResult<Vec<String>>   // drives (Win) / "/" (Unix)
fs_home_dir()                                -> YmuxResult<String>
fs_stat(path: String)                        -> YmuxResult<DirEntryInfo>
fs_create_dir(path: String)                  -> YmuxResult<()>
fs_create_file(path: String)                 -> YmuxResult<()>
fs_rename(from: String, to: String)          -> YmuxResult<()>
fs_copy(from: String, to: String, overwrite: bool) -> YmuxResult<()>
fs_move(from: String, to: String, overwrite: bool) -> YmuxResult<()>
fs_delete(paths: Vec<String>, to_trash: bool)      -> YmuxResult<()>
fs_read_text(path: String)  -> YmuxResult<TextFile>
fs_write_text(args: WriteArgs) -> YmuxResult<ContentStamp>
fs_reveal(path: String)     -> YmuxResult<()>   // Explorer/Finder
fs_open_default(path: String) -> YmuxResult<()> // reuses settings.rs's opener
```

```rust
struct TextFile { text, eol: Eol, bom: bool, stamp: ContentStamp, truncated: bool }
struct ContentStamp { modified_ms: u64, sha256: String }
struct WriteArgs { path, text, eol: Eol, bom: bool, expect: Option<ContentStamp> }
```

**Encoding, BOM and line endings.** `ycode` today is UTF-8 only, normalises
CRLF away through `str::lines()`, and drops a trailing newline on save
(`tools/ycode/src/buffer.rs`). That is a bug set the GUI must not inherit,
because ymux is a Windows-first app where CRLF files are normal:

- `fs_read_text` detects the dominant EOL and returns it; `fs_write_text`
  restores it. Mixed endings report `Eol::Mixed` and write back LF, once,
  after a GUI warning.
- A UTF-8 BOM is stripped on read, recorded, and restored on write.
- Non-UTF-8 content is **not** silently lossy-decoded. `fs_read_text` returns
  `YmuxError::NotUtf8` and the editor pane shows a read-only hex/byte notice.
  Legacy CP949/EUC-KR support is deferred (§2.3) — guessing wrong and then
  saving destroys the file.
- A trailing newline present on read is present on write.

**Size cap.** `MAX_EDIT_BYTES = 8 MiB`. Over it, `fs_read_text` sets
`truncated: true` and the editor opens read-only. Neither editor handles a
200 MB file well; refusing loudly beats hanging.

**Deletion.** `to_trash: true` routes through the `trash` crate (MIT/Apache-2.0,
Windows + macOS support). `ydir` today deletes with `remove_dir_all` and **no
confirmation** (`tools/ydir/src/app.rs`, key `d`). The GUI default is
trash + a confirm dialog; permanent delete is Shift+Delete.

**Path comparison (rule 15).** "Is this file already open in a pane?" and "is
this the directory the pane shows?" go through `ypath::same_path` **on the Rust
side**, never a new case rule in TypeScript. Two new helpers in `fsx.rs`:
`same_file(a, b)` and `is_within(dir, path)`, both `ypath`-backed. The
frontend's only normalisation stays what `src/filedock/cwdFollow.ts` already
does — `normalize("NFC")` and nothing more — for exactly the reason rule 15
gives: a second, differently-opinionated case rule in TS is the one way to make
the layers disagree.

### 1.3 Git commands

`src-tauri/src/git/mod.rs` already owns worktree support with a pure porcelain
parser (`parse_worktree_porcelain`, `mod.rs:146`). Log and branch parsing join
it in the same shape.

```rust
struct CommitInfo { hash, short, subject, author, date_ms, parents: Vec<String>, refs: Vec<String> }
struct BranchList { current: String, local: Vec<String>, remote: Vec<String> }

git_log(cwd: String, limit: u32, skip: u32) -> YmuxResult<Vec<CommitInfo>>
git_branches(cwd: String)                   -> YmuxResult<BranchList>
git_checkout(cwd: String, branch: String)   -> YmuxResult<()>
git_repo_root(cwd: String)                  -> YmuxResult<String>
```

`ygit` shells out to `git log --graph --oneline --all --decorate --color=never`
and parses the drawn ASCII graph back out with a lane colouriser
(`tools/ygit/src/graph.rs`). The GUI does not inherit that. `git_log` uses a
machine format:

```
git log --all --date-order --max-count=<limit> --skip=<skip>
        --format=%H%x1f%h%x1f%s%x1f%an%x1f%at%x1f%P%x1f%D%x1e
```

`%x1f`/`%x1e` (unit/record separators) cannot appear in a subject or refname,
so `parse_log_porcelain(&str) -> Vec<CommitInfo>` is a pure, total function with
no escaping hazards — testable on Linux against captured fixtures, the same way
`parse_worktree_porcelain` is.

`parse_branch_list` reuses `ygit`'s one genuinely good parser, `branch_name`
(`tools/ygit/src/app.rs`), which strips exactly one 2-column marker (`* `,
`+ `, `  `) and rejects `(HEAD detached at …)` pseudo-entries. It is ported
verbatim with its `REAL_BRANCH_OUTPUT` fixture (git 2.52, Korean branch names,
a `+ ` worktree-held branch). Input becomes `git branch --list --format=…`, but
the marker-stripping test stays as a regression guard.

`ygit` blocks its render thread on every git call. The commands are
`#[tauri::command(async)]`, like `filedock_change_dir` already is
(`ipc_server.rs:95`), so a slow repo cannot freeze the window.

Commit **diffs**, staging, commit, push/pull and stash are out of scope (§9) —
`ygit` has none of them either.

### 1.5 Capability scoping — the part that must not be got wrong

This app hosts **remote content**. `src-tauri/capabilities/browser-children.json`
grants `core:default` to child webviews `eb-*` with
`"remote": { "urls": ["http://**", "https://**"] }`, so pages the user browses
to in an embedded browser pane have IPC access to the host.

We are about to add commands that read, write and delete arbitrary files and
list processes.

**A capability is not the gate here.** `src-tauri/build.rs` is a bare
`tauri_build::build()` with no `AppManifest`, so ymux's *own* commands — the
ones registered through `generate_handler!` — are not declared in any
capability file and are not filtered by the permission system. The capability
files govern plugin and `core:` permissions only. **The in-command guard is
therefore the only gate**, not defence in depth.

There is a second exposure the label alone does not close: `pane_kind =
"browser"` is an **iframe inside the `main` webview**, and the CSP permits
`frame-src http: https:`. A `webview.label()` check passes for anything running
there. The memory note on `chrome-error://` is the relevant evidence — Tauri
validates the IPC request's **Origin** header, which is why a page at an
invalid origin gets "Origin header is not a valid URL". So origin, not label,
is the discriminator that separates ymux's own document from embedded content.

**Rules, non-negotiable:**

1. Every new fs/git/process command takes `webview: tauri::Webview` and returns
   `YmuxError::Forbidden` unless **both** the label is `main` **and** the
   request's origin is ymux's own app origin (`tauri://localhost` /
   `http://tauri.localhost`, resolved from the running config rather than
   hard-coded). A shared `guard_local(&webview)?` helper, called on the first
   line of each command, so the check cannot be forgotten by omission.
2. **Before step 1 is called done**, verify empirically — from a page loaded in
   a `browser` pane and from an `eb-*` child webview — that `invoke("fs_read_text", …)`
   is rejected. Record the result in the step's commit message. Design the
   guard for the assumption that remote content *can* reach the IPC channel,
   because that is the assumption that fails safe.
3. `browser-children.json` is not touched: `permissions` stays exactly
   `["core:default"]`, and ymux's commands are never added to it.
4. Rust tests assert `guard_local` rejects a non-`main` label and a remote
   origin, and accepts the local one.
5. No path allow-listing. A terminal multiplexer whose panes already run
   arbitrary shells cannot meaningfully sandbox its own file pane; the boundary
   that matters is *who can call*, not *what path*.

**CSP is not widened.** The current policy
(`src-tauri/tauri.conf.json:27`) is `script-src 'self'` with no `worker-src`
and no `blob:`. This constrains the editor choice (§3.2) and that is on
purpose — an app hosting remote iframes should not loosen its script sources to
get syntax highlighting.

### 1.6 Error handling

All commands return `YmuxResult<T>`. `YmuxError` (`src-tauri/src/error.rs`)
gains variants the frontend can branch on rather than string-matching:

`NotFound` · `PermissionDenied` · `AlreadyExists` · `NotUtf8` · `TooLarge` ·
`Conflict` (stale write, §2.2) · `NotARepo` · `Forbidden`.

Each maps to an i18n key in all 13 languages (rule 7). A command never panics;
`fsx`/`textfile` functions are total.

### 1.7 Tests

**Rust, Linux-safe (`--no-default-features`):**

- `fsx`: hidden-file rule; dirs-first + case-insensitive name sort (matching
  `ydir`'s order so the pane feels the same); size formatting boundaries;
  `is_probably_binary` on NUL, on pure text, on a UTF-8 Hangul sample, on an
  8 KiB sniff window cut mid-syllable (ported from `tools/ydir/src/preview.rs`'s
  18 tests); `same_file` / `is_within` across the rule-15 respellings already
  covered by `ypath` — drive-letter case, `/` vs `\`, NFD vs NFC Hangul.
- `textfile`: EOL detect (LF, CRLF, mixed, none); round-trip preserves CRLF;
  trailing newline preserved and absent-newline preserved; BOM stripped and
  restored; `NotUtf8` on invalid bytes; `TooLarge` over the cap; `ContentStamp`
  changes when content changes and not when it does not.
- `git`: `parse_log_porcelain` on a captured multi-parent, multi-ref fixture
  including a Korean subject and a subject containing `|`; empty output; a
  commit with no refs; `parse_branch_list` keeps `ygit`'s
  `branch_name_strips_every_marker_git_writes`,
  `branch_name_rejects_the_detached_head_pseudo_entry` and
  `branch_name_strips_one_marker_not_a_run` verbatim.

**Rust, desktop, `tempfile`-backed integration (Windows/macOS only):** create /
rename / copy / move / delete round-trips; `fs_write_text` refuses on a stale
stamp and succeeds on a fresh one; `fs_delete` with `to_trash` on a real file;
the `Forbidden` webview-label guard.

---

## 2. Files pane (`ydir` → `PaneKind::Files`)

### 2.1 Feature inventory — keep / drop / defer

`tools/ydir/src` is 2,791 lines and 73 tests. What of it survives:

| Feature (source) | v1 | Note |
|---|---|---|
| Directory listing, dirs-first + case-insensitive sort | **keep** | Same order, so muscle memory survives |
| Hidden toggle (leading `.` only) | **keep** | Plus the Windows hidden *attribute*, which ydir ignores |
| Enter to descend / Backspace to parent | **keep** | Plus breadcrumb click and double-click |
| `Ctrl+R` refresh | **keep** | Plus an automatic refresh on window focus |
| Open file → editor | **keep** | Now an in-process editor pane (§3), not a spawned `ycode` |
| Binary sniff before opening (`is_binary_file`, 8 KiB NUL) | **keep** | Moves to `fsx::is_probably_binary`; same window, so pane and preview still cannot disagree |
| Copy / move / paste | **keep, improved** | ydir's clipboard holds **one** item and **clobbers** a same-named destination with no prompt. v1: multi-select clipboard + an overwrite/rename/skip dialog |
| Delete | **keep, improved** | ydir deletes with `remove_dir_all` and **no confirmation**. v1: trash + confirm |
| **Rename** | **add** | ydir has none — "move" always preserves the filename. F2 renames |
| **New folder / new file** | **add** | ydir has neither |
| **Multi-select** | **add** | Ctrl/Shift click, Ctrl+A |
| Dock mode (`--dock`, single-pane + preview layout) | **keep** | Becomes the dock's normal rendering (§2.4) |
| `PendingDir` 300 ms quiet-period debounce | **replace** | Its reason was "don't swap the listing mid-keystroke so `d` hits a different file". `src/filedock/cwdFollow.ts`'s 200 ms debounce already does the frontend half; the GUI keeps selection stable across a cwd change by re-selecting by path |
| `same_dir` 3-tier compare (bytes → `ypath` → canonicalize) | **keep** | Moves to `fsx::same_file` (§1.2) |
| File preview (text / dir / binary, 64 KiB, 200 lines) | **keep** | `sanitize`'s control-char stripping is no longer needed (no terminal to repaint) but the byte/line caps are |
| Dual-pane (two panels, Tab to switch) | **drop** | Never reachable in dock mode; in a pane, a second files pane is a split. This is ~250 lines of `Panel` state that a tiling multiplexer makes redundant |
| Run dialog (Enter on `.exe`/`.ps1`/`.sh` → args prompt → TUI suspend → run) | **drop** | It exists because a TUI cannot hand off to a shell. ymux has terminal panes; v1 offers "Open in terminal here" and "Run in new pane", which is strictly better |
| `is_executable` extension allow-list | **drop** | Only the run dialog used it |
| TUI suspend/resume around child processes | **drop** | Meaningless in a GUI |
| Search / filter within a directory | **defer** | ydir has none either; a type-to-filter box is an obvious v2 |
| Git status decorations on rows | **defer** | Wants `git status --porcelain=v2`; v2 |
| Archive browsing, bookmarks, drive switcher | **drop** | ydir has none |

**Honest summary of what a v1 files pane will not have:** no in-place search,
no git decorations, no drag-and-drop between panes, no archive support, no
column sorting by size or date (ydir has none of these either). It *gains*
rename, create, multi-select, trash-with-confirm and overwrite prompts.

### 2.2 Components

- `src/files/FilesPane.ts` — implements `Pane`. Chrome: breadcrumb + toolbar
  (up, refresh, new folder, hidden toggle), the list, the preview pane.
- `src/files/fileModel.ts` — **pure**. `sortEntries`, `applyHidden`,
  selection state machine (anchor/extend/toggle), `nextSelectionAfterDelete`,
  `resolveOverwrite`. No DOM, no IPC.
- `src/files/preview.ts` — **pure**. Given bytes/text and a kind, produce the
  preview model. Ports `tools/ydir/src/preview.rs`'s decisions: 64 KiB /
  200 lines / 200 dir entries, and the incomplete-trailing-UTF-8-sequence rule
  (a read cap that cuts a Hangul syllable drops the fragment rather than
  rendering a replacement char).
- `src/ipc/bridge.ts` — typed wrappers for the §1.2 commands.
- i18n keys, 13 languages (rule 7).

### 2.3 Deferred

Type-to-filter; git status decorations; sort-by-column; drag-and-drop; the
editor preference `Config` settings (§0.4).

### 2.4 The file dock stops hosting a PTY

Today `src/filedock/FileDock.ts` spawns a `TerminalPane` with the reserved id
`DOCK_PANE_ID` running `ydir --dock <cwd>` (`dockModel.ts:dockArgv`), and
follows the active pane's cwd by pushing `IpcMessage::ChangeDir` over yipc
(`filedock_change_dir` → `send_to("ydir", …)`).

After this step the dock hosts a `FilesPane` directly. What that deletes:

- `src/filedock/dockModel.ts` → `dockArgv()` and its test.
- `FileDock`'s PTY lifecycle: `startYdir`, the exited/failed message states,
  the Restart button, `DOCK_PANE_ID`.
- `src/ipc/bridge.ts` → `fileDockChangeDir`.
- `src-tauri/src/ipc_server.rs` → `filedock_change_dir` (`:95`) and the
  `IpcServerState` push path.
- `crates/yipc` → `IpcMessage::ChangeDir`, `IpcServer::send_to` and the
  client registry it needs, plus their tests and rule 13's two-tool-name dance
  (`"ydir"` / `"ydir-openfile"`) — which exists only because `send_to` fans out.
- `tools/ydir/src/dock.rs` → `follow_host`, `PendingDir`. (The crate itself
  goes in §5.)

`src/filedock/cwdFollow.ts` **stays** — it is the pure debounce/dedupe, and it
now calls `FilesPane.navigate(dir)` instead of an IPC command. Its 200 ms
debounce and NFC dedupe are unchanged; rule 15's note about the frontend mirror
being deliberately partial still applies, because the Rust side still owns the
full comparison (`fsx::same_file`).

**Rule 13 becomes dead.** It documents why yDir registers under two tool names.
Once `send_to` is gone, so is the hazard. It is removed from CLAUDE.md in §5.

### 2.5 Tests

- **vitest, pure:** `fileModel` — sort order matches ydir's fixtures (dirs
  first, case-insensitive, Hangul names); hidden filter; selection anchor/extend
  /toggle; selection after deleting the last row; overwrite resolution
  (rename → `foo (2).txt`, skip, replace). `preview` — text head, byte cap,
  line cap, dir cap and ordering, NUL → binary, Hangul syllable cut by the cap,
  CRLF, a file with no trailing newline.
- **vitest:** `cwdFollow` tests stay green unchanged — that is the point of
  keeping the module.
- **Manual (GUI):** navigate with keyboard and mouse; copy/move across two
  files panes; delete to trash and restore; rename a file the editor has open
  (§8 risk 1); the dock following a `cd` in a terminal pane; a deleted cwd; a
  UNC path; a WSL path typed into the breadcrumb.

---

## 3. Editor pane (`ycode` → `PaneKind::Editor`)

### 3.1 Feature inventory — keep / drop / defer

`tools/ycode/src` is 3,067 lines and 69 tests.

| Feature (source) | v1 | Note |
|---|---|---|
| Open / save a UTF-8 file | **keep** | Plus CRLF/BOM/trailing-newline preservation, which ycode loses |
| Undo / redo | **keep, improved** | ycode pushes a **full-buffer clone per keystroke**, caps at 1000 with `Vec::remove(0)`, and never returns to a clean state (`undo` sets `dirty = true` unconditionally). CodeMirror's history coalesces by time and tracks the saved generation |
| Cursor movement, Home/End, PageUp/PageDown | **keep** | Free |
| CJK / wide-char correctness | **keep** | ycode's mechanism was char-indexed columns + `UnicodeWidthChar` (`ui.rs:visual_column`). In a GUI the browser does the layout; the risk moves to IME (§3.2) |
| Syntax highlighting | **keep** | syntect (~200 langs, re-highlighted from line 0 **every frame**) → Lezer, incremental, ~8 grammars in v1 (§3.3) |
| ytheme-driven syntax colours | **keep** | The 8 `SyntaxColors` map to a `HighlightStyle` (§3.4) |
| Line numbers | **keep** | Free |
| Find (`Ctrl+F`) | **keep, improved** | ycode's `find_next` is forward-only substring with **no highlighting**, and mixes byte offsets from `str::find` with char-based `cursor_col`, so matches on non-ASCII lines land wrong. `@codemirror/search` is correct and adds match highlighting and find-previous |
| Goto line (`Ctrl+G`) | **keep** | `@codemirror/search`'s `gotoLine` |
| **Replace** | **add** | ycode has none |
| **Selection, cut/copy/paste** | **add** | ycode has **neither** — no anchor state, no clipboard dep |
| **Horizontal scrolling** | **add** | ycode's `scroll_col` exists but is never advanced; long lines run off the edge |
| Sidebar file tree (`Ctrl+B`), flat model with depth, `..` re-root, hidden toggle preserving expansion | **drop** | It is a file tree inside an editor inside a multiplexer that has a file dock and files panes. `sidebar.rs` is 451 lines duplicating §2 |
| Exit dialog (Save / Quit / Cancel) | **keep, reshaped** | Becomes the unsaved-changes close guard (§3.5), which must cover more paths than ycode's single exit |
| Switch prompt (dirty buffer + open another file) | **keep** | Same guard |
| Markdown preview (`Alt+M`, pulldown-cmark → ratatui lines, ASCII box-drawing substitutes for Korean-font width bugs) | **defer** | 377 lines. A GUI has no cell-width problem, so the port is a different program: a real Markdown renderer. Worth doing; not v1 |
| Command mode (`:w`, `:q`, `:goto`, `:find`) | **drop** | Reachable only via `Ctrl+F`/`Ctrl+G` (there is no `:` binding). Its four commands become buttons and shortcuts |
| Svelte grammar bundled as `.sublime-syntax` | **drop** | Lezer has no Svelte grammar in the core set; `lang-html` covers it adequately. Note the loss |
| Multi-cursor, bracket matching, auto-indent, code folding | **add (free)** | CodeMirror basics ycode never had |
| LSP, autocomplete, formatting, diagnostics | **out of scope** | §9 |

**Honest summary of what a v1 editor pane will not have:** no markdown
preview, no LSP/autocomplete/formatting, no multi-file search, no Svelte
grammar, no split-view diff, no file tree of its own. It *gains* selection,
clipboard, replace, horizontal scrolling, multi-cursor, real undo coalescing,
correct find on non-ASCII lines, and CRLF/BOM preservation.

### 3.2 Editor library — **CodeMirror 6**

Recommended over Monaco, over Ace, and over a hand-rolled
`<textarea>` + highlighter overlay.

The three reasons below are ones this repo's own files establish. The IME
argument comes after them, because it is the weakest-evidenced and the spec
should not pretend otherwise.

**1. CSP.** The policy is `script-src 'self'`, no `worker-src`, no `blob:`
(`tauri.conf.json:27`), in an app that also hosts remote iframes and child
webviews (§1.5). Monaco's language services want web workers, commonly from
blob URLs; adopting it means widening script sources in exactly the app that
should not widen them. CodeMirror 6 core needs no workers and runs under the
policy as written. **Verified from this repo's config, not from recall.**

**2. Headless testability.** CM6's `EditorState` / `Transaction` model is
separable from `EditorView` and runs in node with no DOM — which is what makes
§7's "no jsdom" recommendation viable. Monaco's editor model is not separable
from its view. In a repo with 217 frontend tests and no DOM environment, this
is a structural fit, not a preference.

**3. Bundle cost.** Rough, and **to be measured as the first task of this
step** (see §5 step 3):

| | raw | gzip |
|---|---|---|
| CM6 core (`state`, `view`, `commands`, `language`, `search`, `@lezer/highlight`) | ~350–450 KB | ~110–140 KB |
| 8 `@codemirror/lang-*` grammars | ~20–60 KB each | ~200–300 KB total |
| **CM6 total** | **~0.6–0.9 MB** | **~0.3–0.45 MB** |
| Monaco (`monaco-editor`) | ~5 MB + worker chunks | ~1.2 MB+ |

Against an MSI that already carries an embedded WebView2 bootstrapper and five
Rust binaries, neither is fatal — but the editor chunk is **lazy-loaded**
(dynamic `import()` on first editor pane) either way, so it costs nothing for
users who never open one. CM6's tree-shaking makes that chunk genuinely small;
Monaco's does not.

**4. Large files.** Neither library makes an 8 MB file pleasant. CM6
viewport-renders and keeps a rope-like `Text` document, so scrolling stays
sane where a naive `Vec<String>` does not; Monaco is comparable. The real
answer is §1.2's `MAX_EDIT_BYTES` cap with a read-only fallback, which is
library-independent. Worth noting the floor this raises: ycode re-highlights
from line 0 **every frame** and clones the whole line vector **every keystroke**
— any of these libraries is a large improvement.

**5. IME / Hangul — a reason to prefer CM6, but not a proven one.** This app
has a documented history here. `src/terminal/ime.ts` is 276 lines whose header
explains that under WKWebView (Tauri's macOS webview — a shipping platform) the
Hangul IME fires **no composition events at all**; it edits xterm's hidden
`<textarea>` through `input` events with `inputType: "insertReplacementText"`,
and xterm — which forwards only `insertText` — drops every syllable-completing
replacement, so `안녕하세요` arrives as `ㅇㄴㅎ세요`.

CM6 renders into a `contenteditable` and reads `beforeinput` plus native
composition; Monaco uses a hidden textarea. **What is *not* established:** that
Monaco's textarea handling fails the same way xterm's does (Monaco diffs
textarea state rather than filtering on `inputType`, which is a materially
different strategy), or that CM6's contenteditable path is correct for Hangul
under WKWebView. Neither claim has been tested here.

So this is a preference for the input model that has not already burned this
codebase, not evidence that it works. **It is converted into evidence by a
spike**: alongside the bundle measurement, step 3's first task is to mount a
bare CM6 view in a `tauri dev` build **on macOS** and type `ime.test.ts`'s
Hangul fixture strings. If syllables drop, the fallback is to port `ime.ts`'s
textarea-mirroring approach to CM6's input handler — which is tractable
precisely because that module already exists and is tested — and if that fails,
to re-open this choice before any pane code is written.

**6. Theming against `crates/ytheme`.** `ytheme::Theme` is already mirrored in
TypeScript at `src/settings/types.ts` and served by
`settings::load_syntax_theme`. The 8 `SyntaxColors` fields map one-to-one onto
a `HighlightStyle` over `@lezer/highlight` tags:

```
keyword     → tags.keyword, tags.controlKeyword, tags.moduleKeyword
string      → tags.string, tags.special(tags.string)
comment     → tags.comment
number      → tags.number, tags.bool, tags.null
function    → tags.function(tags.variableName), tags.function(tags.propertyName)
type_name   → tags.typeName, tags.className, tags.namespace
variable    → tags.variableName, tags.propertyName
punctuation → tags.punctuation, tags.bracket, tags.operator
```

and the 11 chrome colours (`bg`, `bg_alt`, `bg_hover`, `fg`, `fg_muted`,
`accent`, `border`, `status_*`) map to a CM6 `EditorView.theme({...})`. This is
a direct mapping with no adapter layer, and it is the first time the frontend
consumes ytheme for anything other than the settings editor — which is a
**bonus**: today only `ycode` actually calls `ytheme::load_theme()`, so a user
who themes ymux sees it applied to one TUI and nothing else.

**7. Licence.** CodeMirror 6 is MIT. Monaco is also MIT. Licence does not
discriminate here and is listed only to close the question.

**Rejected alternatives.** *Monaco* — the textarea IME model this codebase has
already been burned by, plus workers/CSP and ~4× the bundle; its wins (LSP,
IntelliSense) are all out of scope (§9). *Ace* — older architecture, CSS-token
theming that fits ytheme worse, textarea input by default. *`<textarea>` +
Shiki/Prism overlay* — cheapest and the IME path is the browser's own, but it
gives up undo granularity, multi-cursor, viewport rendering and find-highlight,
and the overlay-alignment problem with mixed-width CJK text is precisely the
class of bug `ui.rs:visual_column` existed to fix. Not worth re-fighting.

### 3.3 Grammars in v1

`rust`, `javascript` (covers TS/JSX/TSX), `json`, `html` (covers Svelte/Vue
markup), `css`, `markdown`, `python`, `yaml`. Plus plain text. Each is a
separate dynamic import keyed by extension, so opening a `.rs` file does not
load the Python grammar. syntect's ~200 languages are not matched and that is
recorded as a loss; the long tail is additive later.

### 3.4 Components

- `src/editor/EditorPane.ts` — implements `Pane`. Owns the lazy CM6 import,
  the toolbar (save, undo/redo, find), the dirty indicator, the read-only and
  conflict banners.
- `src/editor/editorModel.ts` — **pure**. `dirtyState(savedStamp, currentStamp)`,
  `closeDecision(dirty, hasPath)` → `save | discard | cancel` options,
  `conflictDecision(diskStamp, expectedStamp, dirty)` → `reload | keep | merge-prompt`,
  `languageForPath(path)`.
- `src/editor/theme.ts` — **pure**. `YTheme` → `{ theme, highlightStyle }`.
- `src/editor/eol.ts` — **pure**. The TS half of §1.2's EOL model.

### 3.5 Unsaved changes — the close guard

`Pane.dispose()` has **no veto** (`src/layout/Pane.ts`). Every one of these
paths currently destroys a pane without asking, and each would silently drop
edits:

| Path | Today | Required |
|---|---|---|
| `Ctrl+Shift+W` close tab / pane | `dispose(true)` | prompt |
| Close workspace | `dispose(true)` for every pane | prompt, once, listing dirty files |
| Viewer-tab reuse (`respawnViewer`, `WorkspaceManager.ts:914`) | kills and respawns under the same id | prompt |
| App quit | `dispose(false)` on shutdown | prompt, and cancel the quit |
| Window close button | Tauri close request | prompt |

The `Pane` interface gains:

```ts
/// Resolves false to cancel the close. Default implementation returns true;
/// only EditorPane overrides it. Callers MUST await it before dispose().
canClose?(): Promise<boolean>;
```

Call sites to update: `WorkspaceManager.closeFocused`, `closeTab`,
`deleteWorkspace`, `respawnViewer`, and a Tauri `on_window_event`
`CloseRequested` handler in `main.rs` that asks the frontend first.

**Belt and braces:** the editor writes a draft of unsaved content to disk on a
2 s debounce, keyed by pane id, next to the scrollback files
(`src-tauri/src/scrollback.rs`'s directory). `dispose(permanent = true)` deletes
it; `spawn()` offers to restore it. This is the same lifecycle rule
`TerminalPane` already uses for scrollback, so it needs no new concept. It is a
safety net, not a feature: **there is no autosave to the real file.**

### 3.6 External modification — the agent problem

This is a terminal for coding agents. Claude Code *will* rewrite the file the
editor has open, while it is open. That is the normal case, not an edge case.

- Every `fs_write_text` carries `expect: Some(ContentStamp)`. A mismatch
  returns `YmuxError::Conflict` and the editor shows *Reload / Overwrite /
  Save as copy* — it never silently wins.
- The editor re-`fs_stat`s on window focus and on pane focus. A changed stamp
  with a clean buffer reloads silently and preserves the cursor line; with a
  dirty buffer it shows a banner.
- A file watcher is deferred (§9) — focus-based polling covers the agent case
  without adding `notify` and a whole watch lifecycle.

### 3.7 The `open-file` viewer-tab flow

Today: yDir presses Enter → `IpcMessage::Event { kind: "open-file" }` →
`ipc_server.rs` emits `ymux:open-file` → `FileDock`'s `onOpenFile` →
`WorkspaceManager.openFileInViewerTab(path)` → a tab running
`["ycode", path]`, reused by killing and respawning the PTY
(`respawnViewer`).

After: `FilesPane` calls `manager.openFileInViewerTab(path)` **directly** — a
function call, not an IPC round-trip. The viewer-tab *policy* is unchanged and
`src/workspace/viewerTab.ts`'s pure `viewerTabAction` keeps its test. Reuse
becomes `editorPane.openFile(path)` — no kill, no respawn, no
`deleteScrollback`, and the two carefully-serialised awaits in `respawnViewer`
(`WorkspaceManager.ts:922-934`) disappear along with the race they guard.

Two details the current wiring depends on and that the port must preserve:

- **Which group the file opens into.** `openFileInViewerTab` targets
  `this.activePaneId()`'s group. A files pane **in the layout** is itself the
  active pane, so its Enter would open the file in its *own* group — which is
  what a user wants (the editor appears as a tab beside the file list). A files
  pane **in the dock** is not in any layout tree, so the target must remain the
  last active layout pane. The rule: `FilesPane` takes an
  `openTarget: "self" | "lastActive"` option, set by its host.
- **The dock's `FilesPane` must not set `focusedPaneId`.** Today the dock's
  pane is a `TerminalPane` outside the tree and the manager's
  "pane active before the dock took focus" logic depends on the dock never
  claiming focus ownership. A GUI pane that wires `onFocus` the way
  `createPane` does for layout panes (`WorkspaceManager.ts:477`) would silently
  break which tab the file lands in. The dock constructs its `FilesPane`
  without an `onFocus` callback.

Deleted here: `crates/yipc`'s `OPEN_FILE_KIND`, `open_file_event`,
`open_file_path` and their tests; `ipc_server.rs`'s `OPEN_FILE_EVENT` branch;
`src/ipc/bridge.ts`'s `onOpenFile`; `FileDock`'s listener;
`tools/ydir/src/dock.rs`'s `open_file_link` and the `"ydir-openfile"` tool name.

### 3.8 Tests

- **vitest, pure:** `editorModel` — dirty transitions incl. undo-back-to-saved
  (the bug ycode has); `closeDecision` for dirty/clean × titled/untitled;
  `conflictDecision` for all four stamp/dirty combinations; `languageForPath`
  on `.rs`, `.tsx`, `.d.ts`, no extension, `.RS`, dotfiles. `theme` — the 8
  syntax colours land on the right tags; a malformed hex falls back instead of
  throwing (ycode's `hex()` **panics deliberately** on bad input — the GUI must
  not). `eol` — mirrors the Rust cases.
- **vitest, headless CM6:** `EditorState` and `Transaction` run in node with no
  DOM. Cover: undo coalescing across a typing burst; the history's saved
  generation; find/replace over a Hangul line (the byte-vs-char bug ycode has);
  the keymap resolving to the right commands. This is the lever that makes the
  editor testable without a DOM environment (§7).
- **Rust:** covered in §1.7.
- **Manual (GUI), Hangul specifically:** type `안녕하세요` on macOS and on
  Windows and confirm every syllable lands — reuse `src/terminal/ime.test.ts`'s
  fixture strings; compose into a selection; compose at a line end; undo across
  a composition; paste mixed CJK/ASCII; cursor position on a mixed-width line.
  Plus: save a CRLF file and diff it; open an 8 MB file; have Claude rewrite the
  open file and confirm the conflict banner; close a dirty tab; quit with a
  dirty tab.

---

## 4. Git pane

### 4.1 Git pane (`ygit` → `PaneKind::Git`)

`tools/ygit/src` is 615 lines and 18 tests, and is the thinnest of the three:
three git shell-outs, two panels, six keys.

| Feature | v1 | Note |
|---|---|---|
| Commit log | **keep** | From `git_log`'s machine format, not the ASCII graph |
| ASCII graph lane colouring (`graph.rs`, 140 lines) | **replace** | Redrawn from `parents` as SVG/CSS lanes. `graph.rs`'s 6-colour lane palette is kept as the visual spec |
| Branch list, current branch marked | **keep** | Plus remote branches, which `git branch` (no `-a`) omits today |
| `Enter` to checkout | **keep** | With a confirm, and an error surfaced properly rather than as a red status line |
| `r` refresh, `Tab` focus switch | **keep** | Refresh also on window focus |
| Worktree add / remove / list | **add** | `ygit` has none — it already lives in `src-tauri/src/git/mod.rs` and is exposed to the frontend, but has **no GUI**. The pane is where it belongs, replacing `src/workspace/WorktreeModal.ts`'s 17-line stub |
| Repo discovery from cwd | **add** | `ygit` runs git in the process cwd with no `repo_root` walk |
| Commit detail / diff, stage, commit, push/pull, stash, tags, blame | **defer** | `ygit` has none. Diff view is the obvious v2 |

Pane follows the active pane's cwd the same way the dock does (reusing
`cwdFollow`), with a pin toggle.

- `src/git/GitPane.ts` — implements `Pane`.
- `src/git/graphLanes.ts` — **pure**. `assignLanes(commits) -> LaneLayout`,
  ported from `graph.rs`'s model (lane index → colour, 6-colour cycle) but
  computed from parent hashes instead of parsed from drawn characters.

### 4.3 Tests

- **vitest, pure:** `graphLanes` — a linear history, a merge, an octopus merge,
  a branch that reappears after a gap, an empty log; lane colours cycle at 6.
- **Rust:** in §1.7.
- **Manual (GUI):** checkout from the git pane and confirm terminal panes in
  that repo see the new branch; worktree add then remove; a detached HEAD; a
  non-repo cwd.

---

## 5. Migration order and cut-over

Seven steps. **Every step leaves the app shipping-ready**, and no step deletes
something its own replacement does not yet cover.

### Step 1 — Backend surface

§1 in full. New: `fsx.rs`, `textfile.rs`, `fsops.rs`, `git/mod.rs` additions,
`error.rs` variants, the webview-label guard, i18n keys. Registered in
`generate_handler!` and the `default` capability. `sysmonitor.rs` is untouched.

**Deleted:** nothing. **Verified:** `cargo check --no-default-features --lib --tests -p ymux`
passes; the §1.5 Tauri-reachability question is answered in writing.

### Step 2 — Files pane + dock cut-over

§2 in full, including §2.4.

**Deleted:** `dockArgv` + its test; `FileDock`'s PTY lifecycle and
`DOCK_PANE_ID`; `bridge.ts:fileDockChangeDir`; `ipc_server.rs:filedock_change_dir`;
`yipc::IpcMessage::ChangeDir`; `IpcServer::send_to` + its client registry + the
`send_to` fan-out and timeout tests; `tools/ydir/src/dock.rs`'s `follow_host`
and `PendingDir`.

**Not deleted:** `tools/ydir` itself — `ydir` is still on PATH and in the tool
menu. Users keep the CLI for one more release while the pane proves itself.

### Step 3 — Editor pane

**Three tasks before any pane code is written**, each of which can change the
decision:

1. **Measure the bundle.** `pnpm add` the CM6 packages, build, record real chunk
   sizes against §3.2's estimates. Over ~600 KB gzip → trim grammars.
2. **Hangul spike on macOS** (§3.2 reason 5). A bare CM6 view in `tauri dev`,
   typing `ime.test.ts`'s fixture strings. A failure here has a stated
   fallback; discovering it after three panes are built does not.
3. **Enumerate the keymap collisions** (§0.6) against `HelpOverlay.ts`'s
   `SHORTCUTS`, and settle the final table.

Then §3 in full, including the `canClose` guard (§3.5) and the direct
`openFileInViewerTab` call (§3.7).

**Deleted:** `yipc::OPEN_FILE_KIND` / `open_file_event` / `open_file_path` +
tests; `ipc_server.rs`'s `OPEN_FILE_EVENT` branch; `bridge.ts:onOpenFile`;
`FileDock`'s open-file listener; `WorkspaceManager.respawnViewer`'s kill/
respawn/`deleteScrollback` sequence; `tools/ydir/src/dock.rs:open_file_link`
and the `"ydir-openfile"` tool name.

**yipc after this step** is reduced to: the server, `Hello`, `Event`, `Ack`,
`AGENT_HOOK_KIND`. `RegisterCommands` and `PaneSend` are already unused by every
tool and go too. `IpcClient` survives for the hook relay.

### Step 4 — Git pane

§4 in full. **Deleted:** `src/workspace/WorktreeModal.ts` (the 17-line stub the
git pane replaces).

### Step 5 — The hook relay *(ships as its own release)*

> **Amended at implementation (2026-09-24): `y` is kept, not replaced.** The
> user wanted steps 5 and 6 shipped in one push, and the two-release plan below
> exists only because the hook binary's *path* changes. So the path does not
> change: the binary stays `y`, the sidecar stays `binaries/y`, and
> `tools/ylauncher` shrinks to the hook relay alone — `agent_hook.rs` untouched
> (same 300 ms bounded `Ack` wait; prints nothing, always exits 0, returns at
> once without `YMUX_PANE_ID`/`YMUX_IPC`; same unit and `CARGO_BIN_EXE_y`
> integration tests), with the launcher's tool table, PATH scanning, help and
> `--version` deleted, and its unused `ytheme` dependency dropped. Any
> invocation other than `y agent-hook …` prints one usage line on stderr and
> exits 2 (tested). Because every tracking user's `settings.json` already
> points at `…/y`, there is nothing to migrate: `agent_hooks.rs` is unchanged,
> `y_sidecar_path()` keeps resolving `y{.exe}` beside the running executable,
> and `install_refreshes_the_y_path` plus the marker logic hold as they are. No
> `tools/yhook`, no `ymux-hook`, no interim release carrying both binaries.
> Cost: a crate named `ylauncher` that no longer launches anything — cosmetic,
> and cheaper than a path migration. The original plan follows for the record.

The blocker on deleting `y`. `agent_hooks::hook_command` (`agent_hooks.rs:36`)
writes `"<abs path>/y" agent-hook claude --ymux-agent-hook` into every tracking
user's `~/.claude/settings.json`. Delete `y` and those users' Claude Code
invokes a missing binary on **eleven hook events per session**.

- New crate `tools/yhook`, binary `ymux-hook`. It is `tools/ylauncher/src/agent_hook.rs`
  moved verbatim — same 300 ms bounded `Ack` wait, same "prints nothing, always
  exits 0, returns immediately without `YMUX_PANE_ID`/`YMUX_IPC`" contract, same
  three unit tests and three `CARGO_BIN_EXE_` integration tests. Deps: `yipc`,
  `serde_json`. No `ytheme` (which ylauncher declares and never uses).
- `agent_hooks::y_sidecar_path()` → `hook_sidecar_path()`, resolving
  `ymux-hook{.exe}` beside the running executable.
- **The `--ymux-agent-hook` marker is unchanged** (rule 12). That is what makes
  this safe: `install_hooks` refreshes the command of any entry carrying the
  marker in place (`install_refreshes_the_y_path`), so a tracking user's
  settings.json is rewritten to the new binary automatically on next launch,
  with foreign hooks and key order preserved. No duplicate entry is appended.
- **Both `y` and `ymux-hook` are bundled in this release.** A user who skips a
  version still gets their path refreshed when they land on any release that has
  both.

**Deleted:** nothing yet.

**Alternatives rejected.** *`ymux.exe agent-hook`* — on Windows `ymux` is a GUI-
subsystem binary: launched from a console it detaches, has no usable stdin for
the hook JSON, and would pay Tauri/WebView startup per event. *A shell snippet
writing the socket directly* — no portable way to do a bounded Ack wait, and it
would have to be re-derived for cmd, PowerShell, bash and zsh.

### Step 6 — Cut-over *(one commit; rule 4 makes it atomic)*

> **Amended with Step 5:** read `ymux-hook` below as `y` and `yhook` as
> `ylauncher`. `tools/ylauncher/` is **kept** (as the hook relay), so
> `externalBin` stays `["binaries/y"]`, `build-tools.mjs` stays
> `[{ pkg: "ylauncher", bin: "y" }]`, the CI dummy loop is `for tool in y`,
> and the `-p` lists keep `ylauncher`.

Rule 4: Tauri's build script validates `externalBin` paths even during
`cargo check`, so `tauri.conf.json`, `build-tools.mjs` and the CI dummy loop
must move together or CI breaks.

**Deleted — Rust:**
- `tools/ydir/`, `tools/ycode/`, `tools/ygit/`, `tools/ymon/`, `tools/ylauncher/`
- `crates/yversion/` (its only consumers were the four TUI footers; ylauncher
  never used it and reported its own crate version instead)
- Those members from the workspace `Cargo.toml`
- `src-tauri/src/pty/manager.rs` → `path_with_sidecar_dir`, `sidecar_dir`,
  `sidecar_path_entry`, `direct_profile` and their tests. All four exist so a
  pane could type `ydir`; `ymux-hook` is resolved by `agent_hooks` directly
- `PtyManager`'s `PATH` injection call site in `main.rs`

**Deleted — frontend:**
- `TOOL_MENU` (`WorkspaceManager.ts:69`) and its context-menu entries (`:717`)
- `TerminalPane`'s `argv` option and `createPane`'s `argv` parameter — the
  viewer tab and the dock were its only callers, and both are gone
- The tool lists in `src/help/HelpOverlay.ts:42-45` and
  `src/settings/SettingsOverlay.ts:48-51`, plus the `help.toolY*` i18n keys
  (13 languages each)
- The `"tools"` settings section if nothing else populates it

**Changed — packaging:**
- `src-tauri/tauri.conf.json` → `"externalBin": ["binaries/ymux-hook"]`
- `scripts/build-tools.mjs` → `TOOLS = [{ pkg: "yhook", bin: "ymux-hook" }]`;
  update the header comment
- `.github/workflows/release.yml` → the dummy loop (`:91`) becomes
  `for tool in ymux-hook`; the clippy (`:61`) and test (`:67`) `-p` lists drop
  `ymon ydir ycode ylauncher ygit` and gain `yhook`
- `scripts/test.sh` → the same two `-p` lists

**Kept, deliberately:**
- **`crates/ytheme`** — `src-tauri/src/settings.rs` uses it, and the editor pane
  now consumes it (§3.2). It gains consumers, it does not lose them.
- **`crates/ypath`** — `pty::osc7::CwdChange`, `git/mod.rs`'s worktree tests and
  now `fsx::same_file` (rule 15).
- **`crates/yipc`** — reduced to server + `Hello`/`Event`/`Ack` +
  `AGENT_HOOK_KIND` + `IpcClient`, for `ymux-hook`. It does **not** die.
- **`src-tauri/wix/path-env.wxs` and `componentGroupRefs: ["PathEnvGroup"]`.**
  It is tempting to drop these now that nothing is meant to be typed — but the
  fragment also puts **`ymux.exe` itself** on PATH, which is independently
  useful and is not part of what the user agreed to lose. Removing an MSI
  `Environment` component across a major upgrade is its own regression surface,
  and rule 9's history says the installer is where this project gets bitten.
  **Only the fragment's comment changes** (it names ymon/ydir/ycode/y). If it is
  later wanted gone, it is one fragment file plus one `tauri.conf.json` key.
- **`src-tauri/wix/main.wxs`** — untouched. Rule 9 permits exactly one deviation
  from the stock template and this change does not add a second.

### Step 7 — Docs and release

- `CLAUDE.md`: **rule 4** (sidecar list → one entry), **rule 5** (drop the
  `crates/yversion` line from the version checklist), **rule 13** (delete — it
  documents the `send_to` fan-out that no longer exists), the **project
  structure tree** (drop `tools/`, `crates/yversion`; add `src/files/`,
  `src/editor/`, `src/git/`), and the **test-count table**
  (re-derive; do not guess).
- `README.md` / `README.ko.md` / `README.ja.md`: remove the CLI tool sections
  and any "available from any terminal" claim; document the panes; update the
  keyboard tables (rule 6).
- Version bump per rule 5 — **now four files, not five** (`yversion` is gone).
- Release notes must state plainly: **the `ydir` / `ycode` / `ygit` / `ymon`
  commands no longer exist**, and `y` survives only as the Claude Code hook
  relay (`y mon`, `y code …` etc. are gone).

---

## 6. What breaks for the user

| Today | After | Where |
|---|---|---|
| The file dock runs `ydir --dock` in a PTY, driven by `ChangeDir` over yipc | The dock hosts a `FilesPane`. Same position, collapse, width, `Ctrl+Shift+E`, cwd-following (same `cwdFollow` debounce). Gains rename/create/multi-select/trash; loses the run dialog and the dual-pane toggle (`Tab` now switches preview, as it already does in dock mode) | §2.4 |
| Pane right-click → yDir / yMon / yCode / yGit types the command into the shell | The same three surviving entries **open panes**: "Files here" / "Editor" / "Git", each splitting the focused pane and inheriting its cwd. yMon's entry is removed — no pane replaces it. Plus palette commands and `Ctrl+Shift+…` bindings through rule 6's full 6-step checklist | §5 step 6 |
| yDir Enter on a file → `open-file` over yipc → viewer tab running `ycode <path>` | `FilesPane` calls `openFileInViewerTab` directly; the viewer tab is an `EditorPane`. Reuse is `openFile(path)` — no PTY kill/respawn, so switching files no longer flashes a terminal clear (see the ConPTY memory note) | §3.7 |
| `ydir` / `ycode` / `ygit` / `ymon` / `y` typed in any terminal, anywhere | **Gone.** The accepted loss. Release notes say so plainly | §5 step 6 |
| `ymon`'s TUI (overview/CPU/memory/process tabs) | **Gone, with no in-app replacement.** The status bar already shows the same metrics from the same `sysmonitor.rs` (CPU, RAM, network, disk, GPU); the process list is better served by the OS task manager | §5 step 6 |
| `y mon` / `y code x.rs` shorthand | Gone with the launcher | §5 step 6 |
| A hand-written `startup_cmd` or `HotKeyDef` running `ycode foo.rs` | Breaks. Not auto-migrated — the strings are arbitrary user data. Release notes call this out | §0.3 |
| A viewer tab or dock left open at shutdown | Already survives as an ordinary terminal pane today (`argv` is never persisted), so nothing changes at upgrade: it reloads as a shell tab, exactly as it does now. The *next* Enter from the files pane opens a real editor pane | §0.3 |
| Claude Code hooks pointing at `…/y agent-hook claude --ymux-agent-hook` | Unchanged — `y` survives as the hook relay at the same path, so nothing is rewritten (Step 5 amendment) | §5 step 5 |
| `%APPDATA%\ymux\theme.toml` themed only ycode's syntax colours | Now themes the editor pane | §3.2 |
| Install dir on PATH | Unchanged — `ymux.exe` stays reachable | §5 step 6 |
| Markdown preview (`Alt+M`) | Gone in v1; deferred, not dropped | §3.1 |
| ycode's editor sidebar (`Ctrl+B`) | Gone — the file dock and files panes replace it | §3.1 |

**Downgrade.** A config containing `pane_kind = "files"` fails
`toml::from_str::<Config>` in an older ymux. `main.rs:20` catches that and falls
back to a store at `temp_dir()/ymux-fallback.toml`, so **the real config file is
not overwritten** — but that session shows default layouts. Upgrading again
restores everything. Worth a release-note line; not fixable for binaries already
shipped.

---

## 7. Testing strategy

### 7.1 What is pure and unit-testable — the large majority

**Rust (Linux-safe, `--no-default-features`):** `fsx` (sort, hidden, binary
sniff, `ypath`-backed comparison), `textfile` (EOL, BOM, stamps, caps),
`git::parse_log_porcelain` + `parse_branch_list`.
This is where the *rules* live, and it is the same ungated-pure-logic pattern
`agents.rs` and `agent_hooks.rs` already use.

**vitest, no DOM:** `fileModel`, `preview`, `editorModel`, `theme`, `eol`,
`graphLanes` — every one written DOM-free by construction, matching the
discipline that already produced 217 frontend tests in a vitest with **no**
environment configured.

**vitest, headless CodeMirror:** `EditorState`, `Transaction`, `EditorSelection`
and every `@codemirror/commands` and `@codemirror/search` command operate on
state, not on a view, and run in node. Undo coalescing, the saved generation,
find/replace over Hangul, keymap resolution and indentation are all covered
without a DOM. **This is why the library choice and the testing strategy are the
same decision** — it is the single biggest reason CM6 is cheaper to verify than
Monaco, whose editor model is not separable from its view.

### 7.2 What genuinely needs a real GUI

Pane mounting and `scheduleFit` after un-hiding (rule 14); focus routing between
panes; **IME composition**; CM6 view measurement and scroll; context menus;
drag-to-resize; the close guard's modal flow; anything involving actual layout.

### 7.3 Recommendation: **do not add jsdom**

Against, and these are decisive:

1. **It cannot test the thing most at risk.** jsdom has no IME, no real
   composition events and no `beforeinput` semantics. §8's risk 4 — a Hangul
   regression on WKWebView — is exactly what a DOM env would appear to cover and
   would not. A green jsdom suite here is worse than no suite: it is false
   confidence about the one bug class this codebase has already shipped.
2. **It cannot test CM6's view layer either.** CM6 measures DOM geometry for
   viewport rendering and cursor placement; jsdom reports zero for all of it.
   CodeMirror's own guidance is to test views in a real browser.
3. **No layout means no rule 14.** The pane-visibility/refit bugs that have
   repeatedly bitten this project (`.xterm-viewport` scrollTop, tab un-hide)
   are layout bugs. jsdom has no layout.
4. **It splits the test environment.** Today every frontend test runs in one
   node environment. Adding a second means per-file directives, two sets of
   assumptions, and a class of "passes in jsdom, fails in WebView2" bug the
   project does not have today.

**Instead:**

- **Keep the pure-model discipline.** Every new module gets its `.test.ts`
  beside it. The 8 pure modules listed in §7.1 are the design working.
- **Add a Rust integration layer.** `tempfile`-backed desktop tests for the fs
  and git commands (§1.7). This is new for the project and buys more than jsdom
  would, because the filesystem semantics are where real data loss lives.
- **Keep and extend the manual GUI checklist.** The repo already does this
  (spec §3's "Manual (GUI)" list). The three panes each get one, with the Hangul
  checklist in §3.8 marked as **must run on macOS** — the same instruction rule
  10 already gives for `macos_shell_integration_reports_live_cwd`.
- **If browser-level testing is wanted later**, the right tool is
  `vitest --browser` with the Playwright provider (a real Chromium, real layout,
  real composition events) pointed at a handful of view-mounting tests — not
  jsdom. That is a separate decision, deliberately deferred until after
  cut-over, and it should be triggered by a *second* shipped DOM-level
  regression, not by this spec.

---

## 8. Risks

### 1. Silent data loss in the editor — **highest**

Three independent paths: `Pane.dispose()` has no veto so any close discards
edits; an agent rewrites the file underneath an open buffer; the app quits with
a dirty pane. ycode's own model makes two of these worse (it never returns to a
clean state after undo, and `content()` drops the trailing newline and CRLF, so
even a *correct* save silently rewrites the file).

**De-risk:** the `canClose` veto wired into all five close paths (§3.5,
including the Tauri `CloseRequested` handler); stamp-guarded writes that return
`Conflict` rather than clobbering (§3.6); reload-on-focus; debounced local
drafts with scrollback-style lifecycle; **no autosave to the real file**; and
EOL/BOM/trailing-newline preservation so saving an untouched file is a no-op
diff. Build the guard in the same commit as the first save button, not after.

### 2. Arbitrary-filesystem commands reachable from remote web content

`browser-children.json` grants IPC to `eb-*` child webviews loading
`http://**` / `https://**`, and `browser` panes are remote iframes inside the
`main` webview under `frame-src http: https:`. **And `build.rs` has no
`AppManifest`, so ymux's own commands sit outside the capability system
entirely** — there is no declarative gate to lean on. Adding
read/write/delete-anything commands here is the largest attack-surface change
the app has made.

**De-risk:** a single `guard_local(&webview)?` on the first line of every new
command, checking label **and origin** (§1.5) — the origin half is what covers
the iframe case that a label check misses; empirical verification from both a
`browser` pane and an `eb-*` webview **before step 1 is called done**, recorded
in the commit message; Rust tests for accept and both reject paths; never add
ymux's commands to `browser-children.json`; do not widen the CSP — which
§3.2's library choice makes unnecessary.

### 3. Scope — three panes replacing 7,200 lines at once

The realistic failure is a half-finished branch that cannot ship, or a v1 that
is worse than the TUIs it replaced and gets reverted.

**De-risk:** the seven-step order in §5, every step shippable. Specifically:
ship Files (step 2) and **let it live through a release** before starting
Editor; keep `tools/ydir` and friends on disk and on PATH until step 6, so the
fallback is always one right-click away; treat step 6 as a separate commit
whose only job is deletion. If a pane is not good enough, the step before it
still shipped.

### 4. IME / Hangul regression in the editor

`src/terminal/ime.ts` exists because this exact bug shipped. A new text-input
surface on WKWebView is a fresh chance to ship it again, and §7.3 concedes no
automated test can catch it.

**De-risk:** choose the input model that is not the one that broke
(contenteditable, not hidden textarea — §3.2); make the Hangul manual checklist
a **release blocker** run on macOS, reusing `ime.test.ts`'s fixture strings; if
it does regress, that is the trigger for the Playwright-backed view tests §7.3
defers, not for a jsdom retrofit.

### 5. Losing the Claude Code hook relay

`y` is load-bearing for the agent tree, and its absolute path sits in users'
`settings.json`. Deleting it in the same release that adds `ymux-hook` leaves
anyone who upgrades non-consecutively with broken hooks.

**De-risk:** step 5 ships `ymux-hook` **alongside** `y` as its own release, and
only step 6 removes `y`. The `--ymux-agent-hook` marker is preserved so
`install_hooks` refreshes existing entries in place (rule 12), and
`install_preserves_foreign_hooks_and_key_order` /
`uninstall_restores_foreign_settings_exactly` run before anything else in that
step — rule 12 says so explicitly.

**As implemented:** retired rather than mitigated — `y` stays at the same path
as the hook relay (Step 5 amendment), so no user's `settings.json` changes and
there is no non-consecutive-upgrade window.

### 6. Performance regressions from the new backends

Two candidates: `git log` shelling out on a UI path (`ygit` blocks its
render thread doing this); `fs_list_dir` stat-ing every entry in a huge
directory.

**De-risk:** all git commands `#[tauri::command(async)]`; `fs_list_dir`
paginated above a threshold and its result cached per directory with
focus-based invalidation.

### 7. Bundle size and load time

An editor library in an app whose MSI already carries an embedded WebView2
bootstrapper.

**De-risk:** measure as the literal first task of step 3 and trim grammars
against a stated budget (~600 KB gzip total) before writing pane code;
lazy-load the editor chunk on first editor pane so non-users pay nothing;
per-language dynamic grammar imports.

### 8. Rule-2 field desync

One new `PaneSpec` field, and the project's own documentation calls this its #1
source of bugs — a miss means `file_path` silently vanishes on save/load and the
editor reopens empty.

**De-risk:** one field only (§0.2); the 4 places plus
`panespec_all_fields_roundtrip` in a single commit; a dedicated
`editor_file_path_survives_save_load` test that round-trips through
`nodeToSpec` and `findAndMutatePane`, not only through TOML — TOML round-trip
alone would not have caught the historical misses.

### 9. Feature-parity disappointment

Users who liked ycode's markdown preview, its sidebar, ydir's run dialog or
syntect's 200 languages will notice. §2.1, §3.1 and §4 name every one of these
honestly rather than discovering them in a bug report.

**De-risk:** release notes carry the keep/drop/defer table; "defer" items are
written down here with the reason, so they are a backlog and not an oversight.

---

## 9. Out of scope

LSP, autocomplete, go-to-definition, formatting and diagnostics in the editor
pane. Multi-file search and replace. A diff or merge view. Git staging,
committing, push/pull, stash, tags, blame and rebase. Markdown preview (§3.1 —
deferred, with a reason). Non-UTF-8 encodings (CP949/EUC-KR) and encoding
conversion. A filesystem watcher (focus-based polling instead — §3.6).
Archive browsing, bookmarks and drag-and-drop in the files pane. Any replacement for the
`ydir` / `ycode` / `ygit` / `ymon` / `y` commands outside ymux — that loss is
the premise of this document, not a problem to solve inside it. Removing the MSI
PATH registration (§5 step 6 — kept deliberately). Adding jsdom or any DOM test
environment (§7.3).
