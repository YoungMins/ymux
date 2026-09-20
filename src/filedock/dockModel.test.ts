import { describe, it, expect } from "vitest";
import {
  DOCK_DEFAULT_WIDTH,
  DOCK_STATE_VERSION,
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

  it("replaces a width stored before the version bump, keeping the open state", () => {
    // What a pre-0.10.1 ymux wrote: the old 320 px default, no version.
    expect(parseDockState('{"open":true,"width":320}')).toEqual({
      open: true,
      width: DOCK_DEFAULT_WIDTH,
    });
    // An explicit older version is just as stale.
    expect(parseDockState('{"open":false,"width":320,"v":1}')).toEqual({
      open: false,
      width: DOCK_DEFAULT_WIDTH,
    });
  });

  it("keeps a width dragged since the bump", () => {
    const dragged = serializeDockState({ open: true, width: 700 });
    expect(parseDockState(dragged)).toEqual({ open: true, width: 700 });
  });

  it("stamps the version so the next bump can tell old state apart", () => {
    expect(JSON.parse(serializeDockState({ open: true, width: 700 })).v).toBe(
      DOCK_STATE_VERSION,
    );
  });
});

describe("dock widths", () => {
  /// What these px buy in yDir columns is the whole point of the numbers:
  /// see the note in dockModel.ts. A minimum that cannot hold a filename
  /// is the bug being fixed, so it must not drift back down.
  const CELL_PX = 7.6;
  const columns = (px: number): number => Math.floor(px / CELL_PX) - 2;

  it("gives the default width room for name, size and date", () => {
    expect(columns(DOCK_DEFAULT_WIDTH)).toBeGreaterThanOrEqual(40);
  });

  it("gives the minimum width room for a readable name column", () => {
    expect(columns(DOCK_MIN_WIDTH)).toBeGreaterThanOrEqual(27);
  });

  it("keeps the default above the minimum", () => {
    expect(DOCK_DEFAULT_WIDTH).toBeGreaterThan(DOCK_MIN_WIDTH);
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
