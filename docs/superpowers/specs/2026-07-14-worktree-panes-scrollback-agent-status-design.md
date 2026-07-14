# Design: Worktree Panes, Persistent Scrollback & Agent Status

**Date:** 2026-07-14
**Target version:** v0.8.19
**Status:** Approved for planning

## Motivation

Comparing ymux to [orca](https://github.com/stablyai/orca) (an Electron "Agent
Development Environment" that runs AI coding agents in parallel git worktrees)
surfaced that ymux is already on the same trajectory: v0.8.18 shipped
CLI-done notifications, unlimited workspaces, and pane swap. This spec pushes
ymux further toward being a **lightweight, Windows-native, terminal-first
multi-agent runner** by adding the three highest-fit ideas from orca:

1. **Worktree panes** — open a shell pane rooted in a fresh `git worktree`, so
   multiple agents can work isolated in parallel.
2. **Persistent scrollback** — a terminal's buffer survives an app restart
   (a natural multiplexer feature ymux is currently missing).
3. **Agent status states** — each pane carries a live `idle / running / done /
   attention` status, surfaced on the pane border and workspace tab, so
   parallel agents are legible at a glance.

These three ship together as **v0.8.19**. They share infrastructure (the
status machine reuses the OSC 9 / bell detection added in v0.8.18; worktree
panes benefit from status + scrollback) and are small enough individually that
one plan can land them incrementally.

## Non-goals (explicitly out of scope)

- **Prompt fan-out** (one prompt → N agents across N worktrees). A follow-up
  once single worktree panes are proven.
- **Diff comparison / merge UI** across worktrees. Orca's heavyweight review
  surface; not this release.
- **PTY session detach/attach** (keeping shell processes alive across restart).
  Windows ConPTY makes this very hard; scrollback replay covers the common need.
- **True "waiting-for-input" detection.** No reliable cross-shell signal exists;
  the status machine deliberately omits this state.
- libgit2 / `git2` crate — we shell out to `git` in PATH instead.

---

## Feature 1 — Single Worktree Pane

### Behaviour

From the Command Palette (`Ctrl+Shift+P`), a new command **"Open pane in new
git worktree"**:

1. Reads the **focused pane's cwd** (already tracked via OSC 7 → `CwdMap`) as
   the source repository.
2. If that cwd is not inside a git repo, the command is disabled (greyed in the
   palette) / shows a toast.
3. Prompts via a small input modal, pre-filled with a suggested branch name
   `agent/<shortid>` (6-char id). The user can edit it.
4. Runs `git worktree add <path> <branch>`, where `<path>` defaults to
   `<repo>/../.ymux-worktrees/<branch>` (sibling of the repo — keeps the main
   working tree clean, per git's own recommendation). The base dir is
   configurable via a new config field `worktree_base_dir` (empty = default).
5. Splits a new pane from the focused pane and spawns a fresh shell with
   `cwd = <worktree path>`.

When a worktree-tagged pane is **closed**, a confirm dialog offers to remove the
worktree (`git worktree remove`). Removal is **opt-in** and never deletes the
branch, so commits are never lost. If the worktree is dirty, removal requires a
second explicit confirm (which passes `--force`).

### Backend

New desktop-gated module `src-tauri/src/git/mod.rs`, thin wrappers over the
`git` binary in PATH via `std::process::Command` (no libgit2):

```rust
pub fn is_git_repo(cwd: &Path) -> bool;                    // git rev-parse --is-inside-work-tree
pub fn repo_root(cwd: &Path) -> YmuxResult<PathBuf>;       // git rev-parse --show-toplevel
pub fn worktree_add(repo: &Path, branch: &str, path: &Path) -> YmuxResult<()>;
pub fn worktree_remove(path: &Path, force: bool) -> YmuxResult<()>;
pub fn worktree_list(repo: &Path) -> YmuxResult<Vec<WorktreeEntry>>;
```

- `worktree_add` attaches to an existing branch if it already exists (uses
  `git worktree add <path> <branch>`; falls back to `-b` for a new branch),
  rather than erroring.
- The pure logic (arg construction, output parsing) is **Linux-testable** with a
  temp repo in `#[cfg(test)]`; only the Tauri command layer is desktop-gated.

New Tauri commands in `commands.rs` (desktop): `git_is_repo`,
`git_worktree_add`, `git_worktree_remove`, `git_worktree_list`. Capability
entries added to `src-tauri/capabilities/default.json`.

### PaneSpec field (the #1 source of bugs — sync all 4 places)

Add one field to mark a pane as a worktree pane and enable cleanup on close:

```rust
#[serde(default)]
pub worktree_path: String,   // empty = normal pane; non-empty = worktree root
```

Per CLAUDE.md rule #3, this is a **`String` with `#[serde(default)]`, not
`Option<String>`** (Option inside the tagged `LayoutNode` enum does not
round-trip through TOML). All four sync sites must be updated:

1. `src-tauri/src/config/model.rs` — `PaneSpec` struct + every constructor
2. `src/types.ts` — `PaneSpec` interface
3. `src/layout/LayoutTree.ts` `nodeToSpec()` — manual field copy
4. `src/workspace/WorkspaceManager.ts` `findAndMutatePane()` — snapshot + write-back

Plus a `panespec_worktree_field_roundtrip` test and inclusion in the existing
`panespec_all_fields_roundtrip` test.

`CONFIG_VERSION` is **not** bumped (additive field with serde default).

### Edge cases

- cwd not a git repo → command disabled + toast.
- branch already exists → attach to it (no error).
- worktree path already exists → surface git's error via toast.
- On restart, a persisted worktree pane just re-spawns a shell in the saved
  `worktree_path` (the worktree dir still exists on disk); no auto-recreation.

---

## Feature 2 — Persistent Scrollback

### Behaviour

A pane's terminal buffer survives an app restart. On reopen, prior scrollback is
replayed into the fresh terminal above a localized `── session restored ──`
separator, then the new live shell starts below it.

Because PTY **processes** are not persisted (they die on app close and a fresh
shell spawns in the saved cwd — existing behaviour), this restores *history for
reference*, not a live session. This is called out to the user via the
separator line.

### Architecture (frontend xterm serialization)

- Add dependency **`@xterm/addon-serialize`**.
- `TerminalPane` serializes its buffer:
  - debounced ~2 s after output settles, and
  - once on `beforeunload` / window-close.
- Two new Tauri commands persist blobs per pane:
  `save_scrollback(pane_id, blob)` and `load_scrollback(pane_id) -> Option<String>`,
  writing `scrollback/<pane_id>.txt` under the app config dir (same dir as
  `config.toml`). Keyed by the stable pane `Uuid` that already survives in
  `config.toml`.
- **Restore:** on pane mount, if a blob exists, `term.write(blob + separator)`
  **before** the live PTY data stream is wired, so history sits above live
  output.

### Caps, cleanup & privacy

- Size cap: default last **~256 KB** per pane (configurable). xterm's own 10k
  scrollback bounds the upper limit anyway.
- Settings toggle **"Persist terminal scrollback"** under ⚙ → General
  (on by default), new config field `persist_scrollback: bool`
  (`#[serde(default = true)]`).
- The scrollback file is deleted on **permanent** pane close (kill_pane), not on
  a normal app shutdown.
- Documented: scrollback can contain secrets; the toggle lets privacy-sensitive
  users disable it, and files live only in the local app config dir.

---

## Feature 3 — Agent Status States

### States

Per-pane, frontend-only, **not persisted**:

| State | Meaning | Colour |
|-------|---------|--------|
| `idle` | at a prompt, nothing running | neutral (no highlight) |
| `running` | a command/agent is actively working | blue, subtle pulse |
| `done` | finished while you were watching | green (brief), then idle |
| `attention` | finished/bell while pane was **unseen** | amber, persists until seen |

### State machine

```
idle --(Enter submitted in pane | sustained output)--> running
running --(OSC 9 / bell, pane focused)--> done --> idle
running|idle --(OSC 9 / bell, pane unfocused)--> attention
attention --(pane gains focus)--> idle
running --(no output for N s AND at prompt)--> idle
```

- `done` / `attention` are **solid signals** — they reuse the exact OSC 9 +
  terminal-bell detection added in v0.8.18 (`term.onBell()` +
  `registerOscHandler(9, …)`), including the existing guard that ignores
  Windows-Terminal progress/cwd `OSC 9 ; <digit>;…` payloads.
- `running` is a **heuristic**: set on Enter-submission within the pane or on
  sustained output activity; cleared after N seconds of no output. This is
  documented as best-effort — there is no reliable cross-shell command-boundary
  signal, and OSC 133 semantic marks are not emitted by all shells/agents.
- `attention` is exactly the condition that already fires the v0.8.18 desktop
  notification ("finished while out of sight"), now also reflected as a
  persistent visual state.

### Integration

- New `PaneStatus` enum + a `status` field on the `TerminalPane` runtime object
  (not `PaneSpec` — runtime only, never serialized).
- The v0.8.18 pulsing-border and workspace-tab dot-badge become **one of four
  states** rather than a binary "needs attention" flag. Border colour and tab
  dot colour are driven by `PaneStatus`.
- Surfaced to `WorkspaceBar` via the existing `onWorkspacesChange`-style
  callback mechanism (a new `onPaneStatusChange` callback on
  `WorkspaceManager`).
- Suppression rules from v0.8.18 are preserved: a focused/visible pane goes
  straight `running → done → idle` without ever entering `attention`.

---

## Cross-cutting concerns

### i18n (CLAUDE.md rule #7)

New user-visible strings across all 13 languages:
- Palette command "Open pane in new git worktree"
- Worktree branch input modal (title, placeholder, confirm/cancel)
- Worktree-remove confirm dialog (+ dirty-force variant)
- "── session restored ──" separator text
- Settings toggle "Persist terminal scrollback"
- Any status tooltips ("running", "done", "attention")

### Version bump (CLAUDE.md rule #5) → v0.8.19

Update all six sync points: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`,
`package.json`, `crates/yversion/src/lib.rs`, the three README badge URLs, and
regenerate `Cargo.lock` via `cargo check`.

### Testing

| Area | Test |
|------|------|
| git module | `worktree_add`/`remove`/`list` against a temp repo (Linux-safe) |
| PaneSpec | `panespec_worktree_field_roundtrip` + update `panespec_all_fields_roundtrip` |
| scrollback | `save_scrollback` → `load_scrollback` round-trip (empty + capped) |
| status | state-machine transition table unit tests (idle→running→done, unfocused→attention, focus→idle) |
| Linux CI | `cargo check --no-default-features --lib --tests -p ymux` still passes |

### Linux CI safety (CLAUDE.md rule #1)

All Tauri-dependent code (`commands.rs` additions, capabilities) stays behind
`#[cfg(feature = "desktop")]`. The `git/mod.rs` command-construction and
output-parsing logic is written to be Linux-testable independent of Tauri.

### Compatibility

Drop-in over v0.8.18. New config fields (`worktree_path`, `worktree_base_dir`,
`persist_scrollback`) are all additive with serde defaults, so existing
`config.toml` files load unchanged and `CONFIG_VERSION` is not bumped. New
palette commands and the status colours introduce no keybinding changes.

## Suggested implementation order

1. **Feature 3 (status states)** — smallest, self-contained, reuses v0.8.18
   detection; no backend or PaneSpec changes.
2. **Feature 2 (scrollback)** — frontend + two simple commands; independent.
3. **Feature 1 (worktree panes)** — largest: new git module, PaneSpec field
   (4-place sync), input modal, cleanup flow. Benefits from 2 & 3 already in
   place so worktree panes get status + scrollback for free.
