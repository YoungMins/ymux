# Worktree Panes, Persistent Scrollback & Agent Status — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three orca-inspired features to ymux v0.8.19 — open shell panes rooted in fresh `git worktree`s, persist terminal scrollback across app restarts, and show a live `idle/running/done/attention` status on each pane.

**Architecture:** A new desktop-gated `git/` Rust module shells out to the `git` binary for worktree operations, exposed via Tauri commands and driven by a Command Palette action + input modal. Scrollback is serialized frontend-side with `@xterm/addon-serialize` and persisted per-pane through two small Tauri commands. Agent status is a frontend-only state machine that reuses the existing OSC 9 / bell detection, surfaced on pane borders and workspace tabs.

**Tech Stack:** Rust (Tauri 2, `portable-pty`, `std::process::Command`), TypeScript (xterm.js 5.5 + `@xterm/addon-serialize`), TOML config.

## Global Constraints

- **Target version v0.8.19** — bump `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `package.json`, `crates/yversion/src/lib.rs`, and the badge URL in `README.md` / `README.ko.md` / `README.ja.md`; regenerate `Cargo.lock` via `cargo check` (CLAUDE.md rule #5).
- **`CONFIG_VERSION` stays 5** — all new config fields are additive with serde defaults (CLAUDE.md rule #8).
- **PaneSpec 4-place sync** — any new `PaneSpec` field MUST be added to `src-tauri/src/config/model.rs` (struct + all 3 constructors + test structs), `src/types.ts`, `src/layout/LayoutTree.ts` `nodeToSpec()`, and `src/workspace/WorkspaceManager.ts` `findAndMutatePane()` (CLAUDE.md rule #2).
- **No `Option<T>` in the tagged enum** — new string fields use `String` + `#[serde(default)]`, empty string = "no value" (CLAUDE.md rule #3).
- **Linux CI must pass** — `cargo check --no-default-features --lib --tests -p ymux` (CLAUDE.md rule #1). Tauri-dependent code goes behind `#[cfg(feature = "desktop")]`; pure git arg/parse logic stays Linux-testable.
- **i18n** — every user-visible string added to `src/i18n/i18n.ts` across all 13 languages: en, ko, ja, zh, hi, es, fr, ar, pt, ru, tr, de, vi (CLAUDE.md rule #7).
- **No libgit2** — shell out to `git` in PATH via `std::process::Command`.
- **Commit after every task.** DRY, YAGNI, TDD.

## File Structure

**Feature 1 — Worktree panes**
- Create: `src-tauri/src/git/mod.rs` — pure git-worktree wrappers (Linux-testable arg/parse logic + thin process runner)
- Modify: `src-tauri/src/lib.rs` — register `git` module + new commands in `generate_handler!`
- Modify: `src-tauri/src/commands.rs` — desktop-gated Tauri commands `git_is_repo`, `git_worktree_add`, `git_worktree_remove`, `git_worktree_list`
- Modify: `src-tauri/src/config/model.rs` — `PaneSpec.worktree_path: String`, `Config.worktree_base_dir: String`
- Modify: `src-tauri/capabilities/default.json` — allow the new commands
- Modify: `src/types.ts` — `worktree_path` on `PaneSpec`
- Modify: `src/layout/LayoutTree.ts` — `worktree_path` in `nodeToSpec` / `paneNode`
- Modify: `src/workspace/WorkspaceManager.ts` — `worktree_path` in `findAndMutatePane`; new `openWorktreePane()` flow + cleanup on close
- Modify: `src/palette/commands.ts` — "Open pane in new git worktree" command
- Create: `src/workspace/WorktreeModal.ts` — branch-name input modal (or reuse existing modal util — see Task 1.6)
- Modify: `src/ipc/bridge.ts` — TS wrappers for the new commands

**Feature 2 — Persistent scrollback**
- Modify: `package.json` — add `@xterm/addon-serialize`
- Modify: `src-tauri/src/commands.rs` — `save_scrollback`, `load_scrollback`, `delete_scrollback`
- Modify: `src-tauri/src/lib.rs` — register the three commands
- Modify: `src-tauri/src/config/model.rs` — `Config.persist_scrollback: bool` (default true)
- Modify: `src/terminal/TerminalPane.ts` — SerializeAddon load, debounced save, restore-on-mount, delete-on-kill
- Modify: `src/settings/SettingsOverlay.ts` — "Persist terminal scrollback" toggle
- Modify: `src/ipc/bridge.ts` — TS wrappers

**Feature 3 — Agent status**
- Modify: `src/terminal/TerminalPane.ts` — `PaneStatus` enum + state machine driven by existing bell/OSC9 handlers + activity/Enter tracking
- Modify: `src/workspace/WorkspaceManager.ts` — `onPaneStatusChange` callback
- Modify: `src/workspace/WorkspaceBar.ts` — per-status tab dot colour
- Modify: `src/style.css` — status border/dot colours
- Modify: `src/i18n/i18n.ts` — status tooltip strings

**Cross-cutting (final task)**
- Version bump across 6 sync points.

---

## Suggested order

Build **Feature 3 → Feature 2 → Feature 1** (easiest, most self-contained first; worktree panes then inherit status + scrollback for free). Tasks below are numbered in build order.

Key existing anchors the tasks build on (verified against the current tree):
- Command registration: `src-tauri/src/main.rs:41-69` `invoke_handler(tauri::generate_handler![...])`.
- Sample command: `src-tauri/src/commands.rs:212` `#[tauri::command] pub fn notify(app: AppHandle, title: String, body: String) -> YmuxResult<()>`.
- Config dir: `src-tauri/src/config/store.rs:23` `config_path()` → `dirs::config_dir().join("ymux").join("config.toml")`.
- Bell/OSC9: `src/terminal/TerminalPane.ts:186-194` — `term.onBell(() => opts.onAttention?.(null))` and `registerOscHandler(9, data => { if (!/^\d+;/.test(data)) opts.onAttention?.(data||null); })`; callback type `onAttention?: (message: string | null) => void`.
- Palette entry: `src/palette/commands.ts:6-11` `CommandDef { id; label: () => string; keybinding?; action: () => void | Promise<void> }`.
- Settings toggle: `src/settings/SettingsOverlay.ts:213-227` (notify row) — checkbox bound to `manager.notifyOnBell` / `manager.setNotifyOnBell`.
- Split flow: `src/workspace/WorkspaceManager.ts:466-504` `splitFocused` → `newPane` → `splitPane` → `createPane` → `cache.set` → `renderWorkspace` → `pane.spawn()` → `persistDebounced()`.
- 4-place PaneSpec field list currently: `id, title, shell, cwd, startup_cmd, env, pane_kind, url, hotkeys, bg_color` (model.rs, types.ts, LayoutTree `nodeToSpec`, WorkspaceManager `findAndMutatePane:828-849`).

---

# FEATURE 3 — Agent Status States

### Task 1: `PaneStatus` state machine (pure, frontend)

**Files:**
- Create: `src/terminal/paneStatus.ts`
- Test: `src/terminal/paneStatus.test.ts`

**Interfaces:**
- Produces: `type PaneStatus = "idle" | "running" | "done" | "attention"`;
  `class PaneStatusMachine` with methods `onSubmit()`, `onOutput()`,
  `onAttention(focused: boolean)`, `onFocus()`, `tick(now: number)`, and a
  getter `status: PaneStatus`. Constructor takes
  `(onChange: (s: PaneStatus) => void, idleAfterMs = 4000)`.

> **Decision (not optional):** the frontend currently has NO JS test runner
> (`package.json` tests are Rust + `tsc` only). This task adds `vitest` as a
> devDependency because `PaneStatusMachine` is pure branching logic that TDD
> genuinely protects, and it is the only pure-logic frontend unit in this plan.
> Do it once, here, in Step 0.

- [ ] **Step 0: Add the vitest test runner**

```bash
pnpm add -D vitest
```

Add a script to `package.json`:

```json
    "test:ts": "vitest run",
```

Run: `pnpm exec vitest run` → exits 0 (no tests yet, "No test files found" is fine).

- [ ] **Step 1: Write the failing test**

```ts
// src/terminal/paneStatus.test.ts
import { describe, it, expect } from "vitest";
import { PaneStatusMachine } from "./paneStatus";

describe("PaneStatusMachine", () => {
  it("starts idle", () => {
    const m = new PaneStatusMachine(() => {});
    expect(m.status).toBe("idle");
  });

  it("idle -> running on submit", () => {
    const m = new PaneStatusMachine(() => {});
    m.onSubmit(0);
    expect(m.status).toBe("running");
  });

  it("running -> done on attention while focused, then idle", () => {
    const seen: string[] = [];
    const m = new PaneStatusMachine((s) => seen.push(s));
    m.onSubmit(0);
    m.onAttention(true); // focused
    expect(m.status).toBe("done");
    expect(seen).toContain("done");
  });

  it("attention when bell arrives while unfocused", () => {
    const m = new PaneStatusMachine(() => {});
    m.onSubmit(0);
    m.onAttention(false); // unfocused
    expect(m.status).toBe("attention");
  });

  it("attention clears to idle on focus", () => {
    const m = new PaneStatusMachine(() => {});
    m.onAttention(false);
    expect(m.status).toBe("attention");
    m.onFocus();
    expect(m.status).toBe("idle");
  });

  it("running -> idle after idle timeout with no output", () => {
    const m = new PaneStatusMachine(() => {}, 4000);
    m.onSubmit(1000);
    m.tick(2000); // still running (within window)
    expect(m.status).toBe("running");
    m.tick(6000); // > 4000ms since last activity
    expect(m.status).toBe("idle");
  });

  it("output refreshes the running window", () => {
    const m = new PaneStatusMachine(() => {}, 4000);
    m.onSubmit(0);
    m.onOutput(3000);
    m.tick(6000); // 3000ms since last output < 4000
    expect(m.status).toBe("running");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm exec vitest run src/terminal/paneStatus.test.ts`
Expected: FAIL — cannot find module `./paneStatus`.

- [ ] **Step 3: Write the implementation**

```ts
// src/terminal/paneStatus.ts
export type PaneStatus = "idle" | "running" | "done" | "attention";

/// Frontend-only, per-pane status. Heuristic by design: `done`/`attention`
/// come from the solid OSC 9 / bell signal (reused from v0.8.18); `running`
/// is inferred from command submission + output activity, cleared after an
/// idle window. There is no reliable cross-shell "waiting for input" signal,
/// so that state is intentionally not modelled.
export class PaneStatusMachine {
  private _status: PaneStatus = "idle";
  private lastActivity = 0;

  constructor(
    private onChange: (s: PaneStatus) => void,
    private idleAfterMs = 4000,
  ) {}

  get status(): PaneStatus {
    return this._status;
  }

  private set(next: PaneStatus): void {
    if (next === this._status) return;
    this._status = next;
    this.onChange(next);
  }

  /// User pressed Enter in this pane — a command likely started.
  onSubmit(now: number): void {
    this.lastActivity = now;
    this.set("running");
  }

  /// The PTY produced output — keep the running window alive.
  onOutput(now: number): void {
    this.lastActivity = now;
    if (this._status === "running") this.lastActivity = now;
  }

  /// OSC 9 / bell fired. `focused` = the pane is currently visible+focused.
  onAttention(focused: boolean): void {
    if (focused) {
      this.set("done");
      this.set("idle");
    } else {
      this.set("attention");
    }
  }

  /// Pane gained focus — clear a pending attention flag.
  onFocus(): void {
    if (this._status === "attention") this.set("idle");
  }

  /// Called on a timer; drops running→idle once the idle window elapses.
  tick(now: number): void {
    if (this._status === "running" && now - this.lastActivity >= this.idleAfterMs) {
      this.set("idle");
    }
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm exec vitest run src/terminal/paneStatus.test.ts`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add package.json pnpm-lock.yaml src/terminal/paneStatus.ts src/terminal/paneStatus.test.ts
git commit -m "feat(status): add PaneStatusMachine state machine + vitest runner"
```

---

### Task 2: Wire the state machine into `TerminalPane`

**Files:**
- Modify: `src/terminal/TerminalPane.ts` (bell/OSC9 handlers at 186-194; `onData`/write path; focus handler)

**Interfaces:**
- Consumes: `PaneStatusMachine`, `PaneStatus` from Task 1.
- Produces: `TerminalPane` exposes `get status(): PaneStatus` and accepts an
  option `onStatusChange?: (id: Uuid, status: PaneStatus) => void`.

- [ ] **Step 1: Add the status option to the TerminalPane options type**

Find the options interface that already declares `onAttention?: (message: string | null) => void` (near line 29) and add:

```ts
  onStatusChange?: (status: PaneStatus) => void;
```

Import at the top of the file:

```ts
import { PaneStatusMachine, type PaneStatus } from "./paneStatus";
```

- [ ] **Step 2: Instantiate the machine and a focus flag**

In the `TerminalPane` constructor/field block, add:

```ts
  private statusMachine = new PaneStatusMachine((s) => this.opts.onStatusChange?.(s));
  private isFocused = false;
  private statusTimer: number | undefined;
```

Start the idle timer where the terminal is opened (after `this.term.open(...)`):

```ts
    this.statusTimer = window.setInterval(
      () => this.statusMachine.tick(Date.now()),
      1000,
    );
```

And clear it in the existing dispose/teardown method:

```ts
    if (this.statusTimer !== undefined) window.clearInterval(this.statusTimer);
```

- [ ] **Step 3: Feed the machine from the existing signal points**

Update the bell/OSC9 block at 186-194 so attention also drives status
(keep the existing `onAttention` call — it still powers notifications):

```ts
    this.term.onBell(() => {
      this.opts.onAttention?.(null);
      this.statusMachine.onAttention(this.isFocused);
    });
    this.term.parser.registerOscHandler(9, (data) => {
      if (!/^\d+;/.test(data)) {
        this.opts.onAttention?.(data || null);
        this.statusMachine.onAttention(this.isFocused);
      }
      return false;
    });
```

In the key/data path that sends user keystrokes to the PTY (the existing
`this.term.onData(...)` handler), detect Enter submission:

```ts
    this.term.onData((data) => {
      if (data.includes("\r")) this.statusMachine.onSubmit(Date.now());
      // ...existing write-to-pty call unchanged...
    });
```

In the code that writes PTY output into the terminal (the handler that calls
`this.term.write(...)` for backend `Data` events), add:

```ts
      this.statusMachine.onOutput(Date.now());
```

In the `focus()` method set the flag and clear attention:

```ts
  focus(): void {
    this.isFocused = true;
    this.statusMachine.onFocus();
    // ...existing focus body...
  }
```

Add a blur hook where focus is lost (wherever the pane is un-focused; if none
exists, set `this.isFocused = false` in the manager's focus-change path):

```ts
  blur(): void {
    this.isFocused = false;
  }
```

Expose the status getter:

```ts
  get status(): PaneStatus {
    return this.statusMachine.status;
  }
```

- [ ] **Step 4: Type-check**

Run: `npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add src/terminal/TerminalPane.ts
git commit -m "feat(status): drive PaneStatusMachine from bell/OSC9, input, output, focus"
```

---

### Task 3: Surface status on pane border + workspace tab; i18n tooltips

**Files:**
- Modify: `src/workspace/WorkspaceManager.ts` (createPane options; new `onPaneStatusChange`)
- Modify: `src/workspace/WorkspaceBar.ts` (tab dot colour by status)
- Modify: `src/style.css` (border/dot colours)
- Modify: `src/i18n/i18n.ts` (status tooltip keys)

**Interfaces:**
- Consumes: `TerminalPane.onStatusChange` from Task 2.
- Produces: `WorkspaceManager` tracks `paneStatus: Map<Uuid, PaneStatus>` and a
  public callback `onPaneStatusChange?: (workspaceId: number) => void`.

- [ ] **Step 1: Track status in WorkspaceManager**

Where `createPane(spec)` builds a `TerminalPane` (near the `splitFocused`
plumbing, ~line 494), pass the status callback:

```ts
      onStatusChange: (status) => {
        this.paneStatus.set(spec.id, status);
        this.applyPaneStatusClass(spec.id, status);
        this.onPaneStatusChange?.(this.workspaceOfPane(spec.id));
      },
```

Add the field + helper on the class:

```ts
  paneStatus = new Map<Uuid, PaneStatus>();
  onPaneStatusChange?: (workspaceId: number) => void;

  private applyPaneStatusClass(id: Uuid, status: PaneStatus): void {
    const el = document.querySelector<HTMLElement>(`[data-pane-id="${id}"]`);
    if (!el) return;
    el.classList.remove(
      "pane--status-idle", "pane--status-running",
      "pane--status-done", "pane--status-attention",
    );
    el.classList.add(`pane--status-${status}`);
  }
```

> `data-pane-id` is the attribute already set on each pane's root element for
> the v0.8.18 pulsing border; reuse it. If the attribute name differs, grep for
> the existing attention/pulse class application and mirror the selector.

- [ ] **Step 2: Colour the workspace tab dot by highest-priority status**

In `WorkspaceBar.ts`, where the per-workspace dot badge is rendered, choose the
colour by the most urgent status among that workspace's panes
(`attention > running > done > idle`):

```ts
  function tabStatusClass(statuses: PaneStatus[]): string {
    if (statuses.includes("attention")) return "ws-dot--attention";
    if (statuses.includes("running")) return "ws-dot--running";
    if (statuses.includes("done")) return "ws-dot--done";
    return "ws-dot--idle";
  }
```

Wire `WorkspaceManager.onPaneStatusChange` to call the bar's `rebuild()` (the
same callback mechanism `onWorkspacesChange` already uses).

- [ ] **Step 3: Add CSS**

```css
/* src/style.css — agent status */
.pane--status-running { box-shadow: inset 0 0 0 2px var(--status-running, #3b82f6); }
.pane--status-done    { box-shadow: inset 0 0 0 2px var(--status-done, #22c55e); }
.pane--status-attention { box-shadow: inset 0 0 0 2px var(--status-attention, #f59e0b); animation: ymux-pulse 1.2s ease-in-out infinite; }
.pane--status-idle    { box-shadow: none; }

.ws-dot--running   { background: #3b82f6; }
.ws-dot--done      { background: #22c55e; }
.ws-dot--attention { background: #f59e0b; }
.ws-dot--idle      { background: transparent; }
```

> Reuse the existing `ymux-pulse` keyframes if v0.8.18 already defined one; grep
> `@keyframes` in style.css and use that name instead of redefining.

- [ ] **Step 4: Add i18n tooltip keys (all 13 languages)**

Add to `src/i18n/i18n.ts`, following the exact object shape of existing keys
(en, ko, ja, zh, hi, es, fr, ar, pt, ru, tr, de, vi). English + Korean +
Japanese given; translate the remaining 10 mirroring the tone of neighbouring
status/notification keys:

```ts
"status.running": { en: "Running", ko: "실행 중", ja: "実行中", zh: "运行中", hi: "चल रहा है", es: "En ejecución", fr: "En cours", ar: "قيد التشغيل", pt: "Em execução", ru: "Выполняется", tr: "Çalışıyor", de: "Läuft", vi: "Đang chạy" },
"status.done": { en: "Done", ko: "완료", ja: "完了", zh: "完成", hi: "पूर्ण", es: "Listo", fr: "Terminé", ar: "تم", pt: "Concluído", ru: "Готово", tr: "Tamamlandı", de: "Fertig", vi: "Xong" },
"status.attention": { en: "Needs attention", ko: "확인 필요", ja: "要確認", zh: "需要注意", hi: "ध्यान दें", es: "Requiere atención", fr: "Attention requise", ar: "يتطلب انتباهاً", pt: "Requer atenção", ru: "Требует внимания", tr: "Dikkat gerekli", de: "Aufmerksamkeit nötig", vi: "Cần chú ý" },
```

Set each pane element's `title` attribute to `t("status." + status)` inside
`applyPaneStatusClass`.

- [ ] **Step 5: Type-check + commit**

Run: `npx tsc --noEmit` → no errors.

```bash
git add src/workspace/WorkspaceManager.ts src/workspace/WorkspaceBar.ts src/style.css src/i18n/i18n.ts
git commit -m "feat(status): show pane status on border + workspace tab dot"
```

---

# FEATURE 2 — Persistent Scrollback

### Task 4: Backend scrollback commands + `persist_scrollback` config

**Files:**
- Modify: `src-tauri/src/config/model.rs` (`Config.persist_scrollback: bool`)
- Modify: `src-tauri/src/commands.rs` (`save_scrollback`, `load_scrollback`, `delete_scrollback`)
- Modify: `src-tauri/src/main.rs` (register the three commands)
- Modify: `src-tauri/capabilities/default.json` (allow the commands)

**Interfaces:**
- Produces (TS-visible commands):
  `save_scrollback(paneId: string, blob: string) -> ()`,
  `load_scrollback(paneId: string) -> string` (empty if none),
  `delete_scrollback(paneId: string) -> ()`.

- [ ] **Step 1: Add the config field with a defaulting test**

In `src-tauri/src/config/model.rs`, add to `Config` (mirror `notify_on_bell` at 40-41):

```rust
    #[serde(default = "default_persist_scrollback")]
    pub persist_scrollback: bool,
```

Add the default fn next to `default_notify_on_bell`:

```rust
fn default_persist_scrollback() -> bool {
    true
}
```

Set `persist_scrollback: true` in `Config::default()` and in EVERY `Config {
... }` literal inside the `#[cfg(test)]` module (there are several — the
compiler will flag each missing field).

Add a test mirroring `notify_on_bell_defaults_true_when_absent`:

```rust
    #[test]
    fn persist_scrollback_defaults_true_when_absent() {
        let toml_str = "version = 5\nactive_workspace = 1\n";
        let parsed: Config = toml::from_str(toml_str).expect("deserialize");
        assert!(parsed.persist_scrollback);
    }
```

- [ ] **Step 2: Run the config test — verify it passes and nothing else broke**

Run: `cargo test --no-default-features --lib -p ymux config::model`
Expected: PASS, including the new test.

- [ ] **Step 3: Write the scrollback commands**

Add to `src-tauri/src/commands.rs` (after `notify`, following its `YmuxResult`
style). Files live under the config dir, in a `scrollback/` subfolder keyed by
pane id:

```rust
use std::path::PathBuf;

fn scrollback_dir() -> PathBuf {
    dirs::config_dir()
        .map(|p| p.join("ymux").join("scrollback"))
        .unwrap_or_else(|| PathBuf::from("./ymux-scrollback"))
}

fn scrollback_file(pane_id: &str) -> PathBuf {
    // pane_id is a UUID string; reject anything with path separators.
    let safe: String = pane_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit() || *c == '-')
        .collect();
    scrollback_dir().join(format!("{safe}.txt"))
}

#[tauri::command]
pub fn save_scrollback(pane_id: String, blob: String) -> YmuxResult<()> {
    let dir = scrollback_dir();
    std::fs::create_dir_all(&dir).map_err(YmuxError::Io)?;
    // Cap at ~256 KB: keep the tail (most recent output).
    const CAP: usize = 256 * 1024;
    let bytes = blob.as_bytes();
    let slice = if bytes.len() > CAP { &bytes[bytes.len() - CAP..] } else { bytes };
    std::fs::write(scrollback_file(&pane_id), slice).map_err(YmuxError::Io)?;
    Ok(())
}

#[tauri::command]
pub fn load_scrollback(pane_id: String) -> YmuxResult<String> {
    match std::fs::read_to_string(scrollback_file(&pane_id)) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(YmuxError::Io(e)),
    }
}

#[tauri::command]
pub fn delete_scrollback(pane_id: String) -> YmuxResult<()> {
    let path = scrollback_file(&pane_id);
    if path.exists() {
        std::fs::remove_file(path).map_err(YmuxError::Io)?;
    }
    Ok(())
}
```

> Confirm `YmuxError::Io(std::io::Error)` is the correct variant (it is used at
> `commands.rs:201`). `dirs` is already a dependency (used by `store.rs`).

- [ ] **Step 4: Register the commands**

In `src-tauri/src/main.rs`, add to the `generate_handler!` list (after
`ymux_lib::commands::notify,` at line 52):

```rust
            ymux_lib::commands::save_scrollback,
            ymux_lib::commands::load_scrollback,
            ymux_lib::commands::delete_scrollback,
```

Add matching allow entries in `src-tauri/capabilities/default.json` next to the
existing `"notify"`-style permissions (grep the file for how `notify` is listed
and mirror the three names).

- [ ] **Step 5: Add a Rust round-trip unit test**

Add to the `#[cfg(test)]` module of `commands.rs` (create one if absent). This
is Linux-safe (pure fs):

```rust
#[cfg(test)]
mod scrollback_tests {
    use super::*;

    #[test]
    fn scrollback_file_sanitizes_pane_id() {
        let p = scrollback_file("../../evil");
        assert!(!p.to_string_lossy().contains(".."));
    }
}
```

- [ ] **Step 6: Check + commit**

Run: `cargo check --no-default-features --lib --tests -p ymux` (Linux-safe) → OK.
Run: `cargo test --no-default-features --lib -p ymux scrollback` → PASS.

```bash
git add src-tauri/src/config/model.rs src-tauri/src/commands.rs src-tauri/src/main.rs src-tauri/capabilities/default.json
git commit -m "feat(scrollback): backend save/load/delete commands + persist_scrollback config"
```

---

### Task 5: `@xterm/addon-serialize` + bridge wrappers + manager toggle

**Files:**
- Modify: `package.json` (dependency)
- Modify: `src/ipc/bridge.ts` (TS wrappers)
- Modify: `src/workspace/WorkspaceManager.ts` (`persistScrollback` getter/setter, mirror `notifyOnBell`)

**Interfaces:**
- Produces: `api.saveScrollback(id, blob)`, `api.loadScrollback(id)`,
  `api.deleteScrollback(id)`; `manager.persistScrollback` / `manager.setPersistScrollback(v)`.

- [ ] **Step 1: Add the addon dependency**

```bash
pnpm add @xterm/addon-serialize@^0.13.0
```

Expected: `package.json` dependencies gain `@xterm/addon-serialize`.

- [ ] **Step 2: Add bridge wrappers**

In `src/ipc/bridge.ts`, mirror the existing `invoke`-based wrappers (e.g. the
one for `notify`):

```ts
export function saveScrollback(paneId: Uuid, blob: string): Promise<void> {
  return invoke("save_scrollback", { paneId, blob });
}
export function loadScrollback(paneId: Uuid): Promise<string> {
  return invoke("load_scrollback", { paneId });
}
export function deleteScrollback(paneId: Uuid): Promise<void> {
  return invoke("delete_scrollback", { paneId });
}
```

> Tauri auto-converts snake_case command params from camelCase JS keys, matching
> the existing wrappers. Verify the arg style against a neighbouring wrapper.

- [ ] **Step 3: Add the manager getter/setter mirroring `notifyOnBell`**

Grep `notifyOnBell` in `WorkspaceManager.ts` and mirror it exactly for
`persistScrollback` (backed by the config field, persisted via the same
save-config path):

```ts
  get persistScrollback(): boolean { return this._config.persist_scrollback ?? true; }
  setPersistScrollback(v: boolean): void {
    this._config.persist_scrollback = v;
    this.persistDebounced();
  }
```

> Match the actual field/method names used by `notifyOnBell` in this file
> (`this._config` may be named differently — mirror what's there).

- [ ] **Step 4: Type-check + commit**

Run: `npx tsc --noEmit` → no errors.

```bash
git add package.json pnpm-lock.yaml src/ipc/bridge.ts src/workspace/WorkspaceManager.ts
git commit -m "feat(scrollback): add serialize addon, bridge wrappers, manager toggle"
```

---

### Task 6: Serialize on activity, restore on mount, delete on kill

**Files:**
- Modify: `src/terminal/TerminalPane.ts`

**Interfaces:**
- Consumes: `SerializeAddon`, `api.saveScrollback/loadScrollback/deleteScrollback`,
  `manager.persistScrollback` (passed in via an option `persistScrollback: () => boolean`).

- [ ] **Step 1: Load the serialize addon**

Import and register alongside the existing addons (fit/search/webgl/canvas):

```ts
import { SerializeAddon } from "@xterm/addon-serialize";
// ...
  private serializeAddon = new SerializeAddon();
// after this.term.open(...):
  this.term.loadAddon(this.serializeAddon);
```

- [ ] **Step 2: Restore prior scrollback on mount (before wiring live PTY data)**

In `spawn()` (or wherever the pane first attaches to the PTY stream), before the
first live write, replay saved content above a localized separator:

```ts
    if (this.opts.persistScrollback?.()) {
      try {
        const prior = await api.loadScrollback(this.spec.id);
        if (prior) {
          this.term.write(prior);
          this.term.write(`\r\n\x1b[2m${t("terminal.sessionRestored")}\x1b[0m\r\n`);
        }
      } catch { /* no prior scrollback */ }
    }
```

- [ ] **Step 3: Debounced save on output + save on window close**

Add a debounced saver driven by the same output handler that already calls
`statusMachine.onOutput`:

```ts
  private scrollbackSaveTimer: number | undefined;
  private scheduleScrollbackSave(): void {
    if (!this.opts.persistScrollback?.()) return;
    if (this.scrollbackSaveTimer !== undefined) window.clearTimeout(this.scrollbackSaveTimer);
    this.scrollbackSaveTimer = window.setTimeout(() => {
      void api.saveScrollback(this.spec.id, this.serializeAddon.serialize());
    }, 2000);
  }
```

Call `this.scheduleScrollbackSave()` in the PTY-output handler. Register a
one-time flush on window close (in the constructor):

```ts
    window.addEventListener("beforeunload", () => {
      if (this.opts.persistScrollback?.()) {
        void api.saveScrollback(this.spec.id, this.serializeAddon.serialize());
      }
    });
```

- [ ] **Step 4: Delete on permanent close**

In the teardown path invoked by `kill_pane` (the method that disposes the
terminal when the user closes the pane — NOT on app shutdown), add:

```ts
    void api.deleteScrollback(this.spec.id);
```

> Ensure this runs only from the explicit close path. The window-close/app-exit
> path must NOT delete (that's what enables restore). If a single `dispose()`
> serves both, add a boolean arg `dispose(permanent: boolean)` and only delete
> when `permanent`.

- [ ] **Step 5: Add the separator i18n key (13 languages)**

```ts
"terminal.sessionRestored": { en: "── session restored ──", ko: "── 세션 복원됨 ──", ja: "── セッション復元 ──", zh: "── 会话已恢复 ──", hi: "── सत्र पुनर्स्थापित ──", es: "── sesión restaurada ──", fr: "── session restaurée ──", ar: "── تمت استعادة الجلسة ──", pt: "── sessão restaurada ──", ru: "── сессия восстановлена ──", tr: "── oturum geri yüklendi ──", de: "── Sitzung wiederhergestellt ──", vi: "── phiên đã khôi phục ──" },
```

- [ ] **Step 6: Type-check + commit**

Run: `npx tsc --noEmit` → no errors.

```bash
git add src/terminal/TerminalPane.ts src/i18n/i18n.ts
git commit -m "feat(scrollback): serialize + restore per pane with session-restored separator"
```

---

### Task 7: Settings toggle for scrollback persistence

**Files:**
- Modify: `src/settings/SettingsOverlay.ts` (new toggle row)
- Modify: `src/i18n/i18n.ts` (label key)

- [ ] **Step 1: Add the toggle row (mirror the notify row at 213-227)**

```ts
    const scrollbackRow = document.createElement("div");
    scrollbackRow.className = "settings-row";
    const scrollbackLabel = document.createElement("div");
    scrollbackLabel.className = "settings-row__label";
    scrollbackLabel.textContent = t("settings.general.persistScrollback");
    scrollbackRow.appendChild(scrollbackLabel);
    const scrollbackToggle = document.createElement("input");
    scrollbackToggle.type = "checkbox";
    scrollbackToggle.checked = manager.persistScrollback;
    scrollbackToggle.addEventListener("change", () => {
      manager.setPersistScrollback(scrollbackToggle.checked);
    });
    scrollbackRow.appendChild(scrollbackToggle);
    scrollbackRow.appendChild(document.createElement("div"));
    host.appendChild(scrollbackRow);
```

- [ ] **Step 2: Add the label i18n key (13 languages)**

```ts
"settings.general.persistScrollback": { en: "Persist terminal scrollback", ko: "터미널 스크롤백 유지", ja: "ターミナルのスクロールバックを保持", zh: "保留终端回滚缓冲", hi: "टर्मिनल स्क्रॉलबैक सहेजें", es: "Conservar el historial del terminal", fr: "Conserver l'historique du terminal", ar: "الاحتفاظ بمخزّن التمرير للطرفية", pt: "Manter o histórico do terminal", ru: "Сохранять историю терминала", tr: "Terminal kaydırma geçmişini koru", de: "Terminal-Scrollback beibehalten", vi: "Giữ lịch sử cuộn terminal" },
```

- [ ] **Step 3: Type-check + commit**

Run: `npx tsc --noEmit` → no errors.

```bash
git add src/settings/SettingsOverlay.ts src/i18n/i18n.ts
git commit -m "feat(scrollback): add Settings toggle for scrollback persistence"
```

---

# FEATURE 1 — Worktree Panes

### Task 8: `git/mod.rs` — pure worktree helpers (Linux-testable)

**Files:**
- Create: `src-tauri/src/git/mod.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod git;`)

**Interfaces:**
- Produces:
  - `pub struct WorktreeEntry { pub path: String, pub branch: String }`
  - `pub fn is_git_repo(cwd: &Path) -> bool`
  - `pub fn repo_root(cwd: &Path) -> YmuxResult<PathBuf>`
  - `pub fn worktree_add(repo: &Path, branch: &str, path: &Path) -> YmuxResult<()>`
  - `pub fn worktree_remove(path: &Path, force: bool) -> YmuxResult<()>`
  - `pub fn worktree_list(repo: &Path) -> YmuxResult<Vec<WorktreeEntry>>`
  - `pub fn parse_worktree_porcelain(out: &str) -> Vec<WorktreeEntry>` (pure, tested)
  - `pub fn suggested_worktree_path(repo: &Path, branch: &str, base: &str) -> PathBuf` (pure, tested)

- [ ] **Step 1: Write failing tests for the pure helpers**

```rust
// at the bottom of src-tauri/src/git/mod.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_porcelain_worktree_list() {
        let out = "\
worktree /home/u/repo
HEAD abc
branch refs/heads/main

worktree /home/u/.ymux-worktrees/agent-1
HEAD def
branch refs/heads/agent/xyz
";
        let list = parse_worktree_porcelain(out);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].path, "/home/u/.ymux-worktrees/agent-1");
        assert_eq!(list[1].branch, "agent/xyz");
    }

    #[test]
    fn suggested_path_uses_default_base_when_empty() {
        let repo = Path::new("/home/u/repo");
        let p = suggested_worktree_path(repo, "agent/xyz", "");
        // sibling `.ymux-worktrees` dir, branch slashes flattened
        assert!(p.to_string_lossy().replace('\\', "/").ends_with(".ymux-worktrees/agent-xyz"));
    }

    #[test]
    fn suggested_path_honours_custom_base() {
        let repo = Path::new("/home/u/repo");
        let p = suggested_worktree_path(repo, "feature/a", "/tmp/wt");
        assert!(p.to_string_lossy().replace('\\', "/").ends_with("/tmp/wt/feature-a"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --no-default-features --lib -p ymux git::`
Expected: FAIL — module `git` not found / functions undefined.

- [ ] **Step 3: Implement `git/mod.rs`**

```rust
//! Thin wrappers over the `git` binary for worktree operations. We shell out
//! rather than link libgit2 to keep the dependency surface minimal and match
//! whatever git the user already has on PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{YmuxError, YmuxResult};

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorktreeEntry {
    pub path: String,
    pub branch: String,
}

fn run_git(cwd: &Path, args: &[&str]) -> YmuxResult<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(YmuxError::Io)?;
    if !out.status.success() {
        return Err(YmuxError::Pty(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn is_git_repo(cwd: &Path) -> bool {
    Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn repo_root(cwd: &Path) -> YmuxResult<PathBuf> {
    let out = run_git(cwd, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

/// Add a worktree. Attaches to `branch` if it exists, otherwise creates it.
pub fn worktree_add(repo: &Path, branch: &str, path: &Path) -> YmuxResult<()> {
    let path_s = path.to_string_lossy();
    let branch_exists = run_git(repo, &["rev-parse", "--verify", "--quiet", branch]).is_ok();
    if branch_exists {
        run_git(repo, &["worktree", "add", &path_s, branch])?;
    } else {
        run_git(repo, &["worktree", "add", "-b", branch, &path_s])?;
    }
    Ok(())
}

pub fn worktree_remove(path: &Path, force: bool) -> YmuxResult<()> {
    let path_s = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_s);
    // `git worktree remove` can run from anywhere inside the repo; use the
    // worktree's own parent as cwd fallback.
    let cwd = path.parent().unwrap_or(path);
    run_git(cwd, &args)?;
    Ok(())
}

pub fn worktree_list(repo: &Path) -> YmuxResult<Vec<WorktreeEntry>> {
    let out = run_git(repo, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_porcelain(&out))
}

/// Parse `git worktree list --porcelain` output into entries.
pub fn parse_worktree_porcelain(out: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut path: Option<String> = None;
    let mut branch = String::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
            branch = String::new();
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
        } else if line.is_empty() {
            if let Some(p) = path.take() {
                entries.push(WorktreeEntry { path: p, branch: std::mem::take(&mut branch) });
            }
        }
    }
    if let Some(p) = path.take() {
        entries.push(WorktreeEntry { path: p, branch });
    }
    entries
}

/// Compute the worktree directory for `branch`. Default base is a sibling
/// `.ymux-worktrees` dir next to the repo; branch slashes are flattened to
/// dashes so the path is a single directory.
pub fn suggested_worktree_path(repo: &Path, branch: &str, base: &str) -> PathBuf {
    let flat = branch.replace('/', "-");
    if base.is_empty() {
        let parent = repo.parent().unwrap_or(repo);
        parent.join(".ymux-worktrees").join(flat)
    } else {
        PathBuf::from(base).join(flat)
    }
}
```

Add to `src-tauri/src/lib.rs` module declarations:

```rust
pub mod git;
```

> `YmuxError::Pty(String)` is reused for git failures to avoid adding a variant;
> if a dedicated `YmuxError::Git(String)` is preferred, add it to `error.rs` and
> use it consistently. Keep it one choice.

- [ ] **Step 4: Run tests — verify pass**

Run: `cargo test --no-default-features --lib -p ymux git::`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/git/mod.rs src-tauri/src/lib.rs
git commit -m "feat(git): worktree add/remove/list helpers with porcelain parsing"
```

---

### Task 9: Tauri commands for git worktree ops

**Files:**
- Modify: `src-tauri/src/commands.rs` (`git_is_repo`, `git_worktree_add`, `git_worktree_remove`, `git_worktree_list`)
- Modify: `src-tauri/src/main.rs` (register)
- Modify: `src-tauri/capabilities/default.json` (allow)

**Interfaces:**
- Produces (TS-visible):
  `git_is_repo(cwd: string) -> bool`,
  `git_worktree_add(cwd: string, branch: string, base: string) -> string` (returns the created worktree path),
  `git_worktree_remove(path: string, force: bool) -> ()`,
  `git_worktree_list(cwd: string) -> WorktreeEntry[]`.

- [ ] **Step 1: Write the commands**

```rust
use crate::git;
use std::path::Path;

#[tauri::command]
pub fn git_is_repo(cwd: String) -> bool {
    git::is_git_repo(Path::new(&cwd))
}

#[tauri::command]
pub fn git_worktree_add(cwd: String, branch: String, base: String) -> YmuxResult<String> {
    let repo = git::repo_root(Path::new(&cwd))?;
    let path = git::suggested_worktree_path(&repo, &branch, &base);
    git::worktree_add(&repo, &branch, &path)?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn git_worktree_remove(path: String, force: bool) -> YmuxResult<()> {
    git::worktree_remove(Path::new(&path), force)
}

#[tauri::command]
pub fn git_worktree_list(cwd: String) -> YmuxResult<Vec<git::WorktreeEntry>> {
    let repo = git::repo_root(Path::new(&cwd))?;
    git::worktree_list(&repo)
}
```

- [ ] **Step 2: Register in `main.rs`**

Add after the scrollback commands in `generate_handler!`:

```rust
            ymux_lib::commands::git_is_repo,
            ymux_lib::commands::git_worktree_add,
            ymux_lib::commands::git_worktree_remove,
            ymux_lib::commands::git_worktree_list,
```

Add matching allow entries to `capabilities/default.json`.

- [ ] **Step 3: Check (Linux-safe) + commit**

Run: `cargo check --no-default-features --lib --tests -p ymux` → OK.

```bash
git add src-tauri/src/commands.rs src-tauri/src/main.rs src-tauri/capabilities/default.json
git commit -m "feat(git): Tauri commands for worktree add/remove/list/is_repo"
```

---

### Task 10: `PaneSpec.worktree_path` + `Config.worktree_base_dir` (4-place sync)

**Files:**
- Modify: `src-tauri/src/config/model.rs`
- Modify: `src/types.ts`
- Modify: `src/layout/LayoutTree.ts` (`nodeToSpec`, `paneNode`, `newPane`)
- Modify: `src/workspace/WorkspaceManager.ts` (`findAndMutatePane:828-849`)

- [ ] **Step 1: Write the failing round-trip test (Rust)**

Add to `model.rs` tests:

```rust
    #[test]
    fn panespec_worktree_field_roundtrip() {
        let mut config = Config::default();
        let ws = config.workspace_mut(1);
        if let LayoutNode::Pane(ref mut p) = ws.root {
            p.worktree_path = "C:\\wt\\agent-1".to_string();
        }
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        assert!(toml_str.contains("worktree_path = \"C:\\\\wt\\\\agent-1\""));
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(loaded.workspaces[0].panes()[0].worktree_path, "C:\\wt\\agent-1");
    }
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --no-default-features --lib -p ymux panespec_worktree`
Expected: FAIL — no field `worktree_path`.

- [ ] **Step 3: Add the field (mirror `bg_color` exactly — String + serde default)**

In `PaneSpec` (after `bg_color` at 363-364):

```rust
    /// Non-empty when this pane is rooted in a git worktree created by ymux.
    /// Holds the worktree directory; used to offer cleanup when the pane closes.
    #[serde(default)]
    pub worktree_path: String,
```

Set `worktree_path: String::new()` in `new_default()`, `placeholder()`,
`new_browser()`, the `pane_with_id` test helper, and EVERY explicit `PaneSpec {
... }` literal in tests (compiler flags each). Add `worktree_path` to the
`panespec_all_fields_roundtrip` test's spec and its assertions.

Add the `Config.worktree_base_dir` field (mirror the pattern, empty = default):

```rust
    #[serde(default)]
    pub worktree_base_dir: String,
```

Set `worktree_base_dir: String::new()` in `Config::default()` and every test
`Config { ... }` literal.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test --no-default-features --lib -p ymux config::model`
Expected: PASS (all, including new + updated all-fields test).

- [ ] **Step 5: Sync the 3 frontend places**

`src/types.ts` — add to the `PaneSpec` interface (after `bg_color`):

```ts
  worktree_path?: string;
```

`src/layout/LayoutTree.ts`:
- In `nodeToSpec` (after `bg_color: node.bg_color ?? ""`):

```ts
    worktree_path: node.worktree_path ?? "",
```

- In `paneNode` (after `hotkeys`):

```ts
    worktree_path: spec.worktree_path ?? "",
```

`src/workspace/WorkspaceManager.ts` `findAndMutatePane` (add to BOTH the
`snapshot` object at 828-839 and the write-back at 841-849):

```ts
        worktree_path: root.worktree_path ?? "",   // in snapshot
        // ...
        root.worktree_path = snapshot.worktree_path; // in write-back
```

- [ ] **Step 6: Type-check + commit**

Run: `npx tsc --noEmit` → no errors.

```bash
git add src-tauri/src/config/model.rs src/types.ts src/layout/LayoutTree.ts src/workspace/WorkspaceManager.ts
git commit -m "feat(git): add worktree_path PaneSpec field + worktree_base_dir config (4-place sync)"
```

---

### Task 11: Bridge wrappers + branch-name input modal

**Files:**
- Modify: `src/ipc/bridge.ts`
- Create: `src/workspace/WorktreeModal.ts`

**Interfaces:**
- Produces: `api.gitIsRepo(cwd)`, `api.gitWorktreeAdd(cwd, branch, base)`,
  `api.gitWorktreeRemove(path, force)`, `api.gitWorktreeList(cwd)`;
  `promptWorktreeBranch(suggest: string): Promise<string | null>`.

- [ ] **Step 1: Bridge wrappers**

```ts
export function gitIsRepo(cwd: string): Promise<boolean> {
  return invoke("git_is_repo", { cwd });
}
export function gitWorktreeAdd(cwd: string, branch: string, base: string): Promise<string> {
  return invoke("git_worktree_add", { cwd, branch, base });
}
export function gitWorktreeRemove(path: string, force: boolean): Promise<void> {
  return invoke("git_worktree_remove", { path, force });
}
export interface WorktreeEntry { path: string; branch: string }
export function gitWorktreeList(cwd: string): Promise<WorktreeEntry[]> {
  return invoke("git_worktree_list", { cwd });
}
```

- [ ] **Step 2: Branch input modal**

Simplest correct thing: reuse the existing prompt helper `promptWithBlur`
(imported in `palette/commands.ts:4`) rather than build a new modal:

```ts
// src/workspace/WorktreeModal.ts
import { promptWithBlur } from "../browser/popupBlur";
import { t } from "../i18n/i18n";

export function promptWorktreeBranch(suggest: string): string | null {
  const v = promptWithBlur(t("worktree.branchPrompt"), suggest);
  const trimmed = (v ?? "").trim();
  return trimmed.length ? trimmed : null;
}
```

> If `promptWithBlur` is async, make this `async` and `await` it. Mirror the
> exact signature used at `commands.ts` where `promptWithBlur` is already called.

- [ ] **Step 3: Type-check + commit**

Run: `npx tsc --noEmit` → no errors.

```bash
git add src/ipc/bridge.ts src/workspace/WorktreeModal.ts
git commit -m "feat(git): bridge wrappers + worktree branch prompt"
```

---

### Task 12: `openWorktreePane()` + palette command + i18n

**Files:**
- Modify: `src/workspace/WorkspaceManager.ts` (`openWorktreePane`)
- Modify: `src/palette/commands.ts` (command entry)
- Modify: `src/i18n/i18n.ts` (command label + prompt + toasts)

**Interfaces:**
- Consumes: `api.gitIsRepo/gitWorktreeAdd`, `promptWorktreeBranch`, existing
  `splitFocused` plumbing.
- Produces: `WorkspaceManager.openWorktreePane(direction: SplitDir): Promise<void>`.

- [ ] **Step 1: Implement `openWorktreePane` (mirror `splitFocused:466-504`)**

```ts
  /// Split the focused pane into a fresh git worktree rooted shell.
  async openWorktreePane(direction: SplitDir): Promise<void> {
    const ws = this.active;
    const focusId = this.focusedPaneId ?? panes(ws.root)[0]?.id;
    if (!focusId) return;
    let baseCwd: string | null = null;
    try { baseCwd = await api.getPaneCwd(focusId); } catch { /* fall through */ }
    baseCwd = baseCwd ?? findPane(ws.root, focusId)?.cwd ?? null;
    if (!baseCwd) { this.toast(t("worktree.noCwd")); return; }
    if (!(await api.gitIsRepo(baseCwd))) { this.toast(t("worktree.notRepo")); return; }

    const suggest = `agent/${crypto.randomUUID().slice(0, 6)}`;
    const branch = promptWorktreeBranch(suggest);
    if (!branch) return;

    let wtPath: string;
    try {
      wtPath = await api.gitWorktreeAdd(baseCwd, branch, this.worktreeBaseDir);
    } catch (e) {
      this.toast(t("worktree.addFailed") + " " + String(e));
      return;
    }

    const shellName = this.resolveShell(this.shells[0]?.name ?? "");
    const spec = newPane(shellName, wtPath);
    spec.worktree_path = wtPath;
    ws.root = splitPane(ws.root, focusId, direction, spec);
    const cache = this.paneCaches.get(ws.id)!;
    const pane = this.createPane(spec);
    cache.set(spec.id, pane);
    this.renderWorkspace(ws);
    try { await pane.spawn(); pane.focus(); } catch (e) { console.error("worktree spawn failed", e); }
    this.persistDebounced();
  }
```

Add a `worktreeBaseDir` getter mirroring `persistScrollback`
(`return this._config.worktree_base_dir ?? ""`). If a `toast()` helper does not
exist, use the existing notification/console path used elsewhere for
user-facing errors (grep for how `splitFocused` surfaces failures) — do not
invent a new toast system.

- [ ] **Step 2: Add the palette command (mirror `pane.splitH` at 15-20)**

```ts
    {
      id: "pane.worktree",
      label: () => t("worktree.command"),
      action: () => void manager.openWorktreePane("horizontal"),
    },
```

- [ ] **Step 3: Add i18n keys (13 languages)**

```ts
"worktree.command": { en: "Open pane in new git worktree", ko: "새 git worktree에서 pane 열기", ja: "新しい git worktree でペインを開く", zh: "在新的 git worktree 中打开窗格", hi: "नए git worktree में पेन खोलें", es: "Abrir panel en un nuevo git worktree", fr: "Ouvrir un volet dans un nouveau worktree git", ar: "افتح لوحة في git worktree جديد", pt: "Abrir painel em novo git worktree", ru: "Открыть панель в новом git worktree", tr: "Yeni git worktree'de bölme aç", de: "Bereich in neuem Git-Worktree öffnen", vi: "Mở ô trong git worktree mới" },
"worktree.branchPrompt": { en: "New branch name for the worktree:", ko: "worktree에 사용할 새 브랜치 이름:", ja: "worktree の新しいブランチ名:", zh: "worktree 的新分支名称：", hi: "worktree के लिए नई ब्रांच का नाम:", es: "Nombre de la nueva rama para el worktree:", fr: "Nom de la nouvelle branche pour le worktree :", ar: "اسم الفرع الجديد لـ worktree:", pt: "Nome do novo branch para o worktree:", ru: "Имя новой ветки для worktree:", tr: "Worktree için yeni dal adı:", de: "Neuer Branch-Name für das Worktree:", vi: "Tên nhánh mới cho worktree:" },
"worktree.notRepo": { en: "Current directory is not a git repository", ko: "현재 디렉터리가 git 저장소가 아닙니다", ja: "現在のディレクトリは git リポジトリではありません", zh: "当前目录不是 git 仓库", hi: "वर्तमान डायरेक्टरी git रिपॉज़िटरी नहीं है", es: "El directorio actual no es un repositorio git", fr: "Le répertoire actuel n'est pas un dépôt git", ar: "الدليل الحالي ليس مستودع git", pt: "O diretório atual não é um repositório git", ru: "Текущий каталог не является репозиторием git", tr: "Geçerli dizin bir git deposu değil", de: "Aktuelles Verzeichnis ist kein Git-Repository", vi: "Thư mục hiện tại không phải kho git" },
"worktree.noCwd": { en: "Could not determine the pane's directory", ko: "pane의 디렉터리를 확인할 수 없습니다", ja: "ペインのディレクトリを特定できません", zh: "无法确定窗格的目录", hi: "पेन की डायरेक्टरी निर्धारित नहीं हो सकी", es: "No se pudo determinar el directorio del panel", fr: "Impossible de déterminer le répertoire du volet", ar: "تعذّر تحديد دليل اللوحة", pt: "Não foi possível determinar o diretório do painel", ru: "Не удалось определить каталог панели", tr: "Bölmenin dizini belirlenemedi", de: "Verzeichnis des Bereichs konnte nicht ermittelt werden", vi: "Không xác định được thư mục của ô" },
"worktree.addFailed": { en: "Failed to create worktree:", ko: "worktree 생성 실패:", ja: "worktree の作成に失敗しました:", zh: "创建 worktree 失败：", hi: "worktree बनाने में विफल:", es: "No se pudo crear el worktree:", fr: "Échec de création du worktree :", ar: "فشل إنشاء worktree:", pt: "Falha ao criar o worktree:", ru: "Не удалось создать worktree:", tr: "Worktree oluşturulamadı:", de: "Worktree konnte nicht erstellt werden:", vi: "Không tạo được worktree:" },
```

- [ ] **Step 4: Type-check + commit**

Run: `npx tsc --noEmit` → no errors.

```bash
git add src/workspace/WorkspaceManager.ts src/palette/commands.ts src/i18n/i18n.ts
git commit -m "feat(git): openWorktreePane action + palette command + i18n"
```

---

### Task 13: Remove-worktree-on-close confirmation

**Files:**
- Modify: `src/workspace/WorkspaceManager.ts` (`closeFocused` / pane-close path)
- Modify: `src/i18n/i18n.ts` (confirm strings)

- [ ] **Step 1: Hook the close path**

In the method that closes a pane (`closeFocused`, referenced at
`palette/commands.ts:31`), before removing the pane, check for a worktree tag:

```ts
    const spec = findPane(ws.root, id);
    if (spec?.worktree_path) {
      const ok = confirm(t("worktree.removeConfirm").replace("{path}", spec.worktree_path));
      if (ok) {
        try {
          await api.gitWorktreeRemove(spec.worktree_path, false);
        } catch {
          // Dirty worktree — offer a forced removal.
          if (confirm(t("worktree.removeForce"))) {
            try { await api.gitWorktreeRemove(spec.worktree_path, true); } catch (e) {
              console.error("worktree remove failed", e);
            }
          }
        }
      }
    }
    // ...existing pane-removal logic unchanged...
```

> Use whatever confirm/dialog primitive the codebase already uses (grep for
> `confirm(` or an existing modal). Do not introduce a new dialog system.

- [ ] **Step 2: Add confirm i18n keys (13 languages)**

```ts
"worktree.removeConfirm": { en: "Remove the git worktree at {path}? The branch is kept.", ko: "{path}의 git worktree를 삭제할까요? 브랜치는 유지됩니다.", ja: "{path} の git worktree を削除しますか？ブランチは残ります。", zh: "删除 {path} 处的 git worktree？分支将保留。", hi: "{path} पर git worktree हटाएँ? ब्रांच बनी रहेगी।", es: "¿Eliminar el worktree git en {path}? La rama se conserva.", fr: "Supprimer le worktree git dans {path} ? La branche est conservée.", ar: "إزالة git worktree في {path}؟ يبقى الفرع.", pt: "Remover o worktree git em {path}? O branch é mantido.", ru: "Удалить git worktree в {path}? Ветка сохранится.", tr: "{path} konumundaki git worktree kaldırılsın mı? Dal korunur.", de: "Git-Worktree unter {path} entfernen? Der Branch bleibt erhalten.", vi: "Xóa git worktree tại {path}? Nhánh được giữ lại." },
"worktree.removeForce": { en: "Worktree has uncommitted changes. Force removal?", ko: "worktree에 커밋되지 않은 변경이 있습니다. 강제로 삭제할까요?", ja: "worktree に未コミットの変更があります。強制的に削除しますか？", zh: "worktree 有未提交的更改。强制删除？", hi: "worktree में बिना कमिट किए बदलाव हैं। जबरन हटाएँ?", es: "El worktree tiene cambios sin confirmar. ¿Forzar la eliminación?", fr: "Le worktree a des modifications non validées. Forcer la suppression ?", ar: "يحتوي worktree على تغييرات غير مُودعة. فرض الإزالة؟", pt: "O worktree tem alterações não confirmadas. Forçar remoção?", ru: "В worktree есть незакоммиченные изменения. Удалить принудительно?", tr: "Worktree'de kaydedilmemiş değişiklikler var. Zorla kaldırılsın mı?", de: "Worktree hat nicht committete Änderungen. Entfernung erzwingen?", vi: "Worktree có thay đổi chưa commit. Buộc xóa?" },
```

- [ ] **Step 3: Type-check + commit**

Run: `npx tsc --noEmit` → no errors.

```bash
git add src/workspace/WorkspaceManager.ts src/i18n/i18n.ts
git commit -m "feat(git): confirm + remove worktree when a worktree pane closes"
```

---

# FINAL — Task 14: Version bump to v0.8.19 + full verification

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `package.json`, `crates/yversion/src/lib.rs`, `README.md`, `README.ko.md`, `README.ja.md`

- [ ] **Step 1: Bump all six version points to 0.8.19**

- `src-tauri/Cargo.toml` → `version = "0.8.19"`
- `src-tauri/tauri.conf.json` → `"version": "0.8.19"`
- `package.json` → `"version": "0.8.19"`
- `crates/yversion/src/lib.rs` → `pub const VERSION: &str = "0.8.19";`
- README badge URLs in `README.md`, `README.ko.md`, `README.ja.md` → `0.8.19`

- [ ] **Step 2: Regenerate lockfile**

Run: `cargo check --no-default-features --lib --tests -p ymux`
Expected: OK; `Cargo.lock` updated to 0.8.19.

- [ ] **Step 3: Run the full test suite**

Run each and confirm PASS:
```sh
npx tsc --noEmit
cargo test --no-default-features --lib -p ymux
cargo test -p ytheme -p yipc -p ymon -p ydir -p ycode -p ylauncher
cargo fmt --all --check
cargo clippy --no-default-features --lib --tests -p ymux -- -D warnings
pnpm exec vitest run
```

- [ ] **Step 4: Runtime smoke test (Windows)**

Run: `pnpm tauri dev`, then manually verify:
- Palette → "Open pane in new git worktree" creates a worktree pane in a repo.
- A long CLI (`claude`/`codex`) shows `running` then `attention` when unfocused.
- Restart the app → prior pane shows restored scrollback above the separator.
- Closing a worktree pane prompts to remove the worktree.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: bump to v0.8.19 — worktree panes, persistent scrollback, agent status"
```
