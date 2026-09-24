import { describe, it, expect } from "vitest";
import { isGlobalChord, paneKeyAction, type KeyLike } from "./keys";

function k(key: string, mods: Partial<KeyLike> = {}): KeyLike {
  const code =
    mods.code ??
    (key.length === 1 && /[a-z]/i.test(key) ? `Key${key.toUpperCase()}` : key);
  return {
    key,
    code,
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    isComposing: false,
    ...mods,
  };
}

const win = false;
const mac = true;

describe("isGlobalChord — ymux globals always win (spec 0.6)", () => {
  it("reserves every Ctrl+Shift and Ctrl+Alt chord on Windows", () => {
    for (const key of ["W", "T", "H", "V", "E", "P", "R", "Z", "N"]) {
      expect(isGlobalChord(k(key, { ctrlKey: true, shiftKey: true }), win)).toBe(true);
    }
    expect(isGlobalChord(k("n", { ctrlKey: true, altKey: true, code: "KeyN" }), win)).toBe(true);
    expect(
      isGlobalChord(k("[", { ctrlKey: true, shiftKey: true, code: "BracketLeft" }), win),
    ).toBe(true);
  });

  it("uses Cmd, not Ctrl, as the modifier on macOS", () => {
    expect(isGlobalChord(k("W", { metaKey: true, shiftKey: true }), mac)).toBe(true);
    expect(isGlobalChord(k("W", { ctrlKey: true, shiftKey: true }), mac)).toBe(false);
  });

  it("reserves Ctrl+Tab (Ctrl on both platforms), search and font size", () => {
    expect(isGlobalChord(k("Tab", { ctrlKey: true }), mac)).toBe(true);
    expect(isGlobalChord(k("Tab", { ctrlKey: true, shiftKey: true }), win)).toBe(true);
    expect(isGlobalChord(k("f", { ctrlKey: true }), win)).toBe(true);
    expect(isGlobalChord(k("=", { ctrlKey: true, code: "Equal" }), win)).toBe(true);
    expect(isGlobalChord(k("-", { ctrlKey: true, code: "Minus" }), win)).toBe(true);
    expect(isGlobalChord(k("0", { ctrlKey: true, code: "Digit0" }), win)).toBe(true);
  });

  it("reserves workspace switching", () => {
    expect(isGlobalChord(k("1", { ctrlKey: true, altKey: true, code: "Digit1" }), win)).toBe(true);
    expect(isGlobalChord(k("3", { metaKey: true, code: "Digit3" }), mac)).toBe(true);
  });

  it("leaves the pane's own chords alone", () => {
    for (const key of ["a", "c", "x", "v", "r", "l"]) {
      expect(isGlobalChord(k(key, { ctrlKey: true }), win)).toBe(false);
    }
    expect(isGlobalChord(k("ArrowDown"), win)).toBe(false);
    expect(isGlobalChord(k("F2"), win)).toBe(false);
  });
});

describe("paneKeyAction", () => {
  it("maps navigation keys, with Shift extending", () => {
    expect(paneKeyAction(k("ArrowDown"), win)).toEqual({ kind: "move", delta: 1, extend: false });
    expect(paneKeyAction(k("ArrowUp", { shiftKey: true }), win)).toEqual({
      kind: "move",
      delta: -1,
      extend: true,
    });
    expect(paneKeyAction(k("Home"), win)).toEqual({ kind: "edge", end: false, extend: false });
    expect(paneKeyAction(k("End", { shiftKey: true }), win)).toEqual({
      kind: "edge",
      end: true,
      extend: true,
    });
    expect(paneKeyAction(k("PageDown"), win)).toEqual({ kind: "page", dir: 1, extend: false });
  });

  it("maps open, parent, rename, delete", () => {
    expect(paneKeyAction(k("Enter"), win)).toEqual({ kind: "open" });
    expect(paneKeyAction(k("Backspace"), win)).toEqual({ kind: "parent" });
    expect(paneKeyAction(k("ArrowUp", { altKey: true }), win)).toEqual({ kind: "parent" });
    expect(paneKeyAction(k("F2"), win)).toEqual({ kind: "rename" });
    expect(paneKeyAction(k("Delete"), win)).toEqual({ kind: "delete", permanent: false });
    expect(paneKeyAction(k("Delete", { shiftKey: true }), win)).toEqual({
      kind: "delete",
      permanent: true,
    });
  });

  it("gives macOS a keyboard delete: Cmd+Backspace trashes", () => {
    expect(paneKeyAction(k("Backspace", { metaKey: true }), mac)).toEqual({
      kind: "delete",
      permanent: false,
    });
  });

  it("maps the clipboard, select-all, refresh and path editing on the platform modifier", () => {
    expect(paneKeyAction(k("a", { ctrlKey: true }), win)).toEqual({ kind: "selectAll" });
    expect(paneKeyAction(k("c", { metaKey: true }), mac)).toEqual({ kind: "copy" });
    expect(paneKeyAction(k("x", { ctrlKey: true }), win)).toEqual({ kind: "cut" });
    expect(paneKeyAction(k("v", { ctrlKey: true }), win)).toEqual({ kind: "paste" });
    expect(paneKeyAction(k("r", { ctrlKey: true }), win)).toEqual({ kind: "refresh" });
    expect(paneKeyAction(k("F5"), win)).toEqual({ kind: "refresh" });
    expect(paneKeyAction(k("l", { ctrlKey: true }), win)).toEqual({ kind: "editPath" });
    expect(paneKeyAction(k("F7"), win)).toEqual({ kind: "newFolder" });
    // Ctrl+C on macOS is not the copy chord.
    expect(paneKeyAction(k("c", { ctrlKey: true }), mac)).toBeNull();
  });

  it("uses the physical key for letter chords, so a Korean layout still copies", () => {
    expect(paneKeyAction(k("ㅊ", { ctrlKey: true, code: "KeyC" }), win)).toEqual({ kind: "copy" });
  });

  it("toggles the preview with Tab and clears with Escape", () => {
    expect(paneKeyAction(k("Tab"), win)).toEqual({ kind: "togglePreview" });
    expect(paneKeyAction(k("Escape"), win)).toEqual({ kind: "escape" });
  });

  it("types ahead on printable keys, but never mid-composition", () => {
    expect(paneKeyAction(k("s"), win)).toEqual({ kind: "typeAhead", text: "s" });
    expect(paneKeyAction(k("ㅎ", { isComposing: true }), win)).toBeNull();
    expect(paneKeyAction(k("Process"), win)).toBeNull();
    expect(paneKeyAction(k(" "), win)).toBeNull();
  });

  it("returns null for ymux globals so they reach the window handler", () => {
    expect(paneKeyAction(k("W", { ctrlKey: true, shiftKey: true }), win)).toBeNull();
    expect(paneKeyAction(k("Tab", { ctrlKey: true }), win)).toBeNull();
    expect(paneKeyAction(k("f", { ctrlKey: true }), win)).toBeNull();
  });
});
