# ymux v0.11.1

ymux now ships as a single program. The last helper binary, `y`, is gone: agent tracking talks to Claude Code directly over its built-in HTTP hooks, so there is nothing left to install on `PATH`, bundle beside `ymux.exe`, or keep in sync.

## Breaking: `y` is gone

`y` was the last survivor of the old TUI tool family. After v0.11.0 it did exactly one job — relaying Claude Code hook events into the agent tree — and that job now happens inside ymux. Typing `y` anywhere now fails with "command not found".

**If you had agent tracking turned on, there is nothing to do.** On first launch ymux rewrites its own entries in `~/.claude/settings.json`: the old `y agent-hook …` commands are removed and replaced with HTTP hooks. Every other hook and setting in that file is left exactly as it was, key order included, and the original is still backed up once as `settings.json.ymux-bak`.

## How agent tracking works now

When **Settings → Track Claude Code agents** is on, ymux listens on a local port (`127.0.0.1` only) and registers it with Claude Code as an HTTP hook. Each terminal pane carries its pane id and a per-launch secret in its environment, and Claude Code sends both along with every event, so ymux knows which pane a session belongs to and ignores anything it didn't issue.

What you'll see is unchanged: the agent tree shows each Claude session, its subagents, and whether it's working, waiting for your approval, or done.

Two things differ from before, both only while tracking is on:

- **Claude sessions outside ymux send their hook events to that local port too.** The hooks are part of your user-wide Claude Code settings, so every session posts to it, not just the ones in ymux panes. ymux discards anything that doesn't carry a pane's secret. Turning tracking off removes the hooks.
- **If ymux isn't running, other Claude sessions show a hook notice each turn** ("Stop hook error occurred · ctrl+o to see"). The session is not blocked or slowed; it's Claude Code reporting that nothing answered. Turn tracking off if you'd rather not see it.

A session's id now arrives with your first prompt rather than when Claude starts, because Claude Code doesn't send HTTP hooks for session start.

If you run two copies of ymux, the second one sees the first already listening and leaves the port and your settings alone; its panes simply don't feed the agent tree. Nothing moves the port away from a running ymux.

## Security

The hook listener was reviewed independently before release:

- It accepts connections from this machine only, and only requests carrying a pane's per-launch secret. A web page — in your browser or in a ymux browser pane — can't forge an event.
- It serves at most 32 connections at once, each with a hard time limit, so a local process can't tie it up.
- A session id that arrives by hook is used for **session resume** only when the pane's own Claude process confirms it. A forged event can't point a pane at a different conversation. Resumed Claude sessions still start with `--dangerously-skip-permissions`, as before.
- If you keep your own `allowedHttpHookUrls` list in Claude Code's settings, ymux adds only its exact URL to it and removes that same entry when tracking is turned off.

## For builders

- `tools/ylauncher`, `crates/yipc`, `src-tauri/src/ipc_server.rs`, `scripts/build-tools.mjs` and the `externalBin` bundle entries are deleted. `cargo check -p ymux` no longer needs placeholder sidecar files, and CI's dummy-sidecar step is gone. `YMUX_TARGET_TRIPLE` is no longer used.
- New `src-tauri/src/hook_http.rs` (desktop-gated loopback receiver) and a backend-owned `agent_hook_port` setting that, like `agent_tracking`, is deliberately not carried by `merge_layouts_from`.
- CLAUDE.md rules 4 and 11–13 are rewritten for the new design.
- Tests: `ymux` lib 449 passing on Windows (8 known `pty::osc7` failures there, which pass on Linux/macOS), `ytheme` 6, `ypath` 9, frontend 616.
