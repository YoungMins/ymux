import { describe, expect, it } from "vitest";
import { PEEK_KEYS, keyBytes } from "./peekKeys";
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
