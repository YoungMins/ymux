import { describe, expect, it } from "vitest";
import { gitKeyAction } from "./keys";
import type { KeyLike } from "../files/keys";

function key(k: string, code: string, mods: Partial<KeyLike> = {}): KeyLike {
  return {
    key: k,
    code,
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    isComposing: false,
    ...mods,
  };
}

describe("gitKeyAction", () => {
  it("moves with arrows and with ygit's j / k", () => {
    expect(gitKeyAction(key("ArrowDown", "ArrowDown"), false)).toEqual({ kind: "move", delta: 1 });
    expect(gitKeyAction(key("k", "KeyK"), false)).toEqual({ kind: "move", delta: -1 });
    // Matched by code: a Korean layout's ㅓ on the J key still moves.
    expect(gitKeyAction(key("ㅓ", "KeyJ"), false)).toEqual({ kind: "move", delta: 1 });
    expect(gitKeyAction(key("PageUp", "PageUp"), false)).toEqual({ kind: "page", dir: -1 });
    expect(gitKeyAction(key("End", "End"), false)).toEqual({ kind: "edge", end: true });
  });

  it("switches sections with Tab and Shift+Tab, as ygit's Tab did", () => {
    expect(gitKeyAction(key("Tab", "Tab"), false)).toEqual({ kind: "section", dir: 1 });
    expect(gitKeyAction(key("Tab", "Tab", { shiftKey: true }), false)).toEqual({
      kind: "section",
      dir: -1,
    });
  });

  it("maps Enter, refresh, worktree and pin keys", () => {
    expect(gitKeyAction(key("Enter", "Enter"), false)).toEqual({ kind: "activate" });
    expect(gitKeyAction(key("r", "KeyR"), false)).toEqual({ kind: "refresh" });
    expect(gitKeyAction(key("F5", "F5"), false)).toEqual({ kind: "refresh" });
    expect(gitKeyAction(key("r", "KeyR", { ctrlKey: true }), false)).toEqual({ kind: "refresh" });
    expect(gitKeyAction(key("n", "KeyN"), false)).toEqual({ kind: "newWorktree" });
    expect(gitKeyAction(key("Delete", "Delete"), false)).toEqual({ kind: "remove" });
    expect(gitKeyAction(key("Backspace", "Backspace", { metaKey: true }), true)).toEqual({
      kind: "remove",
    });
    expect(gitKeyAction(key("t", "KeyT"), false)).toEqual({ kind: "terminal" });
    expect(gitKeyAction(key("p", "KeyP"), false)).toEqual({ kind: "pin" });
    expect(gitKeyAction(key("c", "KeyC", { ctrlKey: true }), false)).toEqual({ kind: "copy" });
    expect(gitKeyAction(key("c", "KeyC", { metaKey: true }), true)).toEqual({ kind: "copy" });
  });

  it("lets ymux's global chords through untouched (spec §0.6)", () => {
    for (const ev of [
      key("W", "KeyW", { ctrlKey: true, shiftKey: true }),
      key("T", "KeyT", { ctrlKey: true, shiftKey: true }),
      key("Tab", "Tab", { ctrlKey: true }),
      key("f", "KeyF", { ctrlKey: true }),
      key("=", "Equal", { ctrlKey: true }),
      key("1", "Digit1", { ctrlKey: true, altKey: true }),
    ]) {
      expect(gitKeyAction(ev, false)).toBeNull();
    }
    expect(gitKeyAction(key("1", "Digit1", { metaKey: true }), true)).toBeNull();
  });

  it("ignores composition, bare Ctrl on macOS and unbound keys", () => {
    expect(gitKeyAction(key("Process", "KeyJ"), false)).toBeNull();
    expect(gitKeyAction(key("j", "KeyJ", { isComposing: true }), false)).toBeNull();
    expect(gitKeyAction(key("c", "KeyC", { ctrlKey: true }), true)).toBeNull();
    expect(gitKeyAction(key("x", "KeyX"), false)).toBeNull();
    expect(gitKeyAction(key("j", "KeyJ", { altKey: true }), false)).toBeNull();
  });
});
