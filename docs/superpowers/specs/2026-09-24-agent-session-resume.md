# Agent session resume instead of scrollback replay

Date: 2026-09-24

Today a pane that held a running agent comes back as a picture of one: ymux
replays the saved scrollback, and the agent is gone. This replaces that, for
agent panes only, with a real resume — ymux runs the agent's own resume
command so the conversation continues.

Shell panes are untouched: they keep replaying scrollback exactly as now.

Researched from Orca (github.com/stablyai/orca), whose resume path is the
reference for the parts below that say "as Orca does".

---

## 1. What a resumable session is

Per pane, a record:

```rust
struct AgentSession {
    pane_id: Uuid,
    agent: AgentKind,        // Claude | Codex | …
    session_id: String,      // the agent's own id
    cwd: String,             // raw path as reported (compare via ypath)
    state: AgentStatus,      // last known: working | waiting | done | idle
    interrupted: bool,       // true when the last state was not `done`
    updated_at: SystemTime,
}
```

Kept in a backend-owned store beside `scrollback.rs`
(`src-tauri/src/agent_sessions.rs`, JSON under the config dir), **not** in
`PaneSpec`: rule 2 (four-place sync), rule 3 (`Option<T>` will not round-trip
inside the tagged enum) and rule 11 (a stale frontend save would clobber a
backend-owned value) all apply. Same exception `agent_tracking` already takes.

## 2. Two sources for the session id

**Hooks (Claude, when agent tracking is on).** `y agent-hook` already relays
`session_id` per pane into `AgentRegistry`. The registry writes it into the
store on every hook event, along with the status it already tracks — so
`state`/`interrupted` come for free.

**Disk scan (every agent, no hooks needed).** Each CLI keeps its own
transcripts; ymux finds the newest one whose recorded cwd matches the pane's
OSC 7 cwd. Verified on this machine:

| Agent | Where | Id and cwd |
|---|---|---|
| Claude | `~/.claude/projects/<cwd with `/`,`\`,`:` → `-`>/<session-id>.jsonl` | id is the file stem; cwd is the directory name |
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl` | first line is `session_meta` with `session_id` and `cwd` |
| Gemini | `~/.gemini/history/<hash>/` | format not yet established — out of scope for v1, see below |

Path matching uses `ypath::same_path` (rule 15), never `==`: Claude's
directory mangling and Codex's `cwd` field spell the same directory
differently, and macOS adds NFD.

The disk scan is what makes resume work for a user who never turned agent
tracking on, which is most users — so it is not a fallback, it is the
baseline. Hooks are the faster, more precise source when present.

**Amendment (resume fixes): a record is bound to the agent *process*, not to
"the newest transcript here".** Newest-in-the-folder let two panes swap
conversations and let a pane adopt a Claude running in another terminal. The
rule now (`src-tauri/src/agent_binding.rs`), most exact first: the hook id;
Claude's own `~/.claude/sessions/<pid>.json` (undocumented — believed only for
the pane agent's pid and a `startedAt` matching that process's start); the
process's argv (`--resume <id>`, `--session-id <id>`, `codex resume <id>`);
and only then a transcript whose first recorded `timestamp` is at/after the
process start, earliest first, and only when no other agent process could have
written it. A guess is never changed while the same process lives; a new
process in the pane declines the old record until it is bound itself.

## 3. The resume command

`resume_argv(agent, session_id)`:

| Agent | Command |
|---|---|
| Claude | `claude --resume <id>` |
| Codex | `codex resume <id>` |

Two rules copied from Orca, both load-bearing:

- **Never "continue the most recent"** (`claude -c`, `codex resume --last`).
  An explicit id is the only thing that cannot resume the wrong conversation.
- **Strip any selector already in the pane's `startup_cmd`** before appending
  ours. A saved `claude -c` plus our `--resume <id>` is two selectors fighting.

The command is *typed into the pane's shell* as its startup command, exactly
as a user would type it — not spawned directly. That keeps quoting the
shell's problem and leaves the pane a normal shell after the agent exits.

## 4. When it fires

On pane spawn during app start (and on workspace restore), for a pane whose
record is **fresh** — updated within 24 h — and whose session file still
exists on disk:

1. **Skip the scrollback replay entirely for that pane.** ConPTY clears the
   screen at startup anyway (see the ConPTY memory note), and a resumed agent
   redraws its own history; replaying would put dead backlog above it.
2. Run the resume command as the pane's startup command.
3. Print one line before it: `── 세션 복원 (claude · 3시간 전)` — or, when the
   id was dropped, `── 이전 세션을 찾지 못해 새로 시작합니다`.

No prompt, no modal: automatic, as Orca does. The pane is a terminal; if the
resume is wrong the user can Ctrl-C and start fresh, which is cheaper than a
dialog on every launch.

Records older than 24 h, or whose session file is gone, are ignored and the
pane starts fresh with its normal `startup_cmd` — the record is kept, not
deleted, so a later fix can still use it (Orca's "decline, don't delete").

## 5. What stops being saved

For a pane with a fresh record, scrollback is neither saved on exit nor
restored on start. `shouldSaveScrollback` gains that condition, and
`spawn()`'s restore path skips it. Everything else about scrollback
persistence stays as it is, for shell panes.

**Amendment (resume fixes):** the backend alone decides (`save_scrollback` →
`SessionTracker::scrollback_action`), and a resumed pane's old blob is deleted
only once the scan sees the resumed agent running under an exact id (argv, pid
file or hook). Until then it is left alone; a resume that shows no running
agent within `RESUME_CONFIRM_WINDOW` (90 s) is declined, so it is not retried
on the next launch and that launch restores the old blob instead.

## 6. Components

- Rust: `agent_sessions.rs` (store + staleness + pure `resume_argv` and the
  selector-stripping fn — not desktop-gated so they are unit-testable),
  `agent_scan_disk.rs` (transcript scanners per agent; pure parsers, the
  directory walk desktop-gated), `agents.rs` (write the hook-borne id into
  the store), `commands.rs` (`get_agent_session(pane_id)`,
  `clear_agent_session(pane_id)`).
- TS: `TerminalPane.spawn()` asks for a resume plan before deciding to
  restore scrollback; the banner line; `WorkspaceManager` passes it through.

## 7. Tests

- Rust: `resume_argv` per agent; selector stripping (`claude -c`,
  `claude --resume old`, `codex resume x`, quoted args, nothing to strip);
  staleness boundary; Claude directory-name mangling round-trip against a
  real path; Codex `session_meta` parsing from a captured first line; picking
  the newest of several transcripts for the same cwd; a transcript whose cwd
  differs only by NFD/NFC or case (must match on Windows drive paths, must
  not match on POSIX paths — rule 15).
- vitest: the decision "restore scrollback vs resume" as a pure function.
- Manual: quit ymux mid-turn with Claude running, reopen — the conversation
  continues and no stale screen appears above it; the same with Codex; a
  shell pane still restores its scrollback.

## 8. Out of scope

Gemini and other agents (the scanner interface is per-agent, so adding one is
a contained change once its format is established); resuming across machines;
recovering a session whose transcript the agent itself has pruned; any UI for
browsing past sessions.
