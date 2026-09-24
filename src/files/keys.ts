// The files pane's keymap, as a pure function of a key event. Kept apart
// from the DOM so the precedence rule of spec §0.6 — ymux's global chords
// always win — is a tested predicate rather than a convention.
//
// `isMac` is a parameter (callers pass `IS_MAC` from src/platform.ts) so both
// platforms are testable from one run. The primary modifier is Cmd on macOS
// and Ctrl elsewhere, exactly as `hasMod` defines it (rule 10).

export interface KeyLike {
  key: string;
  code: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  isComposing: boolean;
}

export type PaneKeyAction =
  | { kind: "move"; delta: number; extend: boolean }
  | { kind: "edge"; end: boolean; extend: boolean }
  | { kind: "page"; dir: 1 | -1; extend: boolean }
  | { kind: "open" }
  | { kind: "parent" }
  | { kind: "rename" }
  | { kind: "delete"; permanent: boolean }
  | { kind: "selectAll" }
  | { kind: "copy" }
  | { kind: "cut" }
  | { kind: "paste" }
  | { kind: "refresh" }
  | { kind: "editPath" }
  | { kind: "newFolder" }
  | { kind: "togglePreview" }
  | { kind: "escape" }
  | { kind: "typeAhead"; text: string };

function mod(ev: KeyLike, isMac: boolean): boolean {
  return isMac ? ev.metaKey : ev.ctrlKey;
}

/// Is this a chord ymux's window-level handler owns (src/main.ts)?
///
/// Deliberately broader than today's list: *every* mod+Shift and mod+Alt
/// chord is reserved, so a global shortcut added later cannot be silently
/// eaten by this pane. The pane therefore binds nothing under those.
export function isGlobalChord(ev: KeyLike, isMac: boolean): boolean {
  const m = mod(ev, isMac);
  // Ctrl+Tab / Ctrl+Shift+Tab stay on Ctrl even on macOS (platform.ts).
  if (ev.ctrlKey && ev.key === "Tab") return true;
  // Workspace switch: Ctrl+Alt+digit on Windows, Cmd+digit on macOS.
  if (/^Digit[1-9]$/.test(ev.code) && !ev.shiftKey) {
    if (isMac ? ev.metaKey && !ev.ctrlKey && !ev.altKey : ev.ctrlKey && ev.altKey) return true;
  }
  if (!m) return false;
  if (ev.shiftKey || ev.altKey) return true;
  // Scrollback search and font size.
  if (ev.code === "KeyF") return true;
  if (["Equal", "NumpadAdd", "Minus", "NumpadSubtract", "Digit0", "Numpad0"].includes(ev.code)) {
    return true;
  }
  return false;
}

/// The pane's action for `ev`, or `null` to let it through untouched (no
/// `preventDefault`, no `stopPropagation`). Letter chords match `ev.code`,
/// so a Korean or AZERTY layout still copies with the C key.
export function paneKeyAction(ev: KeyLike, isMac: boolean): PaneKeyAction | null {
  if (ev.isComposing || ev.key === "Process") return null;
  if (isGlobalChord(ev, isMac)) return null;
  const m = mod(ev, isMac);
  const extend = ev.shiftKey;

  if (m) {
    switch (ev.code) {
      case "KeyA":
        return { kind: "selectAll" };
      case "KeyC":
        return { kind: "copy" };
      case "KeyX":
        return { kind: "cut" };
      case "KeyV":
        return { kind: "paste" };
      case "KeyR":
        return { kind: "refresh" };
      case "KeyL":
        return { kind: "editPath" };
    }
    // macOS has no Delete key on laptop keyboards; Cmd+Backspace is Finder's
    // "move to trash".
    if (ev.key === "Backspace") return { kind: "delete", permanent: false };
    if (ev.key === "ArrowUp") return { kind: "parent" };
    return null;
  }
  // A bare Ctrl chord on macOS belongs to nobody here.
  if (ev.ctrlKey || ev.metaKey) return null;

  if (ev.altKey) return ev.key === "ArrowUp" ? { kind: "parent" } : null;

  switch (ev.key) {
    case "ArrowDown":
      return { kind: "move", delta: 1, extend };
    case "ArrowUp":
      return { kind: "move", delta: -1, extend };
    case "Home":
      return { kind: "edge", end: false, extend };
    case "End":
      return { kind: "edge", end: true, extend };
    case "PageDown":
      return { kind: "page", dir: 1, extend };
    case "PageUp":
      return { kind: "page", dir: -1, extend };
    case "Enter":
      return { kind: "open" };
    case "Backspace":
      return { kind: "parent" };
    case "F2":
      return { kind: "rename" };
    case "Delete":
      return { kind: "delete", permanent: ev.shiftKey };
    case "F5":
      return { kind: "refresh" };
    case "F7":
      return { kind: "newFolder" };
    case "Tab":
      return ev.shiftKey ? null : { kind: "togglePreview" };
    case "Escape":
      return { kind: "escape" };
  }
  // One printable character, not a space (space is no-op, not a name start).
  if ([...ev.key].length === 1 && ev.key !== " ") return { kind: "typeAhead", text: ev.key };
  return null;
}
