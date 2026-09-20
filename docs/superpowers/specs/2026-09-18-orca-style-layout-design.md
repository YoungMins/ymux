# Orca-style layout: agent tree, yDir dock, bottom-anchored prompt

Date: 2026-09-18 · Branch: `claude/orca-style-layout`

Three independent features, inspired by Orca (github.com/stablyai/orca), built
in this order, one commit series each:

1. **Agent tree** — the left workspace panel becomes Workspace › Pane › Agent.
2. **yDir dock** — a collapsible right-side panel running `ydir`, following the
   active pane's cwd.
3. **Bottom-anchored prompt** — when a terminal's content is shorter than the
   pane, it is drawn against the bottom edge, so the prompt sits on the last
   row and output grows upward.

Research note: Orca does *not* bottom-anchor plain shells; what looks like it
is agent TUIs (Claude Code) drawing their own bottom input box. Feature 3 is
ours, applied to every shell.

---

## 1. Agent tree

### Behaviour

```
▾ my-workspace                 (existing row: switch / notes / delete, status tint)
   ▸ pwsh — D:\Git\ymux        (pane row: title or shell name; pane status tint)
      ● claude   working       (lead agent)
         ● Explore  working    (subagent, agent_type label)
         ✓ executor done
   ▸ Git Bash
```

- Every pane of every workspace is listed under its workspace (browser panes
  included, labelled by title/URL). Clicking a pane or agent row switches to the
  workspace and focuses that pane.
- Each workspace row is expandable; expansion state persists in localStorage
  (`ymux.workspaceTree.expanded`). The panel collapse behaviour is unchanged.
- Agent rows show name, status dot and (subagents) `agent_type`.

### Detection

Two sources, merged in a backend `AgentRegistry` keyed by pane id:

**Claude Code hooks (precise, subagents).**
- ymux already injects `YMUX_IPC` into every PTY; it additionally injects
  `YMUX_PANE_ID=<pane id>`.
- New subcommand of the existing `y` sidecar (`tools/ylauncher`):
  `y agent-hook claude`. It reads the hook JSON from stdin. If `YMUX_PANE_ID`
  or `YMUX_IPC` is unset it exits 0 immediately (so Claude outside ymux is
  untouched). Otherwise it sends
  `IpcMessage::Event { kind: "agent-hook", payload }` with
  `payload = { pane_id, agent: "claude", event: hook_event_name, session_id,
  agent_id?, agent_type?, tool_name? }`, waits for the Ack at most ~300 ms,
  and **always exits 0 with no stdout** (never blocks or alters Claude).
- `src-tauri/src/agent_hooks.rs` (desktop) installs/uninstalls the hooks in
  `~/.claude/settings.json`:
  - Events: SessionStart, UserPromptSubmit, PreToolUse(`*`), PostToolUse(`*`),
    PermissionRequest(`*`), Stop, StopFailure, SubagentStart, SubagentStop,
    PostCompact, SessionEnd.
  - Command: `"<abs path to y>" agent-hook claude --ymux-agent-hook`. The
    `--ymux-agent-hook` token is the ownership marker.
  - Install is additive and idempotent: for each event, append one hook group
    unless a command containing the marker already exists there (then its path
    is refreshed). All other keys/hooks are preserved in order
    (`serde_json` with `preserve_order`). Before the first write, the original
    is copied to `settings.json.ymux-bak` (only if no backup exists).
  - Uninstall removes only hook entries whose command contains the marker,
    then drops hook groups and event arrays left empty.
  - Missing file → created with just the hooks. Unparseable file → error
    surfaced to the UI, file untouched.
  - Writes are atomic (write temp + rename).
- Setting `agent_tracking: bool` (Config, `#[serde(default)]`, default
  **false**), toggled in the Settings overlay. Toggling on installs, off
  uninstalls. On startup, if enabled, install is re-run (refreshes the `y`
  path after an upgrade). Must be added to `merge_layouts_from` (see memory:
  new Config settings silently revert otherwise).

**Process scan (every CLI, liveness).**
- `PtySession` records the child PID (`portable_pty::Child::process_id`).
- Every 2 s (desktop, background thread) `sysinfo` refreshes processes and,
  for each pane, walks descendants of the shell PID. A descendant whose
  executable stem matches a known agent (`claude`, `codex`, `gemini`,
  `opencode`, `aider`, `cursor-agent`, `amp`, or a `node`/`bun` process whose
  argv contains `claude-code`/`@anthropic-ai/claude-code`/`@openai/codex`/
  `@google/gemini-cli`) marks that agent present in the pane.
- Status for process-only agents comes from the existing frontend
  `PaneStatusMachine` (running/done/attention of the pane).

### Registry state & mapping

`PaneAgents { lead: Option<Agent>, subagents: Vec<Subagent> }`,
`Agent { kind, status, source: Hook|Process, tool: Option<String> }`,
`Subagent { id, agent_type, status }`. Status ∈ `working | waiting | done |
idle`.

| Hook event | Effect |
|---|---|
| SessionStart | lead = claude, idle |
| UserPromptSubmit | lead working; drop finished subagents |
| PreToolUse / PostToolUse (no agent_id) | lead working, tool = tool_name |
| PermissionRequest | lead waiting |
| Stop / StopFailure / PostCompact | lead done |
| SessionEnd | remove lead and subagents |
| SubagentStart | add/replace subagent (agent_id, agent_type) working |
| SubagentStop | that subagent done |
| Pre/PostToolUse with agent_id | that subagent working (lead untouched) |

Process scan: agent process gone → pane entry removed (hook and process
alike). Process present with no hook data → lead from process.

Every change emits Tauri event `agents:changed` with the full snapshot
`Record<paneId, PaneAgents>`; command `get_agents` returns it on demand.
`waiting` also raises the pane's attention status in the frontend.

### Components

- Rust: `pty/session.rs` (PID, env), `agents.rs` (registry, pure state
  machine — not desktop-gated so it is unit-testable on Linux),
  `agent_scan.rs` (desktop, sysinfo walk; the matcher is a pure fn, not
  gated), `agent_hooks.rs` (settings merge — pure fns on
  `serde_json::Value` not gated; file IO desktop), `ipc_server.rs` (route
  `agent-hook` events into the registry), `commands.rs`
  (`get_agents`, `set_agent_tracking`).
- `tools/ylauncher`: `agent-hook` subcommand (uses `yipc` client).
- TS: `src/workspace/agentTree.ts` (pure: workspaces + layout + agents →
  tree model), `WorkspacePanel.ts` (render tree), `ipc/bridge.ts`, `types.ts`,
  settings overlay toggle, i18n keys (13 languages).

### Tests

- Rust: settings merge — preserves foreign hooks/keys and order, idempotent,
  refreshes path, uninstall removes only marked entries and empty groups,
  creates missing file; registry transitions per table; matcher positives /
  negatives; `agent-hook` no-op without env (exit 0, no output).
- vitest: tree model (pane order, labels, agents attached, missing pane ids
  ignored).

---

## 2. yDir dock

### Behaviour

- A right-side dock `.file-dock` in `.app-main`, beside `.workspace-host`,
  collapsible; width draggable (min 200 px, max 50%); open state and width in
  localStorage (`ymux.fileDock`).
- Toggled by a top-bar button and `Ctrl+Shift+E` (canonical form; mapped by
  `platform.ts`). Full shortcut checklist from CLAUDE.md §6 applies.
- One app-wide instance: an xterm in the dock running `ydir <cwd>` through the
  normal `spawnPane` path with a reserved id (`__ydir_dock__`). It is not part
  of any layout tree and never saved to config.
- Spawned lazily on first open; kept alive while hidden. If ydir exits or
  cannot start, the dock shows a message and a "Restart" button.

### Following the active pane's cwd

- Backend: when the OSC 7 parser records a new cwd for a pane, emit
  `pty:cwd:{id}` (payload: path).
- Frontend `src/filedock/cwdFollow.ts` (pure, testable): given
  active-pane-changed and cwd-changed inputs, emits the target dir, debounced
  200 ms, deduplicated against the last dir sent.
- yipc becomes host→tool capable:
  - New variants: `IpcMessage::ChangeDir { path }` (host → tool). `Hello`
    carries the tool name (existing).
  - `IpcServer` keeps a registry of connected clients by tool name and exposes
    `send_to(tool, msg)`.
  - ydir, when `YMUX_IPC` is set, connects in a background thread, sends
    `Hello { tool: "ydir" }`, and forwards received `ChangeDir` into its event
    loop (channel), where it navigates as if the user had entered that dir.
    No `YMUX_IPC` → ydir behaves exactly as today.
- New command `filedock_change_dir(path)` calls `send_to("ydir", ChangeDir)`.

### Tests

- yipc: `ChangeDir` round-trip; server `send_to` reaches a connected client.
- ydir: applying a `ChangeDir` updates the listing / cwd; nonexistent path is
  ignored with no panic.
- vitest: cwdFollow debounce + dedupe + active-pane switch.

---

## 3. Bottom-anchored prompt

### Behaviour

- In the normal buffer with the viewport at the bottom, if the rows below the
  cursor/last content are empty, the terminal is drawn shifted down so that
  the last content row (or the cursor row, whichever is lower) is the pane's
  last row. After `clear`, the prompt is again at the bottom.
- Off (offset 0) in the alternate buffer (vim, less, full-screen TUIs) and
  while the user has scrolled up.
- Purely presentational: the PTY, xterm buffer and row count are unchanged.

### Mechanism

- `src/terminal/bottomAnchor.ts`: pure
  `anchorOffset({ rows, cursorY, lastContentRow, altBuffer, atBottom }):
  number` → `rows - 1 - max(cursorY, lastContentRow)` when
  `!altBuffer && atBottom`, else 0; never negative.
- `TerminalPane` computes inputs from `term.buffer.active` (type, baseY,
  viewportY, cursorY, scanning lines bottom-up for the last non-empty row) and
  applies `transform: translateY(offset * cellHeight px)` to the `.xterm`
  element; the pane host clips (`overflow: hidden`). Recomputed on `onRender`,
  `onResize`, `onScroll`, `onBufferChange`, coalesced per animation frame.
- Mouse mapping stays correct because xterm maps pointer events through
  `getBoundingClientRect`, which includes the transform. The IME preview and
  helper textarea live inside `.xterm` and move with it.
- Setting `bottom_anchor: bool` (Config, `#[serde(default = true)]` via a
  default fn), Settings overlay toggle, added to `merge_layouts_from`.

### Tests

- vitest: `anchorOffset` — empty screen, cursor on last row, alt buffer,
  scrolled up, content below cursor, rows=1.
- Manual (GUI): scrollback restore, selection, IME preview, wheel scroll,
  search highlight, resize.

---

## Out of scope

Hooks for agents other than Claude Code; editing files from the dock; an
Orca-style separate chat composer.

---

## 4. Pane tabs (added 2026-09-20)

Panes hold several terminals, switched by a tab strip **inside** the pane, under
the hotkey bar. Title and hotkey bar are shared by all tabs of a pane; each tab
is its own PTY session.

```
┌ pane ───────────────────────┐
│ my-pane            (title)  │  shared
│ [build] [test]     (hotkeys)│  shared
│ ┌pwsh┐┌claude┐┌ycode: x.rs┐ │  tab strip (hidden when only one tab)
│ terminal of the active tab  │
└─────────────────────────────┘
```

### Model

Reuse the existing but unused `LayoutNode::Tabs { active, children }` (Rust and
TS both already have it; nothing creates it today). A pane gains a tab when it
is wrapped in a Tabs node whose children are panes. No `PaneSpec` change, so
the 4-place field-sync rule is untouched.

Rendering: when a Tabs node's children are all panes, the container renders one
shared chrome — the active child's title row and hotkey bar — then the strip,
then only the active child's terminal host. `TerminalPane` gains an option to
render without its own chrome. Nested Tabs (a Tabs child of a Tabs) is not
created by any command; rendering falls back to today's behaviour.

Non-active tabs stay alive (their PTY keeps running, their xterm stays mounted
but hidden), as split panes do today.

### Interaction

- `Ctrl+Shift+T` new tab (same shell/cwd as the active tab), `Ctrl+Shift+W`
  closes the active tab and, on the last one, the pane (today's meaning),
  `Ctrl+Shift+[` / `Ctrl+Shift+]` previous/next tab. Full CLAUDE.md §6
  checklist. A `+` button on the strip, a tab context menu (rename, close,
  close others) and palette commands mirror these.
- Labels: the pane's own title when the user set one (double-click to rename),
  otherwise the running program — shell name by default, `claude`, `codex`,
  `ycode: <file>` etc. from the existing per-pane process scan, which is
  extended to report the deepest descendant's name (and, for ycode, its file
  argument) for every pane, not only known agents.
- Drag-to-reorder tabs is out of scope.

### yDir dock: open a file in a viewer tab

`Enter` on a file in the dock's ydir sends `IpcMessage::Event { kind:
"open-file", payload: { path } }` to ymux instead of running ycode inside the
dock. ymux opens it in the **viewer tab** of the pane that was active before
the dock took focus: one viewer tab per pane, reused — the second Enter
replaces the file (kill and respawn `ycode <path>` in that tab), so tabs do not
pile up. Focus moves to the viewer tab. Outside dock mode ydir keeps running
ycode inline, as today.

### Workspace tree

The left tree becomes Workspace › Pane › Tab › Agent. A pane with a single tab
omits the tab level, so today's depth is unchanged for simple panes. Agents
attach to the tab (pane id) they run in.

### Tests

- vitest: tab node helpers (add/close/switch/active clamping, closing the last
  tab unwraps the Tabs node), label derivation, tree model with tabs, viewer-tab
  reuse decision.
- Rust: `Tabs` TOML round-trip with several children; process scan reporting a
  non-agent descendant; ydir `open-file` message.
- Manual: strip hidden for a single tab, shortcuts, rename persistence,
  scrollback per tab, bottom anchor unaffected, dock Enter reuse.
