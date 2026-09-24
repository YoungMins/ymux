// Shortcuts an embedded browser pane may forward to the main window, and the
// KeyboardEvent to replay for each.
//
// The payload comes (via Rust's `forward_keystroke`) from a website's
// webview, so it is treated as untrusted even though Rust has already
// validated it: only an exact (code, modifiers) combination from this table is
// replayed, and the `key` is derived here rather than read from the payload —
// main.ts's keydown handler dispatches on `ev.key` for most bindings, so a
// payload's own `key` could otherwise make one shortcut act as another.
//
// Mirrors `ipc_guard::forwarded_shortcut_key` in Rust; keep them in step.
// Deliberately absent: Ctrl+Shift+W (close pane) and Ctrl+Shift+H / V / T
// (split, new tab). Nothing that destroys or creates a PTY may be triggerable
// from a web page.

type Mods = `${boolean},${boolean}`; // shift,alt (ctrl is always required)

const TABLE: Record<Mods, Record<string, string>> = {
  // Ctrl+Alt+1..9 switch workspace, Ctrl+Alt+N notes.
  "false,true": {
    Digit1: "1",
    Digit2: "2",
    Digit3: "3",
    Digit4: "4",
    Digit5: "5",
    Digit6: "6",
    Digit7: "7",
    Digit8: "8",
    Digit9: "9",
    KeyN: "n",
  },
  "true,false": {
    KeyZ: "Z",
    KeyP: "P",
    KeyR: "R",
    KeyE: "E",
    BracketLeft: "{",
    BracketRight: "}",
    Tab: "Tab",
  },
  "false,false": { Tab: "Tab" },
  "true,true": {},
};

export function forwardedKeyInit(payload: unknown): KeyboardEventInit | null {
  if (typeof payload !== "object" || payload === null) return null;
  const { code, ctrl, shift, alt } = payload as Record<string, unknown>;
  if (
    typeof code !== "string" ||
    ctrl !== true ||
    typeof shift !== "boolean" ||
    typeof alt !== "boolean"
  ) {
    return null;
  }
  const row = TABLE[`${shift},${alt}` as Mods];
  const key = Object.prototype.hasOwnProperty.call(row, code) ? row[code] : undefined;
  if (key === undefined) return null;
  return {
    key,
    code,
    ctrlKey: true,
    shiftKey: shift,
    altKey: alt,
    bubbles: true,
    cancelable: true,
  };
}
