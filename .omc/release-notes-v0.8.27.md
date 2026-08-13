# ymux v0.8.27

v0.8.26 gave the workspace rows a status colour. This release makes that colour actually tell you when your agents finish.

## Fixes

### 🟢 Finishing a command now shows green instead of nothing

A workspace row turned blue while an agent worked, then went **untinted** when it finished — the same as a workspace where nothing had ever run. The one moment the indicator exists for was the one moment it said nothing.

The cause: completion was only ever signalled by a terminal bell / OSC 9, and most CLIs never send one. With no bell, a command that simply stopped printing hit the four-second quiet window and decayed straight from `running` back to `idle`. Green was reachable only by tools that announce themselves.

Silence now decays to **`done`** instead. That inference can of course be wrong — a build step can go quiet for ten seconds and still be working — so it is revocable: if output resumes, the pane returns to `running`. A `done` that came from a *real* bell is never revoked, so the trailing prompt and spinner repaints a TUI emits right after it finishes can't erase a genuine completion signal.

### 👀 "Did the user see it?" is now decided by what's on screen

The seen/unseen judgement — green *done* versus pulsing orange *attention* — was made by asking whether that exact pane held the keyboard focus.

That is the wrong question twice over. **Every pane in a split is on screen simultaneously**, so which one owns the cursor says nothing about what the user saw. And an agent finishing in the very workspace you were looking at counted as unseen unless your cursor happened to be sitting in its pane.

Status now asks whether the pane is *visible*: the window has focus and its workspace is the active one. The OS notification deliberately keeps the stricter test — being able to glimpse a pane in the corner of a split is not a reason to swallow the alert for something you launched yourself.

### ⏱️ Green is an unread marker, and behaves like one

`done` used to persist until the pane was refocused, which meant it never cleared at all on a pane that was already focused. Its lifetime now depends on whether you can see it:

| | |
|---|---|
| **Off screen** | held indefinitely — this workspace finished something and you haven't looked |
| **On screen** | fades after 8 seconds — long enough to notice, short enough that green doesn't become the resting colour of every pane sitting at a prompt |

The on-screen expiry doubles as the clearing mechanism: switch to a workspace showing green and the marker retires itself a moment later, with no focus-stealing on workspace activation.

`attention` is untouched by the timer. The loud signal still wants an explicit acknowledgement, so only focusing the pane clears it.

## The resulting colours

| Situation | Row |
|---|---|
| Command or agent running | blue |
| Finished while you were looking | green, ~8s |
| Finished while you were elsewhere | green, held until you visit |
| Bell / OSC 9 while you were elsewhere | orange, pulsing, until you click the pane |
| Nothing has run | untinted |

## Under the hood

- `PaneStatusMachine.onAttention()` and `.tick()` both take their arguments explicitly now. The previous `now = 0` default would have silently expired the hold on its first tick if a caller forgot to pass a clock — a defaulted parameter that fails quietly is worse than a compile error.
- `WorkspaceManager` grew a second, weaker predicate. `isPaneVisible()` (window focused + workspace active) drives status; `isWatching()` (that, plus pane focused) still gates the notification and beep. Two questions that were always distinct, now spelled differently.
- Status machine tests went from 8 to 15, covering the quiet decay, the off-screen hold, the on-screen expiry, losing visibility part-way through the hold, and both revoke cases — the inferred `done` that output takes back, and the bell-driven `done` that it must not.

## Compatibility

Drop-in over v0.8.26. No config, keybinding, or data changes.

## Install

Grab the Windows MSI from the Assets below, or build from source:

```sh
git clone https://github.com/YoungMins/ymux
cd ymux
pnpm install
pnpm tauri build
```

Verified: `npx tsc --noEmit` clean, `npx vitest run` — 57 tests passing, `npx vite build` clean. No Rust logic changed; the only Rust diff is the version constant.

---

**Full Changelog**: https://github.com/YoungMins/ymux/compare/v0.8.26...v0.8.27
