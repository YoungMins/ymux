import { describe, expect, it } from "vitest";
import { PEEK_KEYS, directKey, isInputOption, keyBytes, menuOptions } from "./peekKeys";
import { peekBox } from "./OfficePeek";

describe("keyBytes", () => {
  it("sends arrows in the app's cursor-key mode", () => {
    expect(keyBytes("up", false)).toBe("\x1b[A");
    expect(keyBytes("up", true)).toBe("\x1bOA");
    expect(keyBytes("down", false)).toBe("\x1b[B");
    expect(keyBytes("down", true)).toBe("\x1bOB");
  });

  it("sends Enter as CR and digits as themselves", () => {
    expect(keyBytes("enter", true)).toBe("\r");
    expect(keyBytes("esc", false)).toBe("\x1b");
    expect(keyBytes("tab", false)).toBe("\t");
    expect(keyBytes("2", false)).toBe("2");
  });

  it("has one button per key", () => {
    expect(new Set(PEEK_KEYS.map((k) => k.key)).size).toBe(PEEK_KEYS.length);
  });
});

describe("peekBox", () => {
  it("centres on a big room and caps the size", () => {
    const b = peekBox({ x: 100, y: 100, w: 1000, h: 800 }, 1400, 900);
    expect(b.w).toBe(760);
    expect(b.h).toBe(560);
    expect(b.x + b.w / 2).toBe(600);
    expect(b.y + b.h / 2).toBe(500);
  });

  it("grows past a small room and stays on the floor", () => {
    const b = peekBox({ x: 1250, y: 780, w: 140, h: 110 }, 1400, 900);
    expect(b.w).toBe(380);
    expect(b.h).toBe(280);
    expect(b.x + b.w).toBeLessThanOrEqual(1392);
    expect(b.y + b.h).toBeLessThanOrEqual(892);
  });

  it("shrinks to a floor smaller than the minimum", () => {
    const b = peekBox({ x: 0, y: 0, w: 200, h: 150 }, 300, 200);
    expect(b.w).toBe(284);
    expect(b.h).toBe(184);
    expect(b.x).toBe(8);
    expect(b.y).toBe(8);
  });
});

describe("menuOptions", () => {
  it("finds a live menu by its pointer", () => {
    const screen = [
      "Do you want to make this edit to api.ts?",
      "❯ 1. Yes",
      "  2. Yes, and don't ask again this session",
      "  3. No, and tell Claude what to do differently (esc)",
    ];
    expect(menuOptions(screen)).toEqual([
      { n: "1", label: "Yes", selected: true, input: false },
      { n: "2", label: "Yes, and don't ask again this session", selected: false, input: false },
      { n: "3", label: "No, and tell Claude what to do differently (esc)", selected: false, input: true },
    ]);
  });

  it("follows the pointer and allows a description under an option", () => {
    const screen = ["  1. Plan", "     think first", "› 2. Code", "  3. Ask"];
    expect(menuOptions(screen).map((o) => [o.n, o.selected])).toEqual([
      ["1", false],
      ["2", true],
      ["3", false],
    ]);
  });

  it("ignores a numbered list in an answer, and an old menu above a new one", () => {
    expect(menuOptions(["Steps:", "1. Read the file", "2. Fix the bug"])).toEqual([]);
    const two = ["❯ 1. Old", "  2. Older", "done", "  1. New", "❯ 2. Newer"];
    expect(menuOptions(two).map((o) => o.label)).toEqual(["New", "Newer"]);
  });
});

describe("directKey", () => {
  const k = (key: string, mods: Partial<{ ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean; isComposing: boolean }> = {}) =>
    directKey({ key, ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, ...mods }, false);

  it("sends what a terminal would", () => {
    expect(k("a")).toBe("a");
    expect(k("한")).toBe("한");
    expect(k("Enter")).toBe("\r");
    expect(k("Backspace")).toBe("\x7f");
    expect(k("ArrowUp")).toBe("\x1b[A");
    expect(k("Tab", { shiftKey: true })).toBe("\x1b[Z");
    expect(k("Escape")).toBe("\x1b");
    expect(k("c", { ctrlKey: true })).toBe("\x03");
    expect(k("x", { altKey: true })).toBe("\x1bx");
  });

  it("leaves modifiers, function keys and IME composition alone", () => {
    expect(k("Shift")).toBeNull();
    expect(k("F5")).toBeNull();
    expect(k("Process")).toBeNull();
    expect(k("a", { isComposing: true })).toBeNull();
    expect(k("v", { metaKey: true })).toBeNull();
  });
});

describe("isInputOption", () => {
  it("knows the choices that open a text field", () => {
    for (const l of ["No, and tell Codex what to do differently (esc)", "Type something.", "Other", "직접 입력", "その他"]) {
      expect(isInputOption(l), l).toBe(true);
    }
    for (const l of ["Yes", "Yes, and don't ask again this session", "Plan mode", "Otherwise skip"]) {
      expect(isInputOption(l), l).toBe(false);
    }
  });
});
