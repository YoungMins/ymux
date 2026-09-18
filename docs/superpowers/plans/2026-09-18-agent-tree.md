# Agent Tree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the left workspace panel into a Workspace › Pane › Agent tree. Every pane of every workspace is listed. Under each pane the tree shows the coding agents running in it (Claude Code, Codex, Gemini, …), with live status. Claude Code subagents appear as nested rows.

**Architecture:** A pure Rust `AgentRegistry` (`agents.rs`, not desktop-gated) keyed by pane id is fed from two sources. The first is Claude Code hooks. `y agent-hook claude`, a new subcommand of the `ylauncher` sidecar, relays each hook's stdin JSON over the existing yipc socket as `IpcMessage::Event { kind: "agent-hook" }`, and `ipc_server.rs` routes that into the registry. The second is a 2 s `sysinfo` process-tree scan (`agent_scan.rs`) under each pane's shell PID. Every change emits `agents:changed` with the full snapshot, and `get_agents` returns it on demand. The Settings toggle calls `set_agent_tracking`, which additively merges or removes ymux-owned hook entries in `~/.claude/settings.json` (`agent_hooks.rs`, pure `serde_json::Value` functions plus atomic file IO). On the frontend, a pure `agentTree.ts` combines workspaces, layout and agents into a tree model, and `WorkspacePanel.ts` renders it.

**Tech Stack:** Rust (Tauri 2.10.3, portable-pty 0.8.1, sysinfo 0.33.1, serde_json 1.0.149 + `preserve_order`, yipc), TypeScript (vanilla DOM, Vitest), 13-language i18n.

**Spec:** docs/superpowers/specs/2026-09-18-orca-style-layout-design.md (section 1)

## Global Constraints

- **`desktop` feature gate (CLAUDE.md §1).** Only Tauri glue is `#[cfg(feature = "desktop")]`: `commands.rs`, `ipc_server.rs`, and the `start_agent_scan` loop at the bottom of `agent_scan.rs`. `agents.rs`, the pure part of `agent_scan.rs`, and **all** of `agent_hooks.rs` (file IO included, since it uses only `std` + `dirs` + `serde_json`) stay ungated. `cargo check --no-default-features --lib --tests -p ymux` must pass after every Rust task.
- **Config setting (CLAUDE.md §8 + memory "merge_layouts_from silently drops new Config settings").** `agent_tracking` is additive with `#[serde(default)]`. Do **not** bump `CONFIG_VERSION`. It **must** be copied in `Config::merge_layouts_from`.
- **This feature does not touch `PaneSpec`**, so the 4-place PaneSpec sync rule (CLAUDE.md §2) does not apply.
- **i18n (CLAUDE.md §7).** Every new user-visible string is a key in `src/i18n/i18n.ts` with all 13 languages (en, ko, ja, zh, hi, es, fr, ar, pt, ru, tr, de, vi).
- **Self-contained shared-file edits.** Features 2 (yDir dock) and 3 (bottom anchor) land after this on the same branch and touch the same files.
  - `Config`: add one field after `default_shell`.
  - `merge_layouts_from`: add one line after `default_shell`.
  - SettingsOverlay: add one row after the scrollback row.
  - i18n: tree keys go after `status.attention`. Settings keys go after `settings.general.persistScrollback`.
  - yipc: purely additive. One method, one constant. No new message variants; `ChangeDir` / `send_to` belong to feature 2.
  - Do not refactor code you don't need to.
- **`serde_json` gets `features = ["preserve_order"]`** in `src-tauri/Cargo.toml`. Cargo unifies features workspace-wide, so every `serde_json` user in the ymux build graph (Tauri included) switches from `BTreeMap` to `IndexMap`. That is intended and harmless. Without it, writing `~/.claude/settings.json` back would alphabetically reorder the user's whole file. With it on, **never call `serde_json::Map::remove`**. Under `preserve_order` it is `swap_remove` and scrambles order. Use `retain` / `shift_remove`.
- **Hook binary must never disturb Claude Code.** `y agent-hook` always exits 0, never prints to stdout, and returns immediately when `YMUX_PANE_ID` or `YMUX_IPC` is unset.
- **Workspace-panel drag invariant.** `rows[]` in `WorkspacePanel.ts` must contain **only** workspace rows. `rows.indexOf(row)` is passed straight to `manager.moveWorkspace`. Pane and agent rows live in a separate per-workspace container that is a **sibling** after the workspace row, never inside it.
- **No `data-pane-id` on tree rows.** `WorkspaceManager.typeIntoPaneAt` resolves drop targets via `closest("[data-pane-id]")`.
- MSRV is 1.77 (`src-tauri/Cargo.toml`). Do not use `Option::is_none_or` (1.82) or `LazyLock` (1.80). `is_some_and` (1.70) is fine.
- Source files use CRLF line endings. Make edits with the Edit tool (exact-string replace), not `sed` with `$` anchors.
- DRY, YAGNI, TDD. Commit after every task. Commit messages end with a blank line, then `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. The two-`-m` form below produces exactly that.

Verified API facts this plan relies on:
- `portable_pty::Child::process_id(&self) -> Option<u32>` is implemented for both Unix (`std::process::Child::id`) and Windows ConPTY (`GetProcessId`). Source: `portable-pty-0.8.1/src/lib.rs:137`, `src/win/mod.rs:109`.
- sysinfo 0.33.1 provides:
  - `System::new()`
  - `System::refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet).with_cmd(UpdateKind::OnlyIfNotSet))`
  - `System::processes() -> &HashMap<Pid, Process>`
  - `Process::{pid(), parent() -> Option<Pid>, name() -> &OsStr, exe() -> Option<&Path>, cmd() -> &[OsString]}`
  - `Pid::as_u32()`
- serde_json 1.0.149 has a `preserve_order` feature (indexmap 2). `Map::remove` is `swap_remove` under it (`map.rs:156`), while `Map::retain` and `shift_remove` preserve order.
- `IpcMessage::Event { kind: String, payload: serde_json::Value }` already exists in yipc, with a round-trip test.

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `src-tauri/src/config/model.rs` | modify | `Config.agent_tracking` + `merge_layouts_from` + tests |
| `src-tauri/src/pty/session.rs` | modify | capture child PID at spawn; inject `YMUX_PANE_ID` |
| `src-tauri/src/pty/manager.rs` | modify | `pids_snapshot()` |
| `src-tauri/src/agents.rs` | **create** | `AgentRegistry` pure state machine, snapshot types, `HookEvent`, `SharedAgents` |
| `src-tauri/src/agent_scan.rs` | **create** | pure matcher + process-tree walk; desktop `start_agent_scan` loop |
| `src-tauri/src/agent_hooks.rs` | **create** | pure settings merge (install/uninstall) + atomic file IO + `set_enabled` |
| `src-tauri/src/lib.rs` | modify | register the three new modules |
| `src-tauri/Cargo.toml` | modify | `serde_json` `preserve_order` |
| `src-tauri/src/commands.rs` | modify | `get_agents`, `set_agent_tracking`, `apply_agent_hook`, `emit_agents_changed` |
| `src-tauri/src/ipc_server.rs` | modify | route `agent-hook` events into the registry |
| `src-tauri/src/main.rs` | modify | manage `SharedAgents`, register commands, startup hook refresh, start scan |
| `crates/yipc/src/client.rs` | modify | `IpcClient::set_read_timeout` |
| `crates/yipc/src/protocol.rs`, `crates/yipc/src/lib.rs` | modify | `AGENT_HOOK_KIND` constant |
| `tools/ylauncher/Cargo.toml` | modify | depend on `yipc`, `serde_json` |
| `tools/ylauncher/src/agent_hook.rs` | **create** | `y agent-hook` subcommand |
| `tools/ylauncher/src/main.rs` | modify | dispatch `agent-hook` before tool lookup |
| `tools/ylauncher/tests/agent_hook.rs` | **create** | binary-level tests (no-op outside ymux, relays inside) |
| `src/types.ts` | modify | `Config.agent_tracking`, agent snapshot types |
| `src/ipc/bridge.ts` | modify | `getAgents`, `setAgentTracking`, `onAgentsChanged` |
| `src/workspace/agentTree.ts` | **create** | pure tree model + expansion-state helpers |
| `src/workspace/agentTree.test.ts` | **create** | vitest for the model |
| `src/terminal/paneStatus.ts` (+ `.test.ts`) | modify | `onWaiting()` |
| `src/terminal/TerminalPane.ts` | modify | `markWaiting()` |
| `src/workspace/WorkspaceManager.ts` | modify | agents state, tree listeners, `focusPane`, `agentTracking` |
| `src/main.ts` | modify | seed + subscribe to agents |
| `src/workspace/WorkspacePanel.ts` | modify | render the tree |
| `src/style.css` | modify | tree rows, caret, panel width, settings error hint |
| `src/settings/SettingsOverlay.ts` | modify | agent-tracking toggle |
| `src/i18n/i18n.ts` | modify | 10 new keys × 13 languages |

---

### Task 1: `agent_tracking` config setting

**Files:**
- Modify: `src-tauri/src/config/model.rs`:
  - `Config` struct: lines 38-69
  - `merge_layouts_from`: lines 144-161
  - test `merge_layouts_carries_user_settings_back`: lines 696-716
  - the nine exhaustive `Config { … }` literals whose line reads `            default_shell: String::new(),`: lines 108 (the `Default` impl), 766, 778, 836, 868, 1003, 1032, 1094, 1227
- Modify: `src/types.ts:56-70` (`Config` interface)

**Interfaces:**
- Produces: `Config.agent_tracking: bool` (Rust, serde default `false`) and `Config.agent_tracking?: boolean` (TS). Consumed by Tasks 9, 12 and 14.

- [ ] **Step 1: Write the failing tests.**

  In `model.rs`, change `merge_layouts_carries_user_settings_back` so that `frontend_save` also sets `agent_tracking: true,` right after `default_shell: "pwsh".into(),`, and add `assert!(backend.agent_tracking);` after the last assert. Then add these two tests right after `font_size_roundtrips`:

```rust
    /// Hooks are written into another tool's settings file, so the setting
    /// must be opt-in: a config written before it existed loads as off.
    #[test]
    fn agent_tracking_defaults_off_when_absent() {
        let parsed: Config = toml::from_str("version = 7\n").expect("parse");
        assert!(!parsed.agent_tracking);
    }

    #[test]
    fn agent_tracking_roundtrips() {
        let config = Config {
            agent_tracking: true,
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        assert!(loaded.agent_tracking);
    }
```

- [ ] **Step 2: Run the tests and confirm they fail.**

  Run: `cargo test --no-default-features --lib -p ymux -- agent_tracking merge_layouts_carries_user_settings_back`

  Expected: compile error `struct `Config` has no field named `agent_tracking``.

- [ ] **Step 3: Write the minimal implementation.**

  In the `Config` struct, after the `default_shell` field (line 68), add:

```rust
    /// Install ymux's Claude Code hooks into `~/.claude/settings.json` so the
    /// agent tree gets precise per-agent status and subagents. Off by default:
    /// writing another tool's settings file must be opt-in. Additive with a
    /// serde default, so no `CONFIG_VERSION` bump.
    #[serde(default)]
    pub agent_tracking: bool,
```

  In `merge_layouts_from`, after `self.default_shell = incoming.default_shell;`, add:

```rust
        self.agent_tracking = incoming.agent_tracking;
```

  Use the Edit tool with `replace_all: true` on `model.rs`:
  - old_string: `            default_shell: String::new(),` (12 leading spaces)
  - new_string: that same line, then a newline, then `            agent_tracking: false,`

  This updates the `Default` impl and all eight test literals (9 occurrences). Verify:

  `grep -c "agent_tracking: false," src-tauri/src/config/model.rs` → `9`.

  In `src/types.ts`, inside `interface Config`, after `default_shell: string;`, add:

```ts
  /// Claude Code hooks installed in ~/.claude/settings.json (agent tree).
  /// Optional: absent in configs written before the setting existed.
  agent_tracking?: boolean;
```

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run: `cargo test --no-default-features --lib -p ymux -- agent_tracking merge_layouts` → all PASS.

  Run: `npx tsc --noEmit` → no errors.

- [ ] **Step 5: Commit.**

```sh
git add src-tauri/src/config/model.rs src/types.ts
git commit -m "feat(config): add opt-in agent_tracking setting" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: PTY records child PID and injects `YMUX_PANE_ID`

**Files:**
- Modify: `src-tauri/src/pty/session.rs`:
  - struct: lines 39-44
  - env loop and spawn: lines 113-121
  - constructor: lines 177-182
  - methods: after `kill`, line 206
  - tests module: line 222+
- Modify: `src-tauri/src/pty/manager.rs`:
  - after `cwds_snapshot`: lines 129-133
  - tests: line 179+

**Interfaces:**
- Produces:
  - `PtySession::pid(&self) -> Option<u32>`
  - `PtyManager::pids_snapshot(&self) -> HashMap<Uuid, u32>`
  - env var `YMUX_PANE_ID=<pane uuid>` in every PTY child
- Consumed by: Task 8 (hook reads `YMUX_PANE_ID`) and Task 10 (scan reads PIDs).

- [ ] **Step 1: Write the failing tests.**

  These tests are cross-platform: `cmd.exe` on Windows, `/bin/sh` elsewhere. They therefore also run on the Windows dev machine, unlike the existing `#[cfg(unix)]` PTY tests.

  Append inside `mod tests` in `session.rs`:

```rust
    /// A profile that runs one command and exits, on either platform.
    fn one_shot_profile(unix_script: &str, windows_cmd: &str) -> ShellProfile {
        let (executable, args) = if cfg!(windows) {
            ("cmd.exe", vec!["/C".to_string(), windows_cmd.to_string()])
        } else {
            ("/bin/sh", vec!["-c".to_string(), unix_script.to_string()])
        };
        ShellProfile {
            name: "one-shot".into(),
            executable: executable.into(),
            args,
            icon: None,
            color: None,
            env: Vec::new(),
        }
    }

    fn size_24x80() -> PtySize {
        PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// Drain pane output until `marker` shows up, the child exits, or 10 s pass.
    fn capture_until(rx: &mpsc::Receiver<PaneEvent>, marker: &str) -> String {
        let mut captured = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(PaneEvent::Data(_, b)) => {
                    captured.extend_from_slice(&b);
                    if String::from_utf8_lossy(&captured).contains(marker) {
                        break;
                    }
                }
                Ok(PaneEvent::Exit(_, _)) => break,
                Err(_) => continue,
            }
        }
        String::from_utf8_lossy(&captured).into_owned()
    }

    /// `y agent-hook` runs inside Claude Code inside the pane and reports which
    /// pane it is in via `YMUX_PANE_ID` — so every child must see its own id.
    #[test]
    fn pty_child_sees_its_own_pane_id() {
        let profile = one_shot_profile(
            "printf 'ymux-pane=[%s]\\n' \"$YMUX_PANE_ID\"",
            "echo ymux-pane=[%YMUX_PANE_ID%]",
        );
        let spec = PaneSpec::new_default();
        let (tx, rx) = mpsc::channel();
        let cwds: CwdMap = Arc::new(Mutex::new(HashMap::new()));
        let session =
            PtySession::spawn(&spec, &profile, size_24x80(), tx, cwds, &[]).expect("spawn");
        let expected = format!("ymux-pane=[{}]", spec.id);
        let text = capture_until(&rx, &expected);
        assert!(text.contains(&expected), "expected {expected}, got: {text:?}");
        drop(session);
    }

    /// The agent scan walks the process tree from the shell's PID.
    #[test]
    fn pty_session_records_child_pid() {
        let profile = one_shot_profile("sleep 1", "ping -n 2 127.0.0.1 >NUL");
        let spec = PaneSpec::new_default();
        let (tx, _rx) = mpsc::channel();
        let cwds: CwdMap = Arc::new(Mutex::new(HashMap::new()));
        let session =
            PtySession::spawn(&spec, &profile, size_24x80(), tx, cwds, &[]).expect("spawn");
        assert!(session.pid().is_some_and(|p| p > 0), "pid: {:?}", session.pid());
    }
```

  Append inside `mod tests` in `manager.rs`:

```rust
    #[test]
    fn pids_snapshot_tracks_live_sessions() {
        let profile = if cfg!(windows) {
            ShellProfile {
                name: "cmd".into(),
                executable: "cmd.exe".into(),
                args: vec![],
                icon: None,
                color: None,
                env: Vec::new(),
            }
        } else {
            ShellProfile {
                name: "sh".into(),
                executable: "/bin/sh".into(),
                args: vec![],
                icon: None,
                color: None,
                env: Vec::new(),
            }
        };
        let mgr = PtyManager::default();
        let spec = PaneSpec::new_default();
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        mgr.spawn(&spec, &profile, size).expect("spawn");
        assert!(mgr.pids_snapshot().get(&spec.id).is_some_and(|p| *p > 0));
        mgr.kill(spec.id).expect("kill");
        assert!(!mgr.pids_snapshot().contains_key(&spec.id));
    }
```

- [ ] **Step 2: Run the tests and confirm they fail.**

  Run: `cargo test --no-default-features --lib -p ymux -- pty_child_sees_its_own_pane_id pty_session_records_child_pid pids_snapshot_tracks_live_sessions`

  Expected: compile errors `no method named `pid`` and `no method named `pids_snapshot``.

- [ ] **Step 3: Write the minimal implementation.**

  In `session.rs`, add a field to the struct after `child`:

```rust
    /// Child (shell) PID, captured once at spawn so the 2 s agent scan never
    /// has to take the `child` lock that `kill`/`Drop` contend on.
    pid: Option<u32>,
```

  After the `extra_env` loop (line 116), add:

```rust
        // Per-pane identity for tools running inside the pane: the Claude
        // Code hook (`y agent-hook claude`) reports it back over yipc so the
        // agent tree knows which pane an event belongs to. Set last so
        // neither the profile nor the pane env can clobber it.
        cmd.env("YMUX_PANE_ID", spec.id.to_string());
```

  Immediately after the `let child = pair.slave.spawn_command(cmd)…?;` statement, add:

```rust
        let pid = child.process_id();
```

  In `Ok(Self { … })`, add `pid,` after `child: Mutex::new(child),`. After `kill`, add:

```rust
    /// Shell PID, if the platform reported one at spawn.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
```

  In `manager.rs`, after `cwds_snapshot`, add:

```rust
    /// Snapshot of `pane id → shell PID` for every live session whose PID is
    /// known. Read by the agent scan every 2 s.
    pub fn pids_snapshot(&self) -> HashMap<Uuid, u32> {
        self.sessions
            .lock()
            .iter()
            .filter_map(|(id, s)| s.pid().map(|p| (*id, p)))
            .collect()
    }
```

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run the Step 2 command → 3 PASS. These are the repo's first PTY tests that are not `cfg(unix)`, so run them on the Windows machine too. If `cmd.exe` fails to resolve there, use `std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())` as the executable in `one_shot_profile` and in the manager test.

  Run: `cargo test --no-default-features --lib -p ymux` → all PASS.

- [ ] **Step 5: Commit.**

```sh
git add src-tauri/src/pty/session.rs src-tauri/src/pty/manager.rs
git commit -m "feat(pty): record shell PID and inject YMUX_PANE_ID" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `AgentRegistry` state machine

**Files:**
- Create: `src-tauri/src/agents.rs`
- Modify: `src-tauri/src/lib.rs:7` (add `pub mod agents;` before `pub mod config;`)

**Interfaces:**
- Produces:
  - `enum AgentStatus { Working, Waiting, Done, Idle }`, serialized lowercase
  - `enum AgentSource { Hook, Process }`, serialized lowercase
  - `struct Agent { kind: String, status: AgentStatus, source: AgentSource, tool: Option<String> }`
  - `struct Subagent { id: String, agent_type: String, status: AgentStatus }`
  - `struct PaneAgents { lead: Option<Agent>, subagents: Vec<Subagent> }`
  - `type AgentSnapshot = BTreeMap<Uuid, PaneAgents>`
  - `struct HookEvent { pane_id: Uuid, agent: String, event: String, session_id: String, agent_id: Option<String>, agent_type: Option<String>, tool_name: Option<String> }` with `HookEvent::from_payload(&serde_json::Value) -> Option<HookEvent>`
  - `AgentRegistry::apply_hook(&mut self, &HookEvent) -> bool`
  - `AgentRegistry::apply_scan(&mut self, live: &HashSet<Uuid>, found: &HashMap<Uuid, String>) -> bool`
  - `AgentRegistry::snapshot(&self) -> AgentSnapshot`
  - `struct SharedAgents(pub parking_lot::Mutex<AgentRegistry>)`, which is `Default`
- Consumed by: Tasks 9 and 10 (Rust), and Task 11 (TS mirror of the JSON shape).

- [ ] **Step 1: Write the failing tests.**

  Create `src-tauri/src/agents.rs` containing only the test module below, and add `pub mod agents;` to `lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ev(pane: Uuid, event: &str) -> HookEvent {
        HookEvent {
            pane_id: pane,
            agent: "claude".into(),
            event: event.into(),
            session_id: "s1".into(),
            agent_id: None,
            agent_type: None,
            tool_name: None,
        }
    }

    fn tool_ev(pane: Uuid, event: &str, tool: &str) -> HookEvent {
        HookEvent {
            tool_name: Some(tool.into()),
            ..ev(pane, event)
        }
    }

    fn sub_ev(pane: Uuid, event: &str, id: &str, agent_type: &str) -> HookEvent {
        HookEvent {
            agent_id: Some(id.into()),
            agent_type: Some(agent_type.into()),
            ..ev(pane, event)
        }
    }

    fn lead(reg: &AgentRegistry, pane: Uuid) -> Agent {
        reg.snapshot()[&pane].lead.clone().expect("lead")
    }

    fn set(ids: &[Uuid]) -> HashSet<Uuid> {
        ids.iter().copied().collect()
    }

    fn found(pairs: &[(Uuid, &str)]) -> HashMap<Uuid, String> {
        pairs.iter().map(|(id, k)| (*id, k.to_string())).collect()
    }

    #[test]
    fn session_start_creates_an_idle_hook_lead() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        assert!(reg.apply_hook(&ev(p, "SessionStart")));
        let l = lead(&reg, p);
        assert_eq!(l.kind, "claude");
        assert_eq!(l.status, AgentStatus::Idle);
        assert_eq!(l.source, AgentSource::Hook);
        assert_eq!(l.tool, None);
    }

    #[test]
    fn user_prompt_submit_sets_working_and_drops_finished_subagents() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a2", "executor"));
        reg.apply_hook(&sub_ev(p, "SubagentStop", "a1", "Explore"));
        reg.apply_hook(&ev(p, "UserPromptSubmit"));
        let snap = reg.snapshot();
        assert_eq!(snap[&p].lead.as_ref().unwrap().status, AgentStatus::Working);
        let ids: Vec<&str> = snap[&p].subagents.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["a2"]);
    }

    #[test]
    fn tool_use_without_agent_id_sets_lead_working_with_tool() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&tool_ev(p, "PreToolUse", "Bash"));
        let l = lead(&reg, p);
        assert_eq!(l.status, AgentStatus::Working);
        assert_eq!(l.tool.as_deref(), Some("Bash"));
        reg.apply_hook(&tool_ev(p, "PostToolUse", "Edit"));
        assert_eq!(lead(&reg, p).tool.as_deref(), Some("Edit"));
    }

    #[test]
    fn tool_use_with_agent_id_marks_only_that_subagent() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "Stop"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        reg.apply_hook(&sub_ev(p, "SubagentStop", "a1", "Explore"));
        let mut e = sub_ev(p, "PreToolUse", "a1", "Explore");
        e.tool_name = Some("Grep".into());
        reg.apply_hook(&e);
        let snap = reg.snapshot();
        assert_eq!(snap[&p].subagents[0].status, AgentStatus::Working);
        assert_eq!(snap[&p].lead.as_ref().unwrap().status, AgentStatus::Done);
    }

    #[test]
    fn tool_use_for_an_unknown_subagent_adds_it() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&sub_ev(p, "PostToolUse", "zz", "Plan"));
        let s = &reg.snapshot()[&p].subagents[0];
        assert_eq!((s.id.as_str(), s.agent_type.as_str()), ("zz", "Plan"));
        assert_eq!(s.status, AgentStatus::Working);
    }

    #[test]
    fn permission_request_sets_waiting() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        reg.apply_hook(&tool_ev(p, "PermissionRequest", "Bash"));
        assert_eq!(lead(&reg, p).status, AgentStatus::Waiting);
    }

    #[test]
    fn stop_family_sets_done() {
        for event in ["Stop", "StopFailure", "PostCompact"] {
            let p = Uuid::new_v4();
            let mut reg = AgentRegistry::default();
            reg.apply_hook(&tool_ev(p, "PreToolUse", "Bash"));
            reg.apply_hook(&ev(p, event));
            let l = lead(&reg, p);
            assert_eq!(l.status, AgentStatus::Done, "{event}");
            assert_eq!(l.tool, None, "{event}");
        }
    }

    #[test]
    fn session_end_removes_lead_and_subagents() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        assert!(reg.apply_hook(&ev(p, "SessionEnd")));
        assert!(!reg.snapshot().contains_key(&p));
    }

    #[test]
    fn subagent_start_replaces_and_stop_marks_done() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Plan"));
        let snap = reg.snapshot();
        assert_eq!(snap[&p].subagents.len(), 1);
        assert_eq!(snap[&p].subagents[0].agent_type, "Plan");
        reg.apply_hook(&sub_ev(p, "SubagentStop", "a1", "Plan"));
        assert_eq!(reg.snapshot()[&p].subagents[0].status, AgentStatus::Done);
    }

    #[test]
    fn unknown_event_changes_nothing() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        assert!(!reg.apply_hook(&ev(p, "Notification")));
        assert!(reg.snapshot().is_empty());
    }

    #[test]
    fn scan_adds_a_process_lead_when_there_is_no_hook_data() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        assert!(reg.apply_scan(&set(&[p]), &found(&[(p, "codex")])));
        let l = lead(&reg, p);
        assert_eq!((l.kind.as_str(), l.source), ("codex", AgentSource::Process));
        assert_eq!(l.status, AgentStatus::Idle);
        // Unchanged scan → no change reported (no redundant emit).
        assert!(!reg.apply_scan(&set(&[p]), &found(&[(p, "codex")])));
    }

    #[test]
    fn scan_keeps_a_hook_lead_as_is() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&tool_ev(p, "PreToolUse", "Bash"));
        reg.apply_scan(&set(&[p]), &found(&[(p, "claude")]));
        let l = lead(&reg, p);
        assert_eq!(l.source, AgentSource::Hook);
        assert_eq!(l.status, AgentStatus::Working);
    }

    #[test]
    fn scan_removes_the_entry_once_the_agent_process_is_gone() {
        let hook_pane = Uuid::new_v4();
        let proc_pane = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(hook_pane, "SessionStart"));
        let live = set(&[hook_pane, proc_pane]);
        reg.apply_scan(&live, &found(&[(hook_pane, "claude"), (proc_pane, "aider")]));
        assert!(reg.apply_scan(&live, &HashMap::new()));
        assert!(reg.snapshot().is_empty());
    }

    #[test]
    fn scan_keeps_a_hook_entry_whose_process_was_never_detected() {
        // Detection can miss an unusual install; hook data alone must not
        // flicker away every 2 s. Only a *previously seen* process "going
        // away" removes the entry.
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        assert!(!reg.apply_scan(&set(&[p]), &HashMap::new()));
        assert!(reg.snapshot().contains_key(&p));
    }

    #[test]
    fn scan_drops_closed_panes() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        assert!(reg.apply_scan(&HashSet::new(), &HashMap::new()));
        assert!(reg.snapshot().is_empty());
    }

    #[test]
    fn session_end_is_not_resurrected_by_a_lingering_process() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        reg.apply_scan(&set(&[p]), &found(&[(p, "claude")]));
        reg.apply_hook(&ev(p, "SessionEnd"));
        // The CLI is still shutting down when the next scan runs.
        reg.apply_scan(&set(&[p]), &found(&[(p, "claude")]));
        assert!(!reg.snapshot().contains_key(&p));
        // A new session in the same pane shows up again.
        reg.apply_hook(&ev(p, "SessionStart"));
        assert!(reg.snapshot().contains_key(&p));
    }

    #[test]
    fn from_payload_parses_and_rejects() {
        let p = Uuid::new_v4();
        let ok = serde_json::json!({
            "pane_id": p.to_string(), "agent": "claude", "event": "PreToolUse",
            "session_id": "s", "tool_name": "Bash"
        });
        let parsed = HookEvent::from_payload(&ok).expect("parse");
        assert_eq!(parsed.pane_id, p);
        assert_eq!(parsed.tool_name.as_deref(), Some("Bash"));
        assert_eq!(parsed.agent_id, None);
        let bad = serde_json::json!({ "pane_id": "not-a-uuid", "agent": "claude", "event": "Stop" });
        assert!(HookEvent::from_payload(&bad).is_none());
    }

    #[test]
    fn snapshot_serializes_to_the_frontend_shape() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&tool_ev(p, "PreToolUse", "Bash"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        let v = serde_json::to_value(reg.snapshot()).expect("json");
        assert_eq!(
            v[p.to_string()],
            serde_json::json!({
                "lead": { "kind": "claude", "status": "working", "source": "hook", "tool": "Bash" },
                "subagents": [{ "id": "a1", "agent_type": "Explore", "status": "working" }]
            })
        );
    }
}
```

- [ ] **Step 2: Run the tests and confirm they fail.**

  Run: `cargo test --no-default-features --lib -p ymux -- agents::tests`

  Expected: compile errors (`cannot find type `HookEvent``, `AgentRegistry`, …).

- [ ] **Step 3: Write the minimal implementation.**

  Put this above the test module in `agents.rs`:

```rust
//! Agent registry: which coding agents (Claude Code, Codex, …) run in which
//! pane, and what they are doing. A pure state machine — no Tauri, no IO — so
//! it is unit-tested on Linux. Two feeds:
//! - Claude Code hooks, relayed by `y agent-hook claude` over yipc
//!   ([`AgentRegistry::apply_hook`]): precise, and the only source of
//!   subagents.
//! - The 2 s process scan in `agent_scan` ([`AgentRegistry::apply_scan`]):
//!   covers every CLI and decides liveness.

use std::collections::{BTreeMap, HashMap, HashSet};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Working,
    Waiting,
    Done,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSource {
    Hook,
    Process,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Agent {
    pub kind: String,
    pub status: AgentStatus,
    pub source: AgentSource,
    pub tool: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subagent {
    pub id: String,
    pub agent_type: String,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PaneAgents {
    pub lead: Option<Agent>,
    pub subagents: Vec<Subagent>,
}

/// What the frontend sees: pane id → agents. Panes with none are absent.
pub type AgentSnapshot = BTreeMap<Uuid, PaneAgents>;

/// One hook invocation, as `y agent-hook` packs it into the IPC payload.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HookEvent {
    pub pane_id: Uuid,
    pub agent: String,
    pub event: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
}

impl HookEvent {
    /// `None` for anything that isn't a well-formed hook payload (including a
    /// `pane_id` that isn't a UUID).
    pub fn from_payload(payload: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(payload.clone()).ok()
    }
}

#[derive(Debug, Default)]
struct PaneState {
    agents: PaneAgents,
    /// The scan has seen an agent process here, so its disappearance means
    /// the agent exited.
    process_seen: bool,
    /// A hook reported `SessionEnd`: don't resurrect a process lead from a
    /// CLI that is still shutting down. Cleared by the next hook event.
    ended: bool,
}

#[derive(Debug, Default)]
pub struct AgentRegistry {
    panes: HashMap<Uuid, PaneState>,
}

/// Tauri-managed handle: `app.manage(SharedAgents::default())`.
#[derive(Default)]
pub struct SharedAgents(pub Mutex<AgentRegistry>);

fn set_lead(st: &mut PaneState, kind: &str, status: AgentStatus, tool: Option<String>) {
    st.agents.lead = Some(Agent {
        kind: kind.to_string(),
        status,
        source: AgentSource::Hook,
        tool,
    });
}

fn upsert_subagent(st: &mut PaneState, id: &str, agent_type: Option<&str>, status: AgentStatus) {
    let agent_type = agent_type.filter(|t| !t.is_empty());
    match st.agents.subagents.iter_mut().find(|s| s.id == id) {
        Some(s) => {
            s.status = status;
            if let Some(t) = agent_type {
                s.agent_type = t.to_string();
            }
        }
        None => st.agents.subagents.push(Subagent {
            id: id.to_string(),
            agent_type: agent_type.unwrap_or_default().to_string(),
            status,
        }),
    }
}

impl AgentRegistry {
    pub fn snapshot(&self) -> AgentSnapshot {
        self.panes
            .iter()
            .filter(|(_, s)| s.agents.lead.is_some() || !s.agents.subagents.is_empty())
            .map(|(id, s)| (*id, s.agents.clone()))
            .collect()
    }

    /// Apply one Claude Code hook event (spec §1 mapping table). Returns
    /// whether the snapshot changed.
    pub fn apply_hook(&mut self, ev: &HookEvent) -> bool {
        let before = self.snapshot();
        let sub_id = ev.agent_id.as_deref().filter(|s| !s.is_empty());
        let st = self.panes.entry(ev.pane_id).or_default();
        // Any hook other than SessionEnd means a live session again.
        st.ended = ev.event == "SessionEnd";
        match ev.event.as_str() {
            "SessionStart" => {
                st.agents.subagents.clear();
                set_lead(st, &ev.agent, AgentStatus::Idle, None);
            }
            "UserPromptSubmit" => {
                set_lead(st, &ev.agent, AgentStatus::Working, None);
                st.agents.subagents.retain(|s| s.status != AgentStatus::Done);
            }
            "PreToolUse" | "PostToolUse" => match sub_id {
                Some(id) => {
                    upsert_subagent(st, id, ev.agent_type.as_deref(), AgentStatus::Working)
                }
                None => set_lead(st, &ev.agent, AgentStatus::Working, ev.tool_name.clone()),
            },
            "PermissionRequest" => {
                set_lead(st, &ev.agent, AgentStatus::Waiting, ev.tool_name.clone())
            }
            "Stop" | "StopFailure" | "PostCompact" => {
                set_lead(st, &ev.agent, AgentStatus::Done, None)
            }
            "SessionEnd" => st.agents = PaneAgents::default(),
            "SubagentStart" => {
                if let Some(id) = sub_id {
                    upsert_subagent(st, id, ev.agent_type.as_deref(), AgentStatus::Working);
                }
            }
            "SubagentStop" => {
                if let Some(s) = sub_id
                    .and_then(|id| st.agents.subagents.iter_mut().find(|s| s.id == id))
                {
                    s.status = AgentStatus::Done;
                }
            }
            _ => {}
        }
        self.snapshot() != before
    }

    /// Apply one process scan. `live`: every pane with a running PTY.
    /// `found`: panes where an agent process was seen, with its kind.
    /// Returns whether the snapshot changed.
    pub fn apply_scan(&mut self, live: &HashSet<Uuid>, found: &HashMap<Uuid, String>) -> bool {
        let before = self.snapshot();
        // Closed panes go; so does any pane whose previously seen agent
        // process has exited (hook-fed and process-fed alike).
        self.panes.retain(|id, st| {
            live.contains(id) && (found.contains_key(id) || !st.process_seen)
        });
        for (id, kind) in found {
            if !live.contains(id) {
                continue;
            }
            let st = self.panes.entry(*id).or_default();
            st.process_seen = true;
            if st.ended {
                continue;
            }
            match &mut st.agents.lead {
                None => {
                    st.agents.lead = Some(Agent {
                        kind: kind.clone(),
                        // The frontend derives a process lead's status from
                        // the pane's own PaneStatusMachine.
                        status: AgentStatus::Idle,
                        source: AgentSource::Process,
                        tool: None,
                    })
                }
                Some(lead) if lead.source == AgentSource::Process => lead.kind = kind.clone(),
                Some(_) => {}
            }
        }
        self.snapshot() != before
    }
}
```

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run: `cargo test --no-default-features --lib -p ymux -- agents::tests` → 18 PASS.

  Run: `cargo clippy --no-default-features --lib --tests -p ymux -- -D warnings` → clean.

- [ ] **Step 5: Commit.**

```sh
git add src-tauri/src/agents.rs src-tauri/src/lib.rs
git commit -m "feat(agents): add pure AgentRegistry state machine" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Agent process matcher and tree walk (pure)

**Files:**
- Create: `src-tauri/src/agent_scan.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod agent_scan;` before `pub mod agents;`)

**Interfaces:**
- Produces:
  - `struct ProcEntry { pid: u32, parent: Option<u32>, exe_stem: String, argv: Vec<String> }`
  - `fn match_agent(exe_stem: &str, argv: &[String]) -> Option<&'static str>`
  - `fn exe_stem(exe: Option<&Path>, name: &str) -> String`
  - `struct ProcTree<'a>` with `ProcTree::new(&'a [ProcEntry]) -> Self` and `agent_under(&self, root_pid: u32) -> Option<&'static str>`
  - `fn scan_panes(shells: &HashMap<Uuid, u32>, procs: &[ProcEntry]) -> HashMap<Uuid, String>`
- Consumed by: Task 10.

- [ ] **Step 1: Write the failing tests.**

  Create `agent_scan.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn proc(pid: u32, parent: Option<u32>, stem: &str, args: &[&str]) -> ProcEntry {
        ProcEntry {
            pid,
            parent,
            exe_stem: stem.into(),
            argv: argv(args),
        }
    }

    #[test]
    fn matcher_accepts_known_agent_executables() {
        for (stem, kind) in [
            ("claude", "claude"),
            ("Claude", "claude"),
            ("codex", "codex"),
            ("gemini", "gemini"),
            ("opencode", "opencode"),
            ("aider", "aider"),
            ("cursor-agent", "cursor-agent"),
            ("amp", "amp"),
        ] {
            assert_eq!(match_agent(stem, &[]), Some(kind), "{stem}");
        }
    }

    #[test]
    fn matcher_recognises_script_hosted_agents() {
        let win = argv(&[
            "node",
            "C:\\Users\\me\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\cli.js",
        ]);
        assert_eq!(match_agent("node", &win), Some("claude"));
        assert_eq!(
            match_agent("bun", &argv(&["bun", "/x/node_modules/@openai/codex/bin/codex.js"])),
            Some("codex")
        );
        let gemini_win = argv(&["node", "C:\\npm\\node_modules\\@google\\gemini-cli\\dist\\index.js"]);
        assert_eq!(match_agent("node", &gemini_win), Some("gemini"));
    }

    #[test]
    fn matcher_rejects_everything_else() {
        assert_eq!(match_agent("pwsh", &[]), None);
        assert_eq!(match_agent("bash", &argv(&["bash", "claude"])), None);
        assert_eq!(match_agent("claudette", &[]), None);
        assert_eq!(match_agent("node", &argv(&["node", "server.js"])), None);
        // Package markers only count inside a script host.
        assert_eq!(match_agent("python", &argv(&["python", "@openai/codex"])), None);
    }

    #[test]
    fn exe_stem_prefers_the_path_and_falls_back_to_the_name() {
        assert_eq!(exe_stem(Some(Path::new("/usr/local/bin/node")), "ignored"), "node");
        assert_eq!(exe_stem(None, "claude.exe"), "claude");
        assert_eq!(exe_stem(None, "codex"), "codex");
    }

    #[test]
    fn walk_finds_a_nested_agent_under_the_shell() {
        // shell(10) → cmd(11) → node claude-code(12)
        let procs = vec![
            proc(10, Some(1), "pwsh", &[]),
            proc(11, Some(10), "cmd", &[]),
            proc(12, Some(11), "node", &["node", "/n/@anthropic-ai/claude-code/cli.js"]),
        ];
        assert_eq!(ProcTree::new(&procs).agent_under(10), Some("claude"));
    }

    #[test]
    fn walk_ignores_the_root_and_processes_outside_the_subtree() {
        let procs = vec![
            proc(10, Some(1), "claude", &[]), // the "shell" itself
            proc(20, Some(1), "codex", &[]),  // sibling, not a descendant
        ];
        assert_eq!(ProcTree::new(&procs).agent_under(10), None);
    }

    #[test]
    fn walk_prefers_the_agent_closest_to_the_shell() {
        let procs = vec![
            proc(10, Some(1), "zsh", &[]),
            proc(30, Some(10), "claude", &[]),
            proc(31, Some(30), "codex", &[]),
        ];
        assert_eq!(ProcTree::new(&procs).agent_under(10), Some("claude"));
    }

    #[test]
    fn walk_survives_parent_cycles_from_pid_reuse() {
        let procs = vec![proc(10, Some(11), "sh", &[]), proc(11, Some(10), "sh", &[])];
        assert_eq!(ProcTree::new(&procs).agent_under(10), None);
    }

    #[test]
    fn scan_panes_maps_each_pane_to_its_agent() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let procs = vec![
            proc(10, Some(1), "bash", &[]),
            proc(11, Some(10), "gemini", &[]),
            proc(20, Some(1), "bash", &[]),
        ];
        let shells: HashMap<Uuid, u32> = [(a, 10), (b, 20)].into_iter().collect();
        let found = scan_panes(&shells, &procs);
        assert_eq!(found.get(&a).map(String::as_str), Some("gemini"));
        assert!(!found.contains_key(&b));
    }
}
```

- [ ] **Step 2: Run the tests and confirm they fail.**

  Run: `cargo test --no-default-features --lib -p ymux -- agent_scan::tests`

  Expected: compile errors (`cannot find function `match_agent``, …).

- [ ] **Step 3: Write the minimal implementation.**

  Put this above the tests in `agent_scan.rs`:

```rust
//! Process-tree scan that finds coding-agent CLIs running under each pane's
//! shell. The matcher and tree walk are pure (tested on Linux); only the
//! sysinfo refresh + emit loop is desktop-gated.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use uuid::Uuid;

/// One process, reduced to what the matcher needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub parent: Option<u32>,
    /// Executable file stem: `claude` for `/usr/local/bin/claude` or `claude.exe`.
    pub exe_stem: String,
    pub argv: Vec<String>,
}

/// Executables that are an agent by name.
const AGENT_EXES: &[&str] = &[
    "claude",
    "codex",
    "gemini",
    "opencode",
    "aider",
    "cursor-agent",
    "amp",
];

/// Script hosts whose argv identifies the agent package.
const SCRIPT_HOSTS: &[&str] = &["node", "bun"];

/// argv markers inside a script host, checked after `\` → `/` normalisation so
/// Windows `node_modules\@openai\codex` matches. `claude-code` also covers
/// `@anthropic-ai/claude-code`.
const SCRIPT_MARKERS: &[(&str, &str)] = &[
    ("claude-code", "claude"),
    ("@openai/codex", "codex"),
    ("@google/gemini-cli", "gemini"),
];

/// Which agent, if any, a process is.
pub fn match_agent(exe_stem: &str, argv: &[String]) -> Option<&'static str> {
    let stem = exe_stem.to_ascii_lowercase();
    if let Some(name) = AGENT_EXES.iter().copied().find(|n| *n == stem) {
        return Some(name);
    }
    if !SCRIPT_HOSTS.contains(&stem.as_str()) {
        return None;
    }
    let joined = argv.join(" ").replace('\\', "/").to_ascii_lowercase();
    SCRIPT_MARKERS
        .iter()
        .find(|(marker, _)| joined.contains(marker))
        .map(|(_, kind)| *kind)
}

/// File stem of `exe`, or `name` minus a trailing `.exe` when the path is
/// unreadable (e.g. another user's process on Windows).
pub fn exe_stem(exe: Option<&Path>, name: &str) -> String {
    if let Some(stem) = exe.and_then(Path::file_stem) {
        return stem.to_string_lossy().into_owned();
    }
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
        .to_string()
}

/// Parent → children index over one process snapshot.
pub struct ProcTree<'a> {
    children: HashMap<u32, Vec<&'a ProcEntry>>,
}

impl<'a> ProcTree<'a> {
    pub fn new(procs: &'a [ProcEntry]) -> Self {
        let mut children: HashMap<u32, Vec<&'a ProcEntry>> = HashMap::new();
        for p in procs {
            if let Some(parent) = p.parent.filter(|pp| *pp != p.pid) {
                children.entry(parent).or_default().push(p);
            }
        }
        for list in children.values_mut() {
            list.sort_by_key(|p| p.pid);
        }
        Self { children }
    }

    /// Breadth-first from `root_pid` (exclusive): the agent closest to the
    /// shell wins. Cycle-safe against PID reuse.
    pub fn agent_under(&self, root_pid: u32) -> Option<&'static str> {
        let mut seen: HashSet<u32> = HashSet::from([root_pid]);
        let mut queue: VecDeque<u32> = VecDeque::from([root_pid]);
        while let Some(pid) = queue.pop_front() {
            for child in self.children.get(&pid).map(Vec::as_slice).unwrap_or(&[]) {
                if !seen.insert(child.pid) {
                    continue;
                }
                if let Some(kind) = match_agent(&child.exe_stem, &child.argv) {
                    return Some(kind);
                }
                queue.push_back(child.pid);
            }
        }
        None
    }
}

/// `pane id → agent kind` for every pane whose shell has an agent descendant.
pub fn scan_panes(shells: &HashMap<Uuid, u32>, procs: &[ProcEntry]) -> HashMap<Uuid, String> {
    let tree = ProcTree::new(procs);
    shells
        .iter()
        .filter_map(|(id, pid)| tree.agent_under(*pid).map(|k| (*id, k.to_string())))
        .collect()
}
```

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run: `cargo test --no-default-features --lib -p ymux -- agent_scan::tests` → 9 PASS.

  Run: `cargo clippy --no-default-features --lib --tests -p ymux -- -D warnings` → clean.

- [ ] **Step 5: Commit.**

```sh
git add src-tauri/src/agent_scan.rs src-tauri/src/lib.rs
git commit -m "feat(agents): pure agent-process matcher and tree walk" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Claude Code settings merge (pure `serde_json::Value`)

**Files:**
- Create: `src-tauri/src/agent_hooks.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod agent_hooks;` before `pub mod agent_scan;`)
- Modify: `src-tauri/Cargo.toml`. The `serde_json = "1"` line becomes `serde_json = { version = "1", features = ["preserve_order"] }`. This regenerates `Cargo.lock` (serde_json gains an `indexmap` dependency).

**Interfaces:**
- Produces:
  - `const MARKER: &str = "--ymux-agent-hook"`
  - `const HOOK_EVENTS: &[(&str, Option<&str>)]`
  - `fn hook_command(y_path: &Path) -> String`
  - `fn install_hooks(settings: &mut Value, command: &str) -> Result<(), String>`
  - `fn uninstall_hooks(settings: &mut Value) -> bool`
- Consumed by: Task 6.

- [ ] **Step 1: Write the failing tests.**

  Create `agent_hooks.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A user's settings with foreign hooks, keys deliberately out of
    /// alphabetical order so an order-destroying rewrite is caught.
    const FOREIGN: &str = r#"{
  "model": "opus",
  "hooks": {
    "Notification": [{ "hooks": [{ "type": "command", "command": "notify-send hi" }] }],
    "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "guard.sh" }] }]
  },
  "permissions": { "allow": [] }
}"#;

    fn foreign() -> Value {
        serde_json::from_str(FOREIGN).expect("fixture")
    }

    fn cmd() -> String {
        hook_command(Path::new("/opt/ymux/y"))
    }

    fn keys(v: &Value) -> Vec<String> {
        v.as_object().expect("object").keys().cloned().collect()
    }

    fn ours_in(v: &Value, event: &str) -> Vec<String> {
        v["hooks"][event]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
                    .filter(|e| is_ours(e))
                    .map(|e| e["command"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn hook_command_is_quoted_forward_slashed_and_marked() {
        assert_eq!(
            hook_command(Path::new("C:\\Program Files\\ymux\\y.exe")),
            "\"C:/Program Files/ymux/y.exe\" agent-hook claude --ymux-agent-hook"
        );
    }

    #[test]
    fn install_into_empty_settings_adds_every_event() {
        let mut v = json!({});
        install_hooks(&mut v, &cmd()).unwrap();
        assert_eq!(v["hooks"].as_object().unwrap().len(), HOOK_EVENTS.len());
        for (event, matcher) in HOOK_EVENTS {
            let groups = v["hooks"][*event].as_array().unwrap();
            assert_eq!(groups.len(), 1, "{event}");
            assert_eq!(groups[0].get("matcher").and_then(Value::as_str), *matcher, "{event}");
            assert_eq!(ours_in(&v, event), vec![cmd()], "{event}");
        }
    }

    #[test]
    fn install_preserves_foreign_hooks_and_key_order() {
        let mut v = foreign();
        install_hooks(&mut v, &cmd()).unwrap();
        assert_eq!(keys(&v), vec!["model", "hooks", "permissions"]);
        let hook_keys = keys(&v["hooks"]);
        assert_eq!(&hook_keys[..2], &["Notification".to_string(), "PreToolUse".to_string()]);
        assert_eq!(v["hooks"]["Notification"], foreign()["hooks"]["Notification"]);
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre[0], foreign()["hooks"]["PreToolUse"][0]);
        assert_eq!(pre[1]["matcher"], "*");
    }

    #[test]
    fn install_is_idempotent() {
        let mut once = foreign();
        install_hooks(&mut once, &cmd()).unwrap();
        let mut twice = once.clone();
        install_hooks(&mut twice, &cmd()).unwrap();
        assert_eq!(
            serde_json::to_string(&once).unwrap(),
            serde_json::to_string(&twice).unwrap()
        );
    }

    #[test]
    fn install_refreshes_the_y_path() {
        let mut v = json!({});
        install_hooks(&mut v, &hook_command(Path::new("/old/y"))).unwrap();
        install_hooks(&mut v, &cmd()).unwrap();
        for (event, _) in HOOK_EVENTS {
            assert_eq!(ours_in(&v, event), vec![cmd()], "{event}");
        }
    }

    #[test]
    fn uninstall_restores_foreign_settings_exactly() {
        let mut v = foreign();
        install_hooks(&mut v, &cmd()).unwrap();
        assert!(uninstall_hooks(&mut v));
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            serde_json::to_string(&foreign()).unwrap()
        );
    }

    #[test]
    fn uninstall_keeps_foreign_entries_sharing_a_group() {
        let mut v = json!({ "hooks": { "Stop": [{ "hooks": [
            { "type": "command", "command": "mine.sh" },
            { "type": "command", "command": cmd() }
        ] }] } });
        assert!(uninstall_hooks(&mut v));
        assert_eq!(
            v,
            json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "mine.sh" }] }] } })
        );
    }

    #[test]
    fn uninstall_drops_the_hooks_key_it_emptied() {
        let mut v = json!({});
        install_hooks(&mut v, &cmd()).unwrap();
        assert!(uninstall_hooks(&mut v));
        assert_eq!(v, json!({}));
    }

    #[test]
    fn uninstall_without_our_hooks_changes_nothing() {
        let mut v = foreign();
        assert!(!uninstall_hooks(&mut v));
        assert_eq!(v, foreign());
    }

    #[test]
    fn install_rejects_unexpected_shapes() {
        assert!(install_hooks(&mut json!([]), &cmd()).is_err());
        assert!(install_hooks(&mut json!({ "hooks": 3 }), &cmd()).is_err());
        assert!(install_hooks(&mut json!({ "hooks": { "Stop": {} } }), &cmd()).is_err());
    }
}
```

- [ ] **Step 2: Run the tests and confirm they fail.**

  Run: `cargo test --no-default-features --lib -p ymux -- agent_hooks::tests`

  Expected: compile errors (`cannot find function `hook_command``, …).

- [ ] **Step 3: Write the minimal implementation.**

  Edit `src-tauri/Cargo.toml` as listed under **Files**. Then put this above the tests in `agent_hooks.rs`:

```rust
//! Install / uninstall ymux's Claude Code hooks in `~/.claude/settings.json`.
//!
//! Every hook ymux adds runs `"<abs y>" agent-hook claude --ymux-agent-hook`.
//! The trailing marker is how we recognise our own entries, so install is
//! additive and idempotent and uninstall never touches anything else. The
//! merge functions are pure over `serde_json::Value`, which needs the
//! `preserve_order` feature so the user's key order survives the rewrite.

use std::path::Path;

use serde_json::{json, Map, Value};

/// Ownership marker carried by every hook command ymux installs.
pub const MARKER: &str = "--ymux-agent-hook";

/// Hook events ymux subscribes to, with the tool matcher (if the event takes one).
pub const HOOK_EVENTS: &[(&str, Option<&str>)] = &[
    ("SessionStart", None),
    ("UserPromptSubmit", None),
    ("PreToolUse", Some("*")),
    ("PostToolUse", Some("*")),
    ("PermissionRequest", Some("*")),
    ("Stop", None),
    ("StopFailure", None),
    ("SubagentStart", None),
    ("SubagentStop", None),
    ("PostCompact", None),
    ("SessionEnd", None),
];

/// The hook command for the `y` sidecar at `y_path`. Forward slashes so the
/// same string works whether Claude Code runs it through cmd or Git Bash.
pub fn hook_command(y_path: &Path) -> String {
    let path = y_path.to_string_lossy().replace('\\', "/");
    format!("\"{path}\" agent-hook claude {MARKER}")
}

fn is_ours(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains(MARKER))
}

/// Add ymux's hook to every event in [`HOOK_EVENTS`], or refresh the command
/// of an existing ymux entry. Foreign keys and hooks are untouched and keep
/// their order. `Err` (settings left as they were) when the file's shape is
/// not what Claude Code documents.
pub fn install_hooks(settings: &mut Value, command: &str) -> Result<(), String> {
    let root = settings
        .as_object_mut()
        .ok_or("settings.json is not a JSON object")?;
    // Validate every shape before the first mutation, so an Err leaves the
    // value exactly as it came in.
    if let Some(hooks) = root.get("hooks") {
        let hooks = hooks.as_object().ok_or("\"hooks\" is not a JSON object")?;
        for (event, _) in HOOK_EVENTS {
            if hooks.get(*event).is_some_and(|v| !v.is_array()) {
                return Err(format!("\"hooks.{event}\" is not an array"));
            }
        }
    }
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("\"hooks\" is not a JSON object")?;
    for (event, matcher) in HOOK_EVENTS {
        let groups = hooks
            .entry(*event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("\"hooks.{event}\" is not an array"))?;
        let mut found = false;
        for group in groups.iter_mut() {
            let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            for entry in entries.iter_mut() {
                if is_ours(entry) {
                    entry["command"] = Value::String(command.to_string());
                    found = true;
                }
            }
        }
        if !found {
            let mut group = Map::new();
            if let Some(m) = matcher {
                group.insert("matcher".into(), Value::String((*m).into()));
            }
            group.insert(
                "hooks".into(),
                json!([{ "type": "command", "command": command, "timeout": 5 }]),
            );
            groups.push(Value::Object(group));
        }
    }
    Ok(())
}

/// Remove every hook entry carrying [`MARKER`], then any group and event
/// array (and finally the `hooks` object) that removal left empty. Returns
/// whether anything changed.
pub fn uninstall_hooks(settings: &mut Value) -> bool {
    let Some(root) = settings.as_object_mut() else {
        return false;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    // `retain`, never `remove`: under `preserve_order` `remove` is
    // `swap_remove` and would reorder the user's events.
    hooks.retain(|_event, groups| {
        let Some(list) = groups.as_array_mut() else {
            return true;
        };
        let mut touched = false;
        list.retain_mut(|group| {
            let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = entries.len();
            entries.retain(|e| !is_ours(e));
            if entries.len() == before {
                return true;
            }
            touched = true;
            !entries.is_empty()
        });
        changed |= touched;
        !(touched && list.is_empty())
    });
    let emptied = changed && hooks.is_empty();
    if emptied {
        root.retain(|k, _| k != "hooks");
    }
    changed
}
```

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run: `cargo test --no-default-features --lib -p ymux -- agent_hooks::tests` → 10 PASS.

  Run: `cargo test --no-default-features --lib -p ymux` → all PASS. This checks the serde_json feature switch broke nothing.

  Run: `cargo clippy --no-default-features --lib --tests -p ymux -- -D warnings` → clean.

- [ ] **Step 5: Commit.**

```sh
git add src-tauri/src/agent_hooks.rs src-tauri/src/lib.rs src-tauri/Cargo.toml Cargo.lock
git commit -m "feat(agents): pure Claude Code hook install/uninstall merge" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Hook settings file IO (atomic write, backup, missing/unparseable files)

**Files:**
- Modify: `src-tauri/src/agent_hooks.rs`. Add IO functions after `uninstall_hooks`, and add tests to the existing `mod tests`.

**Interfaces:**
- Consumes: `install_hooks`, `uninstall_hooks`, `hook_command` (Task 5).
- Produces:
  - `fn claude_settings_path() -> Option<PathBuf>`
  - `fn y_sidecar_path() -> YmuxResult<PathBuf>`
  - `fn install_at(path: &Path, command: &str) -> YmuxResult<()>`
  - `fn uninstall_at(path: &Path) -> YmuxResult<()>`
  - `fn set_enabled(enabled: bool) -> YmuxResult<()>`
- Consumed by: Task 9 (command + startup).

- [ ] **Step 1: Write the failing tests.**

  Add inside `mod tests` in `agent_hooks.rs`:

```rust
    /// Fresh isolated dir per test (mirrors `scrollback::tests::tempdir`).
    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "ymux-agent-hooks-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("mkdir");
        base
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json")
    }

    #[test]
    fn install_at_creates_a_missing_file_with_only_hooks() {
        let path = tempdir().join(".claude").join("settings.json");
        install_at(&path, &cmd()).unwrap();
        assert_eq!(keys(&read(&path)), vec!["hooks"]);
        assert!(!sibling(&path, "ymux-bak").exists(), "nothing to back up");
        assert!(!sibling(&path, "ymux-tmp").exists(), "temp file renamed away");
    }

    #[test]
    fn first_write_backs_up_the_original_once() {
        let path = tempdir().join("settings.json");
        std::fs::write(&path, FOREIGN).unwrap();
        install_at(&path, &cmd()).unwrap();
        let bak = sibling(&path, "ymux-bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), FOREIGN);
        uninstall_at(&path).unwrap();
        install_at(&path, &hook_command(Path::new("/new/y"))).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), FOREIGN, "backup never overwritten");
    }

    #[test]
    fn install_then_uninstall_round_trips_the_file_content() {
        let path = tempdir().join("settings.json");
        std::fs::write(&path, FOREIGN).unwrap();
        install_at(&path, &cmd()).unwrap();
        assert_eq!(ours_in(&read(&path), "Stop"), vec![cmd()]);
        uninstall_at(&path).unwrap();
        assert_eq!(
            serde_json::to_string(&read(&path)).unwrap(),
            serde_json::to_string(&foreign()).unwrap()
        );
    }

    #[test]
    fn unparseable_settings_are_left_untouched() {
        let path = tempdir().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(install_at(&path, &cmd()).is_err());
        assert!(uninstall_at(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        assert!(!sibling(&path, "ymux-bak").exists());
    }

    #[test]
    fn uninstall_of_a_missing_file_is_a_noop() {
        let path = tempdir().join("settings.json");
        uninstall_at(&path).unwrap();
        assert!(!path.exists());
    }
```

- [ ] **Step 2: Run the tests and confirm they fail.**

  Run: `cargo test --no-default-features --lib -p ymux -- agent_hooks::tests`

  Expected: compile errors (`cannot find function `install_at``, `sibling`, …).

- [ ] **Step 3: Write the minimal implementation.**

  Change the imports at the top of `agent_hooks.rs` to:

```rust
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::error::{YmuxError, YmuxResult};
```

  Append after `uninstall_hooks`:

```rust
/// `~/.claude/settings.json`, Claude Code's user-level settings.
pub fn claude_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

/// The `y` sidecar next to the running ymux executable (MSI install dir,
/// `.app/Contents/MacOS`, or `target/<profile>` under `tauri dev`).
pub fn y_sidecar_path() -> YmuxResult<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| YmuxError::Other("ymux executable has no parent directory".into()))?;
    let path = dir.join(if cfg!(windows) { "y.exe" } else { "y" });
    if path.is_file() {
        Ok(path)
    } else {
        Err(YmuxError::Other(format!(
            "y sidecar not found at {}",
            path.display()
        )))
    }
}

/// `settings.json` → `settings.json.<suffix>`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(suffix);
    PathBuf::from(s)
}

/// `Ok(None)` when the file doesn't exist; an empty file reads as `{}`.
fn read_settings(path: &Path) -> YmuxResult<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Some(Value::Object(Map::new()))),
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| {
            YmuxError::Config(format!("{} is not valid JSON: {e}", path.display()))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Atomic write (temp file + rename). Before the first write ever, the
/// original is copied to `settings.json.ymux-bak`.
fn write_settings(path: &Path, value: &Value) -> YmuxResult<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let backup = sibling(path, "ymux-bak");
    if path.exists() && !backup.exists() {
        fs::copy(path, &backup)?;
    }
    let tmp = sibling(path, "ymux-tmp");
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Install (or refresh) ymux's hooks in the settings file at `path`.
pub fn install_at(path: &Path, command: &str) -> YmuxResult<()> {
    let original = read_settings(path)?;
    let mut next = original
        .clone()
        .unwrap_or_else(|| Value::Object(Map::new()));
    install_hooks(&mut next, command).map_err(YmuxError::Config)?;
    if original.as_ref() != Some(&next) {
        write_settings(path, &next)?;
    }
    Ok(())
}

/// Remove ymux's hooks from the settings file at `path`. A missing file is fine.
pub fn uninstall_at(path: &Path) -> YmuxResult<()> {
    let Some(mut value) = read_settings(path)? else {
        return Ok(());
    };
    if uninstall_hooks(&mut value) {
        write_settings(path, &value)?;
    }
    Ok(())
}

/// Apply the `agent_tracking` setting to `~/.claude/settings.json`.
pub fn set_enabled(enabled: bool) -> YmuxResult<()> {
    let path = claude_settings_path()
        .ok_or_else(|| YmuxError::Other("cannot resolve the home directory".into()))?;
    if enabled {
        install_at(&path, &hook_command(&y_sidecar_path()?))
    } else {
        uninstall_at(&path)
    }
}
```

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run: `cargo test --no-default-features --lib -p ymux -- agent_hooks::tests` → 15 PASS.

  Run: `cargo clippy --no-default-features --lib --tests -p ymux -- -D warnings` → clean.

- [ ] **Step 5: Commit.**

```sh
git add src-tauri/src/agent_hooks.rs
git commit -m "feat(agents): atomic settings.json install/uninstall with backup" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: yipc — bounded `recv` and the `agent-hook` event kind

**Files:**
- Modify: `crates/yipc/src/client.rs`. Add a method after `recv` (line 62) and a test in `mod tests` (line 88+).
- Modify: `crates/yipc/src/protocol.rs`. Add a constant after `CommandDef` (line 271).
- Modify: `crates/yipc/src/lib.rs:25`. Re-export the constant.

**Interfaces:**
- Produces:
  - `IpcClient::set_read_timeout(&self, timeout: Option<std::time::Duration>) -> IpcResult<()>`
  - `pub const AGENT_HOOK_KIND: &str = "agent-hook"`
- Consumed by: Task 8 (ylauncher) and Task 9 (ipc_server).

- [ ] **Step 1: Write the failing test.**

  Add inside `mod tests` in `client.rs`. It is **not** `cfg(unix)`: it runs over TCP on Windows too.

```rust
    /// `y agent-hook` waits for the host's Ack but must never hang Claude
    /// Code when the host doesn't answer.
    #[test]
    fn recv_honours_the_read_timeout() {
        let handler: MessageHandler = Box::new(|_msg, _writer| {}); // never replies
        let server = IpcServer::start(handler).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut client = IpcClient::connect(server.address()).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .unwrap();
        client.send(&IpcMessage::Ack).unwrap();
        let started = std::time::Instant::now();
        assert!(client.recv().is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        drop(client);
        server.shutdown();
    }
```

- [ ] **Step 2: Run the test and confirm it fails.**

  Run: `cargo test -p yipc -- recv_honours_the_read_timeout`

  Expected: compile error `no method named `set_read_timeout``.

- [ ] **Step 3: Write the minimal implementation.**

  In `client.rs`, after `recv`:

```rust
    /// Bound how long [`recv`](Self::recv) blocks (`None` = forever, the
    /// default). A timed-out `recv` returns [`IpcError::Io`].
    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> IpcResult<()> {
        self.reader.get_ref().set_read_timeout(timeout)?;
        Ok(())
    }
```

  In `protocol.rs`, after the `CommandDef` struct:

```rust
/// `IpcMessage::Event::kind` used by `y agent-hook` to relay a coding-agent
/// hook to the ymux host (agent tree).
pub const AGENT_HOOK_KIND: &str = "agent-hook";
```

  In `lib.rs`, change `pub use protocol::{CommandDef, IpcMessage};` to `pub use protocol::{CommandDef, IpcMessage, AGENT_HOOK_KIND};`.

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run: `cargo test -p yipc` → all PASS.

  Run: `cargo clippy -p yipc -- -D warnings` → clean.

- [ ] **Step 5: Commit.**

```sh
git add crates/yipc/src/client.rs crates/yipc/src/protocol.rs crates/yipc/src/lib.rs
git commit -m "feat(yipc): client read timeout and agent-hook event kind" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: `y agent-hook` subcommand

**Files:**
- Modify: `tools/ylauncher/Cargo.toml`. Add to `[dependencies]`:
  - `yipc = { path = "../../crates/yipc" }`
  - `serde_json = "1"`
- Create: `tools/ylauncher/src/agent_hook.rs`
- Modify: `tools/ylauncher/src/main.rs`:
  - line 1: add `mod agent_hook;`
  - after the `--version` block (line 39): add the dispatch
- Create: `tools/ylauncher/tests/agent_hook.rs`

**Interfaces:**
- Consumes:
  - `yipc::{IpcClient, IpcMessage, AGENT_HOOK_KIND}`
  - `IpcClient::set_read_timeout` (Task 7)
  - env `YMUX_PANE_ID` (Task 2) and `YMUX_IPC` (existing)
- Produces:
  - CLI `y agent-hook <agent> [--ymux-agent-hook]`
  - `agent_hook::hook_message(agent: &str, stdin_json: &str, pane_id: &str) -> Option<IpcMessage>`
  - `agent_hook::run(args: &[String])`
  - wire payload `{ pane_id, agent, event, session_id, agent_id?, agent_type?, tool_name? }`, which Task 3's `HookEvent` deserializes

- [ ] **Step 1: Write the failing tests.**

  Create `tools/ylauncher/src/agent_hook.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_the_hook_fields_ymux_needs() {
        let input = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash",
                        "agent_id":"a1","agent_type":"Explore","tool_input":{"command":"ls"}}"#;
        let msg = hook_message("claude", input, "pane-1").expect("message");
        assert_eq!(
            msg,
            IpcMessage::Event {
                kind: AGENT_HOOK_KIND.into(),
                payload: serde_json::json!({
                    "pane_id": "pane-1", "agent": "claude", "event": "PreToolUse",
                    "session_id": "s1", "agent_id": "a1", "agent_type": "Explore",
                    "tool_name": "Bash"
                }),
            }
        );
    }

    #[test]
    fn omits_absent_optional_fields() {
        let msg = hook_message("claude", r#"{"hook_event_name":"Stop"}"#, "p").expect("message");
        let IpcMessage::Event { payload, .. } = msg else { panic!("not an event") };
        assert_eq!(
            payload,
            serde_json::json!({ "pane_id": "p", "agent": "claude", "event": "Stop", "session_id": "" })
        );
    }

    #[test]
    fn rejects_non_hook_input() {
        assert!(hook_message("claude", "not json", "p").is_none());
        assert!(hook_message("claude", r#"{"session_id":"s"}"#, "p").is_none());
    }
}
```

  Add `mod agent_hook;` as the first line of `main.rs`. Then create `tools/ylauncher/tests/agent_hook.rs`:

```rust
//! Binary-level checks: `y agent-hook` must be invisible to Claude Code —
//! exit 0, no stdout — whether or not it runs inside ymux.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use yipc::{IpcMessage, IpcServer, MessageHandler};

const INPUT: &str = r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash"}"#;

fn run_hook(envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_y"));
    cmd.args(["agent-hook", "claude", "--ymux-agent-hook"])
        .env_remove("YMUX_IPC")
        .env_remove("YMUX_PANE_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn y");
    // Outside ymux the hook exits without reading stdin; ignore EPIPE.
    let _ = child.stdin.take().expect("stdin").write_all(INPUT.as_bytes());
    child.wait_with_output().expect("wait")
}

#[test]
fn agent_hook_is_a_silent_noop_outside_ymux() {
    let out = run_hook(&[]);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
}

#[test]
fn agent_hook_exits_zero_when_ymux_is_unreachable() {
    let dead = if cfg!(windows) { "tcp:127.0.0.1:1" } else { "/nonexistent/ymux.sock" };
    let out = run_hook(&[("YMUX_IPC", dead), ("YMUX_PANE_ID", "p1")]);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
}

#[test]
fn agent_hook_relays_the_event_to_ymux() {
    let (tx, rx) = mpsc::channel();
    let handler: MessageHandler = Box::new(move |msg, writer| {
        tx.send(msg).ok();
        let _ = writer.write_all(IpcMessage::Ack.to_line().unwrap().as_bytes());
        let _ = writer.flush();
    });
    let server = IpcServer::start(handler).expect("server");
    std::thread::sleep(Duration::from_millis(50));
    let out = run_hook(&[("YMUX_IPC", server.address()), ("YMUX_PANE_ID", "pane-1")]);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    match rx.recv_timeout(Duration::from_secs(2)).expect("event") {
        IpcMessage::Event { kind, payload } => {
            assert_eq!(kind, "agent-hook");
            assert_eq!(payload["pane_id"], "pane-1");
            assert_eq!(payload["agent"], "claude");
            assert_eq!(payload["event"], "PreToolUse");
            assert_eq!(payload["tool_name"], "Bash");
        }
        other => panic!("unexpected message: {other:?}"),
    }
    server.shutdown();
}
```

- [ ] **Step 2: Run the tests and confirm they fail.**

  Run: `cargo test -p ylauncher`

  Expected: compile errors (`cannot find function `hook_message``, unresolved `yipc` in the test).

- [ ] **Step 3: Write the minimal implementation.**

  Add the two dependencies to `tools/ylauncher/Cargo.toml`. Put this above the tests in `agent_hook.rs`:

```rust
//! `y agent-hook <agent>` — relays a coding agent's hook event to ymux.
//!
//! Claude Code runs this for every hook ymux installed (see
//! `src-tauri/src/agent_hooks.rs`), with the hook JSON on stdin. It must never
//! get in Claude's way: it prints nothing, always exits 0, and returns at once
//! outside ymux (no `YMUX_PANE_ID` / `YMUX_IPC`), so Claude sessions started
//! in any other terminal are untouched.

use std::io::Read;
use std::time::Duration;

use serde_json::{Map, Value};
use yipc::{IpcClient, IpcMessage, AGENT_HOOK_KIND};

/// Upper bound on waiting for the host's Ack.
const ACK_TIMEOUT: Duration = Duration::from_millis(300);

/// Optional hook fields forwarded when present.
const OPTIONAL_FIELDS: &[&str] = &["agent_id", "agent_type", "tool_name"];

/// Build the IPC event for one hook invocation, or `None` if `stdin_json`
/// isn't a hook payload.
pub fn hook_message(agent: &str, stdin_json: &str, pane_id: &str) -> Option<IpcMessage> {
    let input: Value = serde_json::from_str(stdin_json).ok()?;
    let event = input.get("hook_event_name")?.as_str()?;
    let session_id = input.get("session_id").and_then(Value::as_str).unwrap_or("");
    let mut payload = Map::new();
    payload.insert("pane_id".into(), pane_id.into());
    payload.insert("agent".into(), agent.into());
    payload.insert("event".into(), event.into());
    payload.insert("session_id".into(), session_id.into());
    for key in OPTIONAL_FIELDS {
        if let Some(v) = input.get(*key).and_then(Value::as_str) {
            payload.insert((*key).into(), v.into());
        }
    }
    Some(IpcMessage::Event {
        kind: AGENT_HOOK_KIND.into(),
        payload: Value::Object(payload),
    })
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Entry point for `y agent-hook <agent> [--ymux-agent-hook]`. Never fails,
/// never prints; the caller exits 0 afterwards.
pub fn run(args: &[String]) {
    let (Some(pane_id), Some(ipc)) = (env_nonempty("YMUX_PANE_ID"), env_nonempty("YMUX_IPC"))
    else {
        return;
    };
    let agent = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str)
        .unwrap_or("claude");
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let Some(msg) = hook_message(agent, &input, &pane_id) else {
        return;
    };
    let Ok(mut client) = IpcClient::connect(&ipc) else {
        return;
    };
    if client.send(&msg).is_err() {
        return;
    }
    // Wait (bounded) for the Ack so the host has read our line before we
    // close. On Windows, closing with a reply still in flight can reset the
    // connection and drop the message.
    if client.set_read_timeout(Some(ACK_TIMEOUT)).is_ok() {
        let _ = client.recv();
    }
}
```

  In `main.rs`, right after the `--version` block (after line 39), add:

```rust
    // Hook relay for the ymux agent tree. Must be matched before the
    // `y<tool>` lookup below, which would otherwise look for `yagent-hook`.
    if args[0] == "agent-hook" {
        agent_hook::run(&args[1..]);
        return;
    }
```

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run: `cargo test -p ylauncher` → 4 existing + 3 unit + 3 integration PASS.

  Run: `cargo clippy -p ylauncher --all-targets -- -D warnings` → clean.

- [ ] **Step 5: Commit.**

```sh
git add tools/ylauncher/Cargo.toml tools/ylauncher/src/agent_hook.rs tools/ylauncher/src/main.rs tools/ylauncher/tests/agent_hook.rs Cargo.lock
git commit -m "feat(ylauncher): y agent-hook relays Claude Code hooks to ymux" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Desktop wiring — hook routing, `get_agents`, `set_agent_tracking`, startup refresh

**Files:**
- Modify: `src-tauri/src/commands.rs`. Imports at lines 8-19. Add new items after `get_pane_cwd` (line 185).
- Modify: `src-tauri/src/ipc_server.rs`. Imports at lines 6-9; handler at lines 30-42.
- Modify: `src-tauri/src/main.rs`:
  - `.manage` chain: line 40
  - `generate_handler!`: lines 42-77
  - setup, after paste-image prune: line 101

**Interfaces:**
- Consumes:
  - `SharedAgents`, `HookEvent`, `AgentSnapshot` (Task 3)
  - `agent_hooks::set_enabled` (Task 6)
  - `PtyManager::has` (existing)
  - `yipc::AGENT_HOOK_KIND` (Task 7)
- Produces:
  - `pub const AGENTS_CHANGED_EVENT: &str = "agents:changed"`
  - `pub fn emit_agents_changed(app: &AppHandle, snapshot: &AgentSnapshot)`
  - `pub fn apply_agent_hook(app: &AppHandle, payload: &serde_json::Value)`
  - `#[tauri::command] get_agents(agents: State<SharedAgents>) -> AgentSnapshot`
  - `#[tauri::command] set_agent_tracking(state: State<AppState>, enabled: bool) -> YmuxResult<()>`
- Consumed by: Task 10 (emit helper) and Task 13 (frontend IPC).

This task is Tauri glue with no Linux-testable logic of its own. The behaviour it wires up is covered by the tests in Tasks 3, 6 and 8, plus the GUI checklist in Task 15. Its gate is therefore compile + clippy on the desktop build.

- [ ] **Step 1: Write the "failing test".**

  Register the new commands in `main.rs` first. Add these two lines at the end of `generate_handler![…]`, after `ymux_lib::settings::open_config_path,`:

```rust
            ymux_lib::commands::get_agents,
            ymux_lib::commands::set_agent_tracking,
```

- [ ] **Step 2: Run it and confirm it fails.**

  Run: `cargo check -p ymux`

  Expected: error `cannot find function `get_agents` in module `ymux_lib::commands``. If tauri-build first complains about a missing `binaries/y-<triple>` sidecar, run `node scripts/build-tools.mjs` once and re-run.

- [ ] **Step 3: Write the minimal implementation.**

  In `commands.rs`, add to the imports:

```rust
use crate::agents::{AgentSnapshot, HookEvent, SharedAgents};
```

  After `get_pane_cwd`, add:

```rust
/// Tauri event carrying the full agent snapshot after every registry change.
pub const AGENTS_CHANGED_EVENT: &str = "agents:changed";

pub fn emit_agents_changed(app: &AppHandle, snapshot: &AgentSnapshot) {
    if let Err(e) = app.emit(AGENTS_CHANGED_EVENT, snapshot) {
        tracing::warn!(error = %e, "emit agents:changed failed");
    }
}

/// Route one `agent-hook` IPC payload (from `y agent-hook`) into the agent
/// registry. Payloads for panes this ymux doesn't own — malformed id, or a
/// pane that has since closed — are dropped.
pub fn apply_agent_hook(app: &AppHandle, payload: &serde_json::Value) {
    let Some(ev) = HookEvent::from_payload(payload) else {
        return;
    };
    if !app.state::<AppState>().pty.has(ev.pane_id) {
        return;
    }
    let agents = app.state::<SharedAgents>();
    let snapshot = {
        let mut reg = agents.0.lock();
        if !reg.apply_hook(&ev) {
            return;
        }
        reg.snapshot()
    };
    emit_agents_changed(app, &snapshot);
}

/// Current agent snapshot, for the frontend's initial render.
#[tauri::command]
pub fn get_agents(agents: State<'_, SharedAgents>) -> AgentSnapshot {
    agents.0.lock().snapshot()
}

/// Install (`true`) or remove (`false`) ymux's Claude Code hooks, then persist
/// the setting. The file is written first: if that fails (e.g. unparseable
/// settings.json) the error reaches the UI and the setting is not flipped.
#[tauri::command]
pub fn set_agent_tracking(state: State<'_, AppState>, enabled: bool) -> YmuxResult<()> {
    crate::agent_hooks::set_enabled(enabled)?;
    state.config.update(|c| c.agent_tracking = enabled);
    state.config.flush()?;
    Ok(())
}
```

  In `ipc_server.rs`, change the import to `use yipc::{IpcMessage, IpcServer, MessageHandler, AGENT_HOOK_KIND};`, and replace the body of the handler closure (lines 31-35, the serialize-and-emit part) with:

```rust
        match &msg {
            // Agent-tree hook relayed by `y agent-hook`: into the registry,
            // not onto the generic frontend channel.
            IpcMessage::Event { kind, payload } if kind == AGENT_HOOK_KIND => {
                crate::commands::apply_agent_hook(&app, payload);
            }
            _ => {
                // Serialize the message to a JSON Value for the event payload.
                if let Ok(value) = serde_json::to_value(&msg) {
                    let payload = IpcEventPayload { message: value };
                    let _ = app.emit(IPC_EVENT, &payload);
                }
            }
        }
```

  Keep the existing "Always acknowledge." block after it unchanged.

  In `main.rs`:
  - After `.manage(eb_registry)`, add `.manage(ymux_lib::agents::SharedAgents::default())`.
  - In `setup`, after the paste-image prune `if let Err(e) = … { … }` block, add:

```rust
            // While agent tracking is on, re-run the hook install on every
            // launch: a reinstall to another directory would otherwise leave
            // Claude Code calling a stale `y` path.
            if state.config.snapshot().agent_tracking {
                if let Err(e) = ymux_lib::agent_hooks::set_enabled(true) {
                    tracing::warn!(error = %e, "failed to refresh Claude Code hooks at startup");
                }
            }
```

- [ ] **Step 4: Run it and confirm it passes.**

  Run: `cargo check -p ymux` → OK.

  Run: `cargo clippy -p ymux -- -D warnings` → clean.

  Run: `cargo check --no-default-features --lib --tests -p ymux` → OK.

- [ ] **Step 5: Commit.**

```sh
git add src-tauri/src/commands.rs src-tauri/src/ipc_server.rs src-tauri/src/main.rs
git commit -m "feat(agents): route agent-hook IPC, add get_agents and set_agent_tracking" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: 2 s background process scan

**Files:**
- Modify: `src-tauri/src/agent_scan.rs`. Append a desktop-gated section after `scan_panes`, before `#[cfg(test)]`.
- Modify: `src-tauri/src/main.rs`. Import at line 9; setup at line 104.

**Interfaces:**
- Consumes:
  - `PtyManager::pids_snapshot` (Task 2)
  - `scan_panes`, `exe_stem`, `ProcEntry` (Task 4)
  - `SharedAgents::apply_scan` (Task 3)
  - `emit_agents_changed` (Task 9)
- Produces: `#[cfg(feature = "desktop")] pub fn start_agent_scan(app: tauri::AppHandle)`

This is desktop glue. The pure parts are tested in Task 4. The gate is compile + clippy.

- [ ] **Step 1: Write the "failing test".**

  In `main.rs`, add `use ymux_lib::agent_scan::start_agent_scan;` next to the other `use` lines. After `start_sysmonitor(app.handle().clone());`, add `start_agent_scan(app.handle().clone());`.

- [ ] **Step 2: Run it and confirm it fails.**

  Run: `cargo check -p ymux`

  Expected: error `unresolved import `ymux_lib::agent_scan::start_agent_scan``.

- [ ] **Step 3: Write the minimal implementation.**

  Append to `agent_scan.rs`, before the test module:

```rust
/// How often the process tree is re-scanned.
#[cfg(feature = "desktop")]
const SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Background thread: every [`SCAN_INTERVAL`], walk each pane's shell
/// descendants for agent CLIs, feed the registry, and emit `agents:changed`
/// when the snapshot moved. Only exe + argv are refreshed, each once per
/// process, so steady-state cost is one process-list enumeration.
#[cfg(feature = "desktop")]
pub fn start_agent_scan(app: tauri::AppHandle) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    use tauri::Manager;

    use crate::agents::SharedAgents;
    use crate::commands::{emit_agents_changed, AppState};

    std::thread::Builder::new()
        .name("ymux-agent-scan".into())
        .spawn(move || {
            let mut sys = System::new();
            let refresh = ProcessRefreshKind::nothing()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet);
            loop {
                std::thread::sleep(SCAN_INTERVAL);
                let shells = app.state::<AppState>().pty.pids_snapshot();
                let found = if shells.is_empty() {
                    HashMap::new()
                } else {
                    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
                    let procs: Vec<ProcEntry> = sys
                        .processes()
                        .values()
                        .map(|p| ProcEntry {
                            pid: p.pid().as_u32(),
                            parent: p.parent().map(|pp| pp.as_u32()),
                            exe_stem: exe_stem(p.exe(), &p.name().to_string_lossy()),
                            argv: p
                                .cmd()
                                .iter()
                                .map(|a| a.to_string_lossy().into_owned())
                                .collect(),
                        })
                        .collect();
                    scan_panes(&shells, &procs)
                };
                let live: HashSet<Uuid> = shells.keys().copied().collect();
                let agents = app.state::<SharedAgents>();
                let snapshot = {
                    let mut reg = agents.0.lock();
                    if !reg.apply_scan(&live, &found) {
                        continue;
                    }
                    reg.snapshot()
                };
                emit_agents_changed(&app, &snapshot);
            }
        })
        .expect("spawn agent scan thread");
}
```

- [ ] **Step 4: Run it and confirm it passes.**

  Run: `cargo check -p ymux` → OK.

  Run: `cargo clippy -p ymux -- -D warnings` → clean.

  Run: `cargo test --no-default-features --lib -p ymux -- agent_scan` → PASS. This confirms the gate keeps the Linux build clean.

- [ ] **Step 5: Commit.**

```sh
git add src-tauri/src/agent_scan.rs src-tauri/src/main.rs
git commit -m "feat(agents): 2s process scan feeds the agent registry" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: Pure tree model `agentTree.ts`

**Files:**
- Modify: `src/types.ts`. Append the agent snapshot types after `SpawnedPane` (line 81).
- Create: `src/workspace/agentTree.ts`
- Create: `src/workspace/agentTree.test.ts`

**Interfaces:**
- Consumes:
  - `panes(root)` and `findPane(root, id)` from `src/layout/LayoutTree.ts:238,257`
  - `PaneStatus` from `src/terminal/paneStatus.ts`
  - the JSON shape of Task 3's `AgentSnapshot`
- Produces:
  - TS types `AgentStatus`, `AgentSource`, `Agent`, `Subagent`, `PaneAgents`, `AgentSnapshot`
  - `EXPANDED_KEY`
  - types `ExpandedMap`, `AgentRow`, `PaneRow`, `WorkspaceTree`, `TreeLabels`
  - `paneStatusToAgentStatus(s: PaneStatus): AgentStatus`
  - `paneLabel(spec: PaneSpec, labels: TreeLabels): string`
  - `agentRows(paneId: Uuid, entry: PaneAgents | undefined, paneStatus: PaneStatus, labels: TreeLabels): AgentRow[]`
  - `buildAgentTree(workspaces: Workspace[], agents: AgentSnapshot, statusOf: (paneId: Uuid) => PaneStatus, labels: TreeLabels): WorkspaceTree[]`
  - `workspaceIdOfPane(workspaces: Workspace[], paneId: Uuid): number | null`
  - `newlyWaitingPanes(prev: AgentSnapshot, next: AgentSnapshot): Uuid[]`
  - `parseExpanded(raw: string | null): ExpandedMap`
  - `isExpanded(map: ExpandedMap, wsId: number): boolean`
- Consumed by: Tasks 12 and 13.

- [ ] **Step 1: Write the failing tests.**

  Create `src/workspace/agentTree.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import {
  buildAgentTree,
  isExpanded,
  newlyWaitingPanes,
  paneLabel,
  parseExpanded,
  workspaceIdOfPane,
  type TreeLabels,
} from "./agentTree";
import { newBrowserPane, newPane, paneNode } from "../layout/LayoutTree";
import type { AgentSnapshot, PaneSpec, Workspace } from "../types";
import type { PaneStatus } from "../terminal/paneStatus";

const labels: TreeLabels = { terminal: "pane", browser: "Browser", subagent: "subagent" };

const a: PaneSpec = { ...newPane("pwsh"), title: "build" };
const b: PaneSpec = newPane("Git Bash");
const c: PaneSpec = newBrowserPane("https://example.com");

const workspaces: Workspace[] = [
  {
    id: 1,
    name: "one",
    root: {
      kind: "split",
      direction: "horizontal",
      ratio: 0.5,
      a: paneNode(a),
      b: { kind: "split", direction: "vertical", ratio: 0.5, a: paneNode(b), b: paneNode(c) },
    },
  },
  { id: 3, name: "three", root: paneNode(newPane("zsh")) },
];

describe("buildAgentTree", () => {
  it("lists every pane of every workspace in depth-first order", () => {
    const tree = buildAgentTree(workspaces, {}, () => "idle", labels);
    expect(tree.map((w) => w.wsId)).toEqual([1, 3]);
    expect(tree[0].panes.map((p) => p.paneId)).toEqual([a.id, b.id, c.id]);
    expect(tree[0].panes.map((p) => p.label)).toEqual(["build", "Git Bash", "https://example.com"]);
    expect(tree[1].panes).toHaveLength(1);
  });

  it("attaches agents to their pane, subagents nested under the lead", () => {
    const agents: AgentSnapshot = {
      [b.id]: {
        lead: { kind: "claude", status: "working", source: "hook", tool: "Bash" },
        subagents: [
          { id: "s1", agent_type: "Explore", status: "working" },
          { id: "s2", agent_type: "", status: "done" },
        ],
      },
    };
    const tree = buildAgentTree(workspaces, agents, () => "idle", labels);
    const rows = tree[0].panes[1].agents;
    expect(rows.map((r) => [r.label, r.status, r.depth])).toEqual([
      ["claude", "working", 1],
      ["Explore", "working", 2],
      ["subagent", "done", 2],
    ]);
    expect(rows[0].tool).toBe("Bash");
    expect(tree[0].panes[0].agents).toEqual([]);
  });

  it("derives a process-only lead's status from the pane status", () => {
    const agents: AgentSnapshot = {
      [a.id]: { lead: { kind: "codex", status: "idle", source: "process", tool: null }, subagents: [] },
    };
    const statusOf = (id: string): PaneStatus => (id === a.id ? "attention" : "idle");
    const tree = buildAgentTree(workspaces, agents, statusOf, labels);
    expect(tree[0].panes[0].status).toBe("attention");
    expect(tree[0].panes[0].agents[0].status).toBe("waiting");
  });

  it("ignores agent entries for pane ids not in any layout", () => {
    const agents: AgentSnapshot = {
      "00000000-0000-4000-8000-000000000000": {
        lead: { kind: "claude", status: "working", source: "hook", tool: null },
        subagents: [],
      },
    };
    const tree = buildAgentTree(workspaces, agents, () => "idle", labels);
    expect(tree.flatMap((w) => w.panes.flatMap((p) => p.agents))).toEqual([]);
  });
});

describe("paneLabel", () => {
  it("falls back to shell, then to the generic label", () => {
    expect(paneLabel({ ...newPane(""), title: null }, labels)).toBe("pane");
    expect(paneLabel({ ...newBrowserPane(""), title: null }, labels)).toBe("Browser");
    expect(paneLabel({ ...c, title: "Docs" }, labels)).toBe("Docs");
  });
});

describe("workspaceIdOfPane", () => {
  it("finds the owning workspace or returns null", () => {
    expect(workspaceIdOfPane(workspaces, c.id)).toBe(1);
    expect(workspaceIdOfPane(workspaces, "missing")).toBeNull();
  });
});

describe("newlyWaitingPanes", () => {
  it("reports only panes whose lead just entered waiting", () => {
    const working = { kind: "claude", status: "working", source: "hook", tool: null } as const;
    const waiting = { ...working, status: "waiting" } as const;
    const prev: AgentSnapshot = { p1: { lead: waiting, subagents: [] }, p2: { lead: working, subagents: [] } };
    const next: AgentSnapshot = {
      p1: { lead: waiting, subagents: [] },
      p2: { lead: waiting, subagents: [] },
      p3: { lead: waiting, subagents: [] },
    };
    expect(newlyWaitingPanes(prev, next).sort()).toEqual(["p2", "p3"]);
  });
});

describe("expansion state", () => {
  it("defaults every workspace to expanded and survives garbage", () => {
    expect(parseExpanded(null)).toEqual({});
    expect(parseExpanded("not json")).toEqual({});
    expect(parseExpanded("[1,2]")).toEqual({});
    expect(parseExpanded('{"1":false,"2":"x"}')).toEqual({ "1": false });
    expect(isExpanded({}, 4)).toBe(true);
    expect(isExpanded({ "4": false }, 4)).toBe(false);
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail.**

  Run: `npx vitest run src/workspace/agentTree.test.ts`

  Expected: `Failed to resolve import "./agentTree"`.

- [ ] **Step 3: Write the minimal implementation.**

  Append to `src/types.ts` after `SpawnedPane`:

```ts
/// Agent-tree snapshot — mirror of `src-tauri/src/agents.rs` (serde
/// lowercase enums, snake_case fields).
export type AgentStatus = "working" | "waiting" | "done" | "idle";
export type AgentSource = "hook" | "process";

export interface Agent {
  kind: string;
  status: AgentStatus;
  source: AgentSource;
  tool: string | null;
}

export interface Subagent {
  id: string;
  agent_type: string;
  status: AgentStatus;
}

export interface PaneAgents {
  lead: Agent | null;
  subagents: Subagent[];
}

/// Pane id → agents. Panes with no agents are absent.
export type AgentSnapshot = Record<Uuid, PaneAgents>;
```

  Create `src/workspace/agentTree.ts`:

```ts
// Pure model behind the workspace panel's Workspace › Pane › Agent tree.
// No DOM, no IPC: WorkspacePanel renders whatever `buildAgentTree` returns.

import type {
  AgentSnapshot,
  AgentStatus,
  PaneAgents,
  PaneSpec,
  Uuid,
  Workspace,
} from "../types";
import type { PaneStatus } from "../terminal/paneStatus";
import { findPane, panes } from "../layout/LayoutTree";

/// localStorage key for per-workspace expansion (spec §1).
export const EXPANDED_KEY = "ymux.workspaceTree.expanded";

/// Workspace id (as string) → expanded. Missing ids count as expanded.
export type ExpandedMap = Record<string, boolean>;

export interface AgentRow {
  /// Stable per-row key: `${paneId}:lead` / `${paneId}:sub:${id}`.
  key: string;
  /// Lead: agent kind ("claude"). Subagent: its agent_type.
  label: string;
  status: AgentStatus;
  /// 1 = lead (or a subagent with no lead), 2 = subagent nested under the lead.
  depth: 1 | 2;
  tool: string | null;
}

export interface PaneRow {
  paneId: Uuid;
  label: string;
  status: PaneStatus;
  agents: AgentRow[];
}

export interface WorkspaceTree {
  wsId: number;
  panes: PaneRow[];
}

/// Localised fallbacks, passed in so this module stays free of i18n state.
export interface TreeLabels {
  terminal: string;
  browser: string;
  subagent: string;
}

/// The backend can't know a process-only agent's status; the pane's own
/// PaneStatusMachine is the best evidence (spec §1, "Process scan").
export function paneStatusToAgentStatus(s: PaneStatus): AgentStatus {
  switch (s) {
    case "running":
      return "working";
    case "attention":
      return "waiting";
    case "done":
      return "done";
    default:
      return "idle";
  }
}

export function paneLabel(spec: PaneSpec, labels: TreeLabels): string {
  if ((spec.pane_kind ?? "terminal") === "terminal") {
    return spec.title || spec.shell || labels.terminal;
  }
  return spec.title || spec.url || labels.browser;
}

export function agentRows(
  paneId: Uuid,
  entry: PaneAgents | undefined,
  paneStatus: PaneStatus,
  labels: TreeLabels,
): AgentRow[] {
  if (!entry) return [];
  const rows: AgentRow[] = [];
  const lead = entry.lead;
  if (lead) {
    rows.push({
      key: `${paneId}:lead`,
      label: lead.kind,
      status: lead.source === "process" ? paneStatusToAgentStatus(paneStatus) : lead.status,
      depth: 1,
      tool: lead.tool,
    });
  }
  const subDepth: 1 | 2 = lead ? 2 : 1;
  for (const s of entry.subagents) {
    rows.push({
      key: `${paneId}:sub:${s.id}`,
      label: s.agent_type || labels.subagent,
      status: s.status,
      depth: subDepth,
      tool: null,
    });
  }
  return rows;
}

/// Every pane of every workspace (config order, depth-first within a
/// layout), each with its agents. Agent entries for pane ids that are in no
/// layout are simply never looked up.
export function buildAgentTree(
  workspaces: Workspace[],
  agents: AgentSnapshot,
  statusOf: (paneId: Uuid) => PaneStatus,
  labels: TreeLabels,
): WorkspaceTree[] {
  return workspaces.map((ws) => ({
    wsId: ws.id,
    panes: panes(ws.root).map((spec) => {
      const status = statusOf(spec.id);
      return {
        paneId: spec.id,
        label: paneLabel(spec, labels),
        status,
        agents: agentRows(spec.id, agents[spec.id], status, labels),
      };
    }),
  }));
}

/// Owning workspace of `paneId` across *all* workspaces, hydrated or not.
export function workspaceIdOfPane(workspaces: Workspace[], paneId: Uuid): number | null {
  for (const ws of workspaces) {
    if (findPane(ws.root, paneId)) return ws.id;
  }
  return null;
}

/// Panes whose lead agent just entered `waiting` — each raises the pane's
/// attention status once, on the transition.
export function newlyWaitingPanes(prev: AgentSnapshot, next: AgentSnapshot): Uuid[] {
  return Object.keys(next).filter(
    (id) => next[id].lead?.status === "waiting" && prev[id]?.lead?.status !== "waiting",
  );
}

export function parseExpanded(raw: string | null): ExpandedMap {
  if (!raw) return {};
  try {
    const v: unknown = JSON.parse(raw);
    if (!v || typeof v !== "object" || Array.isArray(v)) return {};
    const out: ExpandedMap = {};
    for (const [k, b] of Object.entries(v as Record<string, unknown>)) {
      if (typeof b === "boolean") out[k] = b;
    }
    return out;
  } catch {
    return {};
  }
}

export function isExpanded(map: ExpandedMap, wsId: number): boolean {
  return map[String(wsId)] ?? true;
}
```

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run: `npx vitest run src/workspace/agentTree.test.ts` → all PASS.

  Run: `npx tsc --noEmit` → no errors.

- [ ] **Step 5: Commit.**

```sh
git add src/types.ts src/workspace/agentTree.ts src/workspace/agentTree.test.ts
git commit -m "feat(agents): pure workspace/pane/agent tree model" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 12: Frontend agent state — `waiting` raises attention, `focusPane`, IPC bridge

**Files:**
- Modify: `src/terminal/paneStatus.ts`. Add `onWaiting()` after `onFocus()` (line 86).
- Modify: `src/terminal/paneStatus.test.ts`. Append tests.
- Modify: `src/terminal/TerminalPane.ts`. Add `markWaiting()` after `blur()` (line 545).
- Modify: `src/ipc/bridge.ts`:
  - imports: lines 8-14
  - `api`: before the closing `};` at line 239
  - end of file (line 257)
- Modify: `src/workspace/WorkspaceManager.ts`:
  - imports: lines 6-40
  - fields: after line 86
  - methods: after `worktreeBaseDir` (line 906)
  - `persistDebounced`: line 1053
- Modify: `src/main.ts:57`. Add after `await manager.start();`.

**Interfaces:**
- Consumes:
  - `newlyWaitingPanes`, `workspaceIdOfPane` (Task 11)
  - `get_agents`, `set_agent_tracking`, `agents:changed` (Task 9)
  - `Config.agent_tracking` (Task 1)
- Produces:
  - `PaneStatusMachine.onWaiting(): void`
  - `TerminalPane.markWaiting(): void`
  - `api.getAgents(): Promise<AgentSnapshot>`
  - `api.setAgentTracking(enabled: boolean): Promise<void>`
  - `onAgentsChanged(handler: (s: AgentSnapshot) => void): Promise<UnlistenFn>`
  - `WorkspaceManager.agents: AgentSnapshot` (getter)
  - `WorkspaceManager.applyAgents(next: AgentSnapshot): void`
  - `WorkspaceManager.onTreeChange(cb: () => void): () => void`
  - `WorkspaceManager.focusPane(paneId: Uuid): Promise<void>`
  - `WorkspaceManager.agentTracking: boolean` (getter)
  - `WorkspaceManager.setAgentTracking(enabled: boolean): Promise<void>`
- Consumed by: Tasks 13 and 14.

- [ ] **Step 1: Write the failing tests.**

  Append to `src/terminal/paneStatus.test.ts`, inside the top-level `describe`:

```ts
  it("waiting raises attention even from done, and submit clears it", () => {
    const m = new PaneStatusMachine(() => {});
    m.onSubmit(0);
    m.onAttention(true, 0); // done
    m.onWaiting();
    expect(m.status).toBe("attention");
    m.onSubmit(1); // user answered the permission prompt
    expect(m.status).toBe("running");
  });

  it("waiting attention survives ticks until focus", () => {
    const m = new PaneStatusMachine(() => {}, 10, 10);
    m.onWaiting();
    m.tick(1_000, true);
    expect(m.status).toBe("attention");
    m.onFocus();
    expect(m.status).toBe("idle");
  });
```

- [ ] **Step 2: Run the tests and confirm they fail.**

  Run: `npx vitest run src/terminal/paneStatus.test.ts`

  Expected: `TypeError: m.onWaiting is not a function`.

- [ ] **Step 3: Write the minimal implementation.**

  In `paneStatus.ts`, after `onFocus()`:

```ts
  /// A coding agent in this pane is blocked on the user (Claude Code
  /// PermissionRequest, via the agent registry). Same loud state as an unseen
  /// bell: focus, or the Enter that answers the prompt, clears it.
  onWaiting(): void {
    this.set("attention");
  }
```

  In `TerminalPane.ts`, after `blur()`:

```ts
  /// The agent registry reports this pane's agent is waiting on the user.
  markWaiting(): void {
    this.statusMachine.onWaiting();
  }
```

  In `bridge.ts`:
  - Add `AgentSnapshot,` to the `import type { … } from "../types"` list.
  - Add inside `api`, after `gitWorktreeList`:

```ts
  /// Current agent-tree snapshot (pane id → agents).
  getAgents: (): Promise<AgentSnapshot> => call("get_agents"),

  /// Install (true) or remove (false) ymux's Claude Code hooks in
  /// ~/.claude/settings.json and persist the setting.
  setAgentTracking: (enabled: boolean): Promise<void> =>
    call("set_agent_tracking", { enabled }),
```

  - Append at the end of the file:

```ts
/// Subscribe to agent-tree snapshots pushed after every registry change.
export function onAgentsChanged(
  handler: (snapshot: AgentSnapshot) => void,
): Promise<UnlistenFn> {
  return safeListen<AgentSnapshot>("agents:changed", handler);
}
```

  In `WorkspaceManager.ts`:
  - Add `AgentSnapshot,` to the `import type { … } from "../types"` list.
  - Add `import { newlyWaitingPanes, workspaceIdOfPane } from "./agentTree";` after the `moveItem` import.
  - After the `onFontSizeChange` field, add:

```ts
  /// Latest agent-tree snapshot from the backend (pane id → agents).
  private _agents: AgentSnapshot = {};
  /// Tree listeners (the workspace panel), fired when agents change or any
  /// layout/pane metadata changes. A set so other views can subscribe too.
  private treeListeners = new Set<() => void>();
```

  - After the `worktreeBaseDir` getter, add:

```ts
  get agents(): AgentSnapshot {
    return this._agents;
  }

  /// Subscribe to tree-relevant changes. Returns an unsubscribe function.
  onTreeChange(cb: () => void): () => void {
    this.treeListeners.add(cb);
    return () => {
      this.treeListeners.delete(cb);
    };
  }

  private notifyTree(): void {
    for (const cb of this.treeListeners) cb();
  }

  /// Take a new backend snapshot. A lead that just started waiting on the
  /// user raises its pane to `attention`, unless the user is already looking
  /// at that exact pane (same bar as the bell notification).
  applyAgents(next: AgentSnapshot): void {
    for (const id of newlyWaitingPanes(this._agents, next)) {
      if (this.isWatching(id)) continue;
      const pane = this.findPaneById(id);
      if (pane instanceof TerminalPane) pane.markWaiting();
    }
    this._agents = next;
    this.notifyTree();
  }

  /// Switch to the workspace owning `paneId` (hydrating it if never visited)
  /// and focus that pane. Used by the tree's pane and agent rows.
  async focusPane(paneId: Uuid): Promise<void> {
    const wsId = workspaceIdOfPane(this.config.workspaces, paneId);
    if (wsId === null) return;
    if (wsId !== this.activeId) await this.activate(wsId);
    this.paneCaches.get(wsId)?.get(paneId)?.focus();
  }

  get agentTracking(): boolean {
    return this.config.agent_tracking ?? false;
  }

  /// Install/remove the Claude Code hooks, then record the choice. Rejects
  /// (setting unchanged) if the backend couldn't write settings.json.
  async setAgentTracking(enabled: boolean): Promise<void> {
    await api.setAgentTracking(enabled);
    this.config.agent_tracking = enabled;
    this.persistDebounced();
  }
```

  - In `persistDebounced()`, add `this.notifyTree();` as the first statement, with the comment: `// Every layout / pane-metadata mutation funnels through here — the tree follows it.`

  In `src/main.ts`, after `await manager.start();`:

```ts
  // Agent tree: subscribe first, then seed, so no change slips between.
  void onAgentsChanged((s) => manager.applyAgents(s)).catch((e) =>
    console.warn("agents:changed listen failed:", e),
  );
  void api
    .getAgents()
    .then((s) => manager.applyAgents(s))
    .catch((e) => console.warn("get_agents failed:", e));
```

  Then change `import { api } from "./ipc/bridge";` to `import { api, onAgentsChanged } from "./ipc/bridge";`.

- [ ] **Step 4: Run the tests and confirm they pass.**

  Run: `npx vitest run src/terminal/paneStatus.test.ts` → PASS.

  Run: `npx tsc --noEmit` → no errors.

  Run: `pnpm exec vitest run` → all PASS.

- [ ] **Step 5: Commit.**

```sh
git add src/terminal/paneStatus.ts src/terminal/paneStatus.test.ts src/terminal/TerminalPane.ts src/ipc/bridge.ts src/workspace/WorkspaceManager.ts src/main.ts
git commit -m "feat(agents): frontend agent state, waiting raises attention, focusPane" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 13: Render the tree in the workspace panel

**Files:**
- Modify: `src/workspace/WorkspacePanel.ts`:
  - imports: lines 1-10
  - state: lines 72-76
  - pointerdown guard: lines 157-160
  - `makeRow`: line 151+
  - `rebuild`: lines 227-240
  - subscriptions: lines 263-271
  - cleanup: lines 278-285
- Modify: `src/style.css`:
  - `.workspace-panel` width: lines 1329-1338
  - new rules after `.workspace-panel__add:hover`: line 1523
- Modify: `src/i18n/i18n.ts`. Insert after the `"status.attention"` entry (ends line 736).

**Interfaces:**
- Consumes:
  - `buildAgentTree`, `isExpanded`, `parseExpanded`, `EXPANDED_KEY`, `ExpandedMap`, `PaneRow`, `AgentRow`, `TreeLabels` (Task 11)
  - `manager.agents`, `manager.onTreeChange`, `manager.focusPane`, `manager.paneStatus` (Task 12 / existing)
- Produces: DOM only.
  - `.workspace-panel__caret`
  - `.workspace-panel__children`
  - `.workspace-panel__pane[data-status]`
  - `.workspace-panel__agent[data-status]`, with `--depth1` / `--depth2`

There is no DOM test harness in this repo (vitest runs in node). The pure logic is in Task 11. This task's gate is `tsc` plus the GUI checklist.

- [ ] **Step 1: Write the "failing test" — i18n keys first.**

  Insert after the `"status.attention"` entry in `i18n.ts`:

```ts
  "tree.expand": {
    en: "Show panes", ko: "패널 펼치기", ja: "ペインを表示", zh: "展开窗格", hi: "पैन दिखाएँ",
    es: "Mostrar paneles", fr: "Afficher les panneaux", ar: "إظهار اللوحات", pt: "Mostrar painéis",
    ru: "Показать панели", tr: "Panelleri göster", de: "Bereiche anzeigen", vi: "Hiện các khung",
  },
  "tree.collapse": {
    en: "Hide panes", ko: "패널 접기", ja: "ペインを隠す", zh: "折叠窗格", hi: "पैन छिपाएँ",
    es: "Ocultar paneles", fr: "Masquer les panneaux", ar: "إخفاء اللوحات", pt: "Ocultar painéis",
    ru: "Скрыть панели", tr: "Panelleri gizle", de: "Bereiche ausblenden", vi: "Ẩn các khung",
  },
  "tree.browser": {
    en: "Browser", ko: "브라우저", ja: "ブラウザー", zh: "浏览器", hi: "ब्राउज़र",
    es: "Navegador", fr: "Navigateur", ar: "المتصفح", pt: "Navegador",
    ru: "Браузер", tr: "Tarayıcı", de: "Browser", vi: "Trình duyệt",
  },
  "tree.subagent": {
    en: "subagent", ko: "하위 에이전트", ja: "サブエージェント", zh: "子代理", hi: "उप-एजेंट",
    es: "subagente", fr: "sous-agent", ar: "وكيل فرعي", pt: "subagente",
    ru: "субагент", tr: "alt ajan", de: "Subagent", vi: "tác tử con",
  },
  "agent.status.working": {
    en: "working", ko: "작업 중", ja: "作業中", zh: "工作中", hi: "काम कर रहा है",
    es: "trabajando", fr: "en cours", ar: "يعمل", pt: "trabalhando",
    ru: "работает", tr: "çalışıyor", de: "arbeitet", vi: "đang làm",
  },
  "agent.status.waiting": {
    en: "waiting", ko: "대기 중", ja: "待機中", zh: "等待中", hi: "प्रतीक्षा में",
    es: "esperando", fr: "en attente", ar: "بانتظار", pt: "aguardando",
    ru: "ждёт", tr: "bekliyor", de: "wartet", vi: "đang chờ",
  },
  "agent.status.done": {
    en: "done", ko: "완료", ja: "完了", zh: "完成", hi: "पूर्ण",
    es: "listo", fr: "terminé", ar: "تم", pt: "concluído",
    ru: "готово", tr: "bitti", de: "fertig", vi: "xong",
  },
  "agent.status.idle": {
    en: "idle", ko: "유휴", ja: "アイドル", zh: "空闲", hi: "निष्क्रिय",
    es: "inactivo", fr: "inactif", ar: "خامل", pt: "ocioso",
    ru: "простаивает", tr: "boşta", de: "untätig", vi: "rảnh",
  },
```

  In `WorkspacePanel.ts`, add the imports:

```ts
import type { Uuid } from "../types";
import {
  buildAgentTree,
  isExpanded,
  parseExpanded,
  EXPANDED_KEY,
  type AgentRow,
  type ExpandedMap,
  type PaneRow,
  type TreeLabels,
} from "./agentTree";
```

- [ ] **Step 2: Run it and confirm it fails.**

  Run: `npx tsc --noEmit`

  Expected: `error TS6133: 'buildAgentTree' is declared but its value is never read` (and the same for the other new imports). `tsconfig.json` has `noUnusedLocals: true`.

- [ ] **Step 3: Write the minimal implementation.**

  In `WorkspacePanel.ts`, add module-level helpers after `writeCollapsed`:

```ts
/// Per-workspace tree expansion, persisted under `ymux.workspaceTree.expanded`.
function readExpanded(): ExpandedMap {
  try {
    return parseExpanded(localStorage.getItem(EXPANDED_KEY));
  } catch {
    return {};
  }
}

function writeExpanded(map: ExpandedMap): void {
  try {
    localStorage.setItem(EXPANDED_KEY, JSON.stringify(map));
  } catch {
    /* localStorage unavailable — expansion just won't persist */
  }
}

function treeLabels(): TreeLabels {
  return {
    terminal: t("terminal.defaultTitle"),
    browser: t("tree.browser"),
    subagent: t("tree.subagent"),
  };
}
```

  Inside `mountWorkspacePanel`, after `const rows: HTMLElement[] = [];`:

```ts
  /// Per-workspace container for pane + agent rows. A *sibling* after the
  /// workspace row, never inside it: `rows` must stay workspace-rows-only
  /// because its indices feed `moveWorkspace`, and a press inside a
  /// `.workspace-panel__row` would start a workspace drag.
  const childHosts = new Map<number, HTMLElement>();
  const carets = new Map<number, HTMLButtonElement>();
  let expanded: ExpandedMap = readExpanded();
```

  In `makeRow`, change the pointerdown guard selector from `".workspace-panel__del"` to `".workspace-panel__del, .workspace-panel__caret"`. Then, right after the `row.addEventListener("pointerdown", …)` block and before `const btn = …`, insert:

```ts
    const caret = document.createElement("button");
    caret.className = "workspace-panel__caret";
    caret.type = "button";
    caret.addEventListener("click", (ev) => {
      ev.stopPropagation();
      if (dragJustEnded) return;
      expanded = { ...expanded, [String(id)]: !isExpanded(expanded, id) };
      writeExpanded(expanded);
      renderTree();
    });
    row.appendChild(caret);
    carets.set(id, caret);
```

  Add the row builders and `renderTree` after `makeRow`:

```ts
  function statusTitle(label: string, key: string | null): string {
    return key === null ? label : `${label} — ${t(key)}`;
  }

  function makePaneRow(pane: PaneRow): HTMLElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "workspace-panel__pane";
    btn.dataset.status = pane.status;
    btn.textContent = pane.label;
    btn.title = statusTitle(pane.label, pane.status === "idle" ? null : `status.${pane.status}`);
    btn.addEventListener("click", () => {
      void manager.focusPane(pane.paneId).then(highlight);
    });
    return btn;
  }

  function makeAgentRow(paneId: Uuid, agent: AgentRow): HTMLElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = `workspace-panel__agent workspace-panel__agent--depth${agent.depth}`;
    btn.dataset.status = agent.status;
    const dot = document.createElement("span");
    dot.className = "workspace-panel__agent-dot";
    dot.textContent = agent.status === "done" ? "✓" : "●";
    const name = document.createElement("span");
    name.className = "workspace-panel__agent-name";
    name.textContent = agent.label;
    const state = document.createElement("span");
    state.className = "workspace-panel__agent-state";
    state.textContent = t(`agent.status.${agent.status}`);
    btn.append(dot, name, state);
    const base = statusTitle(agent.label, `agent.status.${agent.status}`);
    btn.title = agent.tool ? `${base} (${agent.tool})` : base;
    btn.addEventListener("click", () => {
      void manager.focusPane(paneId).then(highlight);
    });
    return btn;
  }

  /// Re-render only the pane/agent rows under each workspace. Workspace rows
  /// are left alone, so an agent update mid-drag never detaches the row
  /// being dragged.
  function renderTree(): void {
    const tree = buildAgentTree(
      manager.workspaces,
      manager.agents,
      (id) => manager.paneStatus.get(id) ?? "idle",
      treeLabels(),
    );
    for (const ws of tree) {
      const host = childHosts.get(ws.wsId);
      if (!host) continue;
      const open = isExpanded(expanded, ws.wsId);
      const caret = carets.get(ws.wsId);
      if (caret) {
        caret.textContent = open ? "▾" : "▸";
        caret.title = t(open ? "tree.collapse" : "tree.expand");
        caret.setAttribute("aria-label", caret.title);
        caret.setAttribute("aria-expanded", String(open));
      }
      host.replaceChildren();
      host.style.display = open ? "" : "none";
      if (!open) continue;
      for (const pane of ws.panes) {
        host.appendChild(makePaneRow(pane));
        for (const agent of pane.agents) host.appendChild(makeAgentRow(pane.paneId, agent));
      }
    }
  }
```

  Replace `rebuild()` with:

```ts
  function rebuild(): void {
    buttons.clear();
    noteButtons.clear();
    childHosts.clear();
    carets.clear();
    rows.length = 0;
    while (list.firstChild) list.removeChild(list.firstChild);
    // Render in `config.workspaces` order — that array *is* the user's order,
    // set by drag-to-reorder and persisted by TOML's `[[workspaces]]`.
    for (const ws of manager.workspaces) {
      const row = makeRow(ws.id);
      rows.push(row);
      list.appendChild(row);
      const children = document.createElement("div");
      children.className = "workspace-panel__children";
      childHosts.set(ws.id, children);
      list.appendChild(children);
    }
    highlight();
    renderTree();
  }
```

  Replace `manager.onPaneStatusChange = () => highlight();` with:

```ts
  manager.onPaneStatusChange = () => {
    highlight();
    renderTree();
  };
  const cleanupTree = manager.onTreeChange(renderTree);
```

  In the returned cleanup function, add `cleanupTree();` before `cleanupLang();`.

  In `style.css`, widen the panel so two nesting levels stay readable. In `.workspace-panel` (lines 1329-1338), change `flex: 0 0 160px;` → `flex: 0 0 200px;` and `width: 160px;` → `width: 200px;`. Then append after the `.workspace-panel__add:hover` rule:

```css
/* ── Agent tree (Workspace › Pane › Agent) ────────────────────────────── */
.workspace-panel__caret {
  flex: 0 0 auto;
  width: 16px;
  height: 22px;
  padding: 0;
  background: transparent;
  border: none;
  color: var(--fg-muted);
  font-size: 10px;
  cursor: pointer;
}

.workspace-panel__caret:hover {
  color: var(--accent);
}

.workspace-panel__children {
  display: flex;
  flex-direction: column;
  gap: 1px;
  margin-bottom: 2px;
}

.workspace-panel__pane,
.workspace-panel__agent {
  width: 100%;
  min-width: 0;
  height: 20px;
  background: transparent;
  color: var(--fg);
  border: 1px solid transparent;
  border-radius: 4px;
  font-size: 11px;
  text-align: left;
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
}

.workspace-panel__pane {
  display: block;
  padding: 0 6px 0 18px;
  text-overflow: ellipsis;
}

.workspace-panel__pane:hover,
.workspace-panel__agent:hover {
  background: var(--bg-hover);
}

.workspace-panel__pane[data-status="running"] {
  background: var(--status-running-bg);
}

.workspace-panel__pane[data-status="done"] {
  background: var(--status-done-bg);
}

.workspace-panel__pane[data-status="attention"] {
  background: var(--status-attention-bg);
}

.workspace-panel__agent {
  display: flex;
  align-items: center;
  gap: 4px;
  padding-right: 6px;
  color: var(--fg-muted);
}

.workspace-panel__agent--depth1 {
  padding-left: 30px;
}

.workspace-panel__agent--depth2 {
  padding-left: 42px;
}

.workspace-panel__agent-dot {
  flex: 0 0 auto;
  font-size: 9px;
}

.workspace-panel__agent-name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--fg);
}

.workspace-panel__agent-state {
  flex: 0 0 auto;
  font-size: 10px;
}

.workspace-panel__agent[data-status="working"] .workspace-panel__agent-dot {
  color: var(--status-running);
}

.workspace-panel__agent[data-status="waiting"] .workspace-panel__agent-dot {
  color: var(--status-attention);
}

.workspace-panel__agent[data-status="done"] .workspace-panel__agent-dot {
  color: var(--status-done);
}
```

- [ ] **Step 4: Run it and confirm it passes.**

  Run: `npx tsc --noEmit` → no errors.

  Run: `pnpm exec vitest run` → all PASS.

  Then run `pnpm tauri dev` and smoke-check:
  - Every workspace shows its panes.
  - The caret toggles the panes, and the toggle survives a reload.
  - Clicking a pane in another workspace switches to it and focuses the pane.
  - Dragging a workspace row still reorders.
  - Pressing a pane row never starts a drag.

- [ ] **Step 5: Commit.**

```sh
git add src/workspace/WorkspacePanel.ts src/style.css src/i18n/i18n.ts
git commit -m "feat(agents): render Workspace > Pane > Agent tree in the left panel" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 14: Settings toggle for agent tracking

**Files:**
- Modify: `src/settings/SettingsOverlay.ts`:
  - import: line 9
  - new row: after the scrollback row, which ends at line 305 (`host.appendChild(scrollbackRow);`)
- Modify: `src/i18n/i18n.ts`. Insert after the `"settings.general.persistScrollback"` entry (ends line 81).
- Modify: `src/style.css`. Add after `.settings-row__hint` (ends line 1191).

**Interfaces:**
- Consumes:
  - `manager.agentTracking` and `manager.setAgentTracking` (Task 12)
  - `describeError` (`src/ipc/bridge.ts:45`)
- Produces: the Settings → General "Track Claude Code agents" checkbox.

This is DOM glue. The gate is `tsc` plus the GUI checklist. The backend behaviour it triggers is tested in Tasks 5-6.

- [ ] **Step 1: Write the "failing test" — i18n keys first.**

  Insert after `"settings.general.persistScrollback"`:

```ts
  "settings.general.agentTracking": {
    en: "Track Claude Code agents (adds hooks to ~/.claude/settings.json)",
    ko: "Claude Code 에이전트 추적 (~/.claude/settings.json에 훅 추가)",
    ja: "Claude Code エージェントを追跡 (~/.claude/settings.json にフックを追加)",
    zh: "跟踪 Claude Code 代理（向 ~/.claude/settings.json 添加钩子）",
    hi: "Claude Code एजेंट ट्रैक करें (~/.claude/settings.json में हुक जोड़ता है)",
    es: "Seguir agentes de Claude Code (añade hooks a ~/.claude/settings.json)",
    fr: "Suivre les agents Claude Code (ajoute des hooks à ~/.claude/settings.json)",
    ar: "تتبّع وكلاء Claude Code (يضيف خطافات إلى ~/.claude/settings.json)",
    pt: "Acompanhar agentes do Claude Code (adiciona hooks ao ~/.claude/settings.json)",
    ru: "Отслеживать агентов Claude Code (добавляет хуки в ~/.claude/settings.json)",
    tr: "Claude Code ajanlarını izle (~/.claude/settings.json dosyasına kanca ekler)",
    de: "Claude-Code-Agenten verfolgen (fügt Hooks in ~/.claude/settings.json hinzu)",
    vi: "Theo dõi tác tử Claude Code (thêm hook vào ~/.claude/settings.json)",
  },
  "settings.general.agentTrackingFailed": {
    en: "Could not update Claude Code hooks:", ko: "Claude Code 훅을 업데이트하지 못했습니다:",
    ja: "Claude Code のフックを更新できませんでした:", zh: "无法更新 Claude Code 钩子：",
    hi: "Claude Code हुक अपडेट नहीं हो सके:", es: "No se pudieron actualizar los hooks de Claude Code:",
    fr: "Impossible de mettre à jour les hooks de Claude Code :", ar: "تعذّر تحديث خطافات Claude Code:",
    pt: "Não foi possível atualizar os hooks do Claude Code:", ru: "Не удалось обновить хуки Claude Code:",
    tr: "Claude Code kancaları güncellenemedi:", de: "Claude-Code-Hooks konnten nicht aktualisiert werden:",
    vi: "Không cập nhật được hook của Claude Code:",
  },
```

  In `SettingsOverlay.ts`, change `import { api } from "../ipc/bridge";` to `import { api, describeError } from "../ipc/bridge";`.

- [ ] **Step 2: Run it and confirm it fails.**

  Run: `npx tsc --noEmit`

  Expected: `error TS6133: 'describeError' is declared but its value is never read` (`noUnusedLocals: true`).

- [ ] **Step 3: Write the minimal implementation.**

  After `host.appendChild(scrollbackRow);`, insert:

```ts
    // Agent tracking: installs / removes ymux's Claude Code hooks. That
    // writes another tool's settings file, so the checkbox only settles once
    // the write succeeded; a failure (e.g. unparseable settings.json)
    // reverts it and says why.
    const agentRow = document.createElement("div");
    agentRow.className = "settings-row";
    const agentLabel = document.createElement("div");
    agentLabel.className = "settings-row__label";
    agentLabel.textContent = t("settings.general.agentTracking");
    agentRow.appendChild(agentLabel);
    const agentToggle = document.createElement("input");
    agentToggle.type = "checkbox";
    agentToggle.checked = manager.agentTracking;
    const agentError = document.createElement("div");
    agentError.className = "settings-row__hint settings-row__hint--error";
    agentToggle.addEventListener("change", () => {
      const want = agentToggle.checked;
      agentToggle.disabled = true;
      agentError.textContent = "";
      manager
        .setAgentTracking(want)
        .catch((e) => {
          agentToggle.checked = !want;
          agentError.textContent = `${t("settings.general.agentTrackingFailed")} ${describeError(e)}`;
        })
        .finally(() => {
          agentToggle.disabled = false;
        });
    });
    agentRow.appendChild(agentToggle);
    const agentSpacer = document.createElement("div");
    agentRow.appendChild(agentSpacer);
    agentRow.appendChild(agentError);
    host.appendChild(agentRow);
```

  In `style.css`, after the `.settings-row__hint` rule:

```css
.settings-row__hint--error {
  color: var(--status-critical);
}

.settings-row__hint--error:empty {
  display: none;
}
```

- [ ] **Step 4: Run it and confirm it passes.**

  Run: `npx tsc --noEmit` → clean.

  Run: `pnpm exec vitest run` → all PASS.

- [ ] **Step 5: Commit.**

```sh
git add src/settings/SettingsOverlay.ts src/i18n/i18n.ts src/style.css
git commit -m "feat(agents): Settings toggle installs/removes Claude Code hooks" -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 15: Full verification + manual GUI checklist

**Files:** none modified. Fix-forward commits only if a check fails.

- [ ] **Step 1: Run the full automated suite.**

  Run: `pnpm test`. That is `bash scripts/test.sh`: fmt check, tsc, vitest, clippy on tools + lib without desktop, cargo tests for tools + lib.

  Expected: `✓ All checks passed.`

  If `cargo fmt --all --check` fails, run `cargo fmt --all` and commit it as `style: cargo fmt`, with the same Co-Authored-By trailer.

- [ ] **Step 2: Run the desktop build checks, which `scripts/test.sh` does not cover.**

  - `cargo clippy --workspace -- -D warnings` → clean.
  - `cargo check --no-default-features --lib --tests -p ymux` → OK. This is the Linux-safe gate (CLAUDE.md §1).
  - `cargo test -p yipc -p ylauncher` → all PASS, including the new tests that are not gated to Unix.

- [ ] **Step 3: Stage fresh sidecars and launch.**

  Run `node scripts/build-tools.mjs`. It builds in **release** and stages `src-tauri/binaries/y-<triple>`.

  Then run `cargo build -p ylauncher`. This makes sure a debug `y(.exe)` with the `agent-hook` subcommand sits in `target/debug/` next to the dev `ymux(.exe)`, because `agent_hooks::y_sidecar_path()` resolves `y` as a sibling of `current_exe()`.

  Then run `pnpm tauri dev`. If "Track Claude Code agents" reports `y sidecar not found at …`, the path in the message tells you which directory is missing `y`.

- [ ] **Step 4: Work through the manual GUI checklist.** Tick each item.

  **Panel and tree**
  - [ ] Every workspace lists all its panes, including workspaces never visited this session and browser panes (labelled by title, else URL).
  - [ ] Workspace carets collapse and expand. The state survives an app restart (`ymux.workspaceTree.expanded`). Collapsing the whole panel (toolbar toggle) behaves as before.
  - [ ] Clicking a pane row in another workspace switches to it and focuses that pane. The workspace row highlight follows.
  - [ ] Dragging workspace rows still reorders them. Pressing on pane or agent rows never starts a drag.
  - [ ] Splitting, closing or renaming a pane (Ctrl+Shift+R) updates the tree immediately.
  - [ ] Dropping a file onto the panel does not type into any terminal.

  **Process scan (tracking off)**
  - [ ] `codex`, `gemini` or `claude` started in a pane appears as a lead row within about 2 s. Its dot follows the pane status: blue while output flows, green when quiet.
  - [ ] Quitting the CLI removes the row within about 2 s.
  - [ ] Closing a pane with a running agent removes the pane and agent rows.

  **Hooks (tracking on)**
  - [ ] Settings → General → tick "Track Claude Code agents". `~/.claude/settings.json` now has all 11 events. Each command is `"<abs path>/y(.exe)" agent-hook claude --ymux-agent-hook`, with forward slashes. `settings.json.ymux-bak` holds the original. Pre-existing hooks and keys are intact and in their original order.
  - [ ] Start `claude` in a pane:
    - [ ] The lead appears `idle`.
    - [ ] After a prompt it shows `working`, with the tool name in the tooltip.
    - [ ] When it finishes it shows `done` (✓).
  - [ ] A prompt that spawns subagents (e.g. an Explore task) shows nested subagent rows labelled with `agent_type`, going from working (●) to done (✓). The next prompt clears finished subagents.
  - [ ] A permission prompt shows the lead as `waiting` (amber). With that pane in a background workspace, its pane and workspace tint to attention. Answering the prompt returns it to working.
  - [ ] `/exit` in Claude removes the rows. They do not flicker back while the process winds down.
  - [ ] Restart ymux with tracking on: `settings.json` still has exactly one ymux group per event (path refreshed, not duplicated).
  - [ ] Claude Code started in a terminal **outside** ymux runs normally with no hook errors (`y` exits 0 silently).
  - [ ] Untick the setting. Only the ymux entries are removed. `settings.json` otherwise matches `settings.json.ymux-bak` (`diff` after normalising whitespace).
  - [ ] Corrupt `settings.json` (e.g. append `{`) and tick the setting. An inline error appears, the checkbox reverts, and the file is byte-identical to the corrupted version.

  **i18n**
  - [ ] Switch the language to ko and then ar. The caret tooltips, agent status words and settings label are translated.

  **macOS (arm64)**
  - [ ] Repeat the hook items with zsh. The hook path points inside `ymux.app/Contents/MacOS/y`.

- [ ] **Step 5: Update the CLAUDE.md test-count table (optional).**

  Only do this if the maintainer wants it. The counts are documented as drifting, so skipping is acceptable.
