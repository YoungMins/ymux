// The git pane's keymap, as a pure function of a key event — the files
// pane's pattern (src/files/keys.ts), including its precedence rule: any
// chord `isGlobalChord` reserves for ymux is let through untouched (§0.6).
//
// ygit's six keys carry over (j/k and arrows, Tab, Enter, r) plus the
// worktree actions ygit never had. Letters match `ev.code`, so they work on
// a Korean layout too.

import { isGlobalChord, type KeyLike } from "../files/keys";

export type GitKeyAction =
  | { kind: "move"; delta: number }
  | { kind: "page"; dir: 1 | -1 }
  | { kind: "edge"; end: boolean }
  /// Next / previous section: log → branches → worktrees.
  | { kind: "section"; dir: 1 | -1 }
  /// Enter: check out the branch, view the worktree.
  | { kind: "activate" }
  | { kind: "refresh" }
  | { kind: "newWorktree" }
  | { kind: "remove" }
  /// Open a terminal in the selected worktree (or the repository).
  | { kind: "terminal" }
  /// Toggle following the active pane's directory.
  | { kind: "pin" }
  /// Copy the selected commit's hash / branch name / worktree path.
  | { kind: "copy" };

export function gitKeyAction(ev: KeyLike, isMac: boolean): GitKeyAction | null {
  if (ev.isComposing || ev.key === "Process") return null;
  if (isGlobalChord(ev, isMac)) return null;
  const mod = isMac ? ev.metaKey : ev.ctrlKey;

  if (mod) {
    if (ev.code === "KeyR") return { kind: "refresh" };
    if (ev.code === "KeyC") return { kind: "copy" };
    // Finder's "move to trash" chord, for keyboards with no Delete key.
    if (ev.key === "Backspace") return { kind: "remove" };
    return null;
  }
  // A bare Ctrl chord on macOS, or any Alt chord, is not ours.
  if (ev.ctrlKey || ev.metaKey || ev.altKey) return null;

  switch (ev.key) {
    case "ArrowDown":
      return { kind: "move", delta: 1 };
    case "ArrowUp":
      return { kind: "move", delta: -1 };
    case "PageDown":
      return { kind: "page", dir: 1 };
    case "PageUp":
      return { kind: "page", dir: -1 };
    case "Home":
      return { kind: "edge", end: false };
    case "End":
      return { kind: "edge", end: true };
    case "Tab":
      return { kind: "section", dir: ev.shiftKey ? -1 : 1 };
    case "Enter":
      return { kind: "activate" };
    case "F5":
      return { kind: "refresh" };
    case "Delete":
      return { kind: "remove" };
  }
  if (ev.shiftKey) return null;
  switch (ev.code) {
    case "KeyJ":
      return { kind: "move", delta: 1 };
    case "KeyK":
      return { kind: "move", delta: -1 };
    case "KeyR":
      return { kind: "refresh" };
    case "KeyN":
      return { kind: "newWorktree" };
    case "KeyT":
      return { kind: "terminal" };
    case "KeyP":
      return { kind: "pin" };
  }
  return null;
}
