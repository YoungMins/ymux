import { describe, it, expect } from "vitest";
import {
  DOCK_DEFAULT_WIDTH,
  DOCK_MIN_WIDTH,
  clampDockWidth,
  dockArgv,
  parseDockState,
  serializeDockState,
} from "./dockModel";

const DEFAULT = { open: false, width: DOCK_DEFAULT_WIDTH };

describe("parseDockState", () => {
  it("defaults to closed at the default width", () => {
    expect(parseDockState(null)).toEqual(DEFAULT);
  });

  it("round-trips what serializeDockState wrote", () => {
    const s = { open: true, width: 412 };
    expect(parseDockState(serializeDockState(s))).toEqual(s);
  });

  it("falls back on garbage instead of throwing", () => {
    expect(parseDockState("{nope")).toEqual(DEFAULT);
    expect(parseDockState('{"open":"yes","width":"wide"}')).toEqual(DEFAULT);
  });

  it("replaces a width below the minimum with the default", () => {
    expect(parseDockState('{"open":true,"width":50}')).toEqual({
      open: true,
      width: DOCK_DEFAULT_WIDTH,
    });
  });
});

describe("clampDockWidth", () => {
  it("keeps widths between the minimum and half the container", () => {
    expect(clampDockWidth(300, 1000)).toBe(300);
    expect(clampDockWidth(100, 1000)).toBe(DOCK_MIN_WIDTH);
    expect(clampDockWidth(900, 1000)).toBe(500);
  });

  it("never goes below the minimum in a narrow window", () => {
    expect(clampDockWidth(300, 300)).toBe(DOCK_MIN_WIDTH);
  });

  it("treats a non-finite width as the minimum", () => {
    expect(clampDockWidth(Number.NaN, 1000)).toBe(DOCK_MIN_WIDTH);
  });
});

describe("dockArgv", () => {
  it("passes the start dir after --dock, unsplit", () => {
    expect(dockArgv("C:\\work dir")).toEqual(["ydir", "--dock", "C:\\work dir"]);
  });

  it("omits the dir when it is unknown", () => {
    expect(dockArgv(null)).toEqual(["ydir", "--dock"]);
  });
});
