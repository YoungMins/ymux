import { describe, it, expect } from "vitest";
import {
  DOCK_DEFAULT_WIDTH,
  DOCK_STATE_VERSION,
  DOCK_MIN_WIDTH,
  clampDockWidth,
  migrateDockState,
  nextDockState,
  parseDockState,
  serializeDockState,
  toggleDockOpen,
} from "./dockModel";

const DEFAULT = { open: false, width: DOCK_DEFAULT_WIDTH, view: "files" };

describe("parseDockState", () => {
  it("defaults to closed at the default width", () => {
    expect(parseDockState(null)).toEqual(DEFAULT);
  });

  it("round-trips what serializeDockState wrote", () => {
    const s = { open: true, width: 412, view: "usage" as const };
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
      view: "files",
    });
  });

  it("replaces a width stored before the version bump, keeping the open state", () => {
    // What a pre-0.10.1 ymux wrote: the old 320 px default, no version.
    expect(parseDockState('{"open":true,"width":320}')).toEqual({
      open: true,
      width: DOCK_DEFAULT_WIDTH,
      view: "files",
    });
    // An explicit older version is just as stale.
    expect(parseDockState('{"open":false,"width":320,"v":1}')).toEqual({
      open: false,
      width: DOCK_DEFAULT_WIDTH,
      view: "files",
    });
  });

  it("keeps a width dragged since the bump", () => {
    const dragged = serializeDockState({ open: true, width: 700, view: "files" });
    expect(parseDockState(dragged)).toEqual({ open: true, width: 700, view: "files" });
  });

  it("stamps the version so the next bump can tell old state apart", () => {
    expect(JSON.parse(serializeDockState({ open: true, width: 700, view: "files" })).v).toBe(
      DOCK_STATE_VERSION,
    );
  });
});

describe("toggleDockOpen", () => {
  it("opens on the persisted view and closes again", () => {
    const closed = { open: false, width: 400, view: "usage" as const };
    const open = toggleDockOpen(closed);
    expect(open).toEqual({ open: true, width: 400, view: "usage" });
    expect(toggleDockOpen(open)).toEqual(closed);
  });
});

describe("nextDockState", () => {
  const closed = { open: false, width: 400, view: "files" as const };

  it("opens a closed panel on the requested view", () => {
    expect(nextDockState(closed, "usage", true)).toEqual({ open: true, width: 400, view: "usage" });
  });

  it("closes when the already-showing view is requested with toggle", () => {
    const open = { ...closed, open: true };
    expect(nextDockState(open, "files", true)).toEqual({ ...open, open: false });
  });

  it("switches views when another view is requested on an open panel", () => {
    const open = { ...closed, open: true };
    expect(nextDockState(open, "usage", true)).toEqual({ ...open, view: "usage" });
  });

  it("never closes without toggle", () => {
    const open = { ...closed, open: true, view: "usage" as const };
    expect(nextDockState(open, "usage", false)).toEqual(open);
    expect(nextDockState(closed, "files", false)).toEqual({ ...closed, open: true });
  });
});

describe("migrateDockState", () => {
  const dock = (open: boolean, width: number): string =>
    serializeDockState({ open, width, view: "files" });

  it("takes the file dock's state when it exists", () => {
    expect(migrateDockState(dock(true, 500), null)).toEqual({ open: true, width: 500, view: "files" });
    expect(migrateDockState(dock(false, 500), null)).toEqual({ open: false, width: 500, view: "files" });
  });

  it("starts on usage when only the token dock was open", () => {
    expect(migrateDockState(dock(false, 500), dock(true, 380))).toEqual({
      open: true,
      width: 500,
      view: "usage",
    });
    expect(migrateDockState(null, dock(true, 380))).toEqual({ open: true, width: 380, view: "usage" });
  });

  it("prefers files when both were open, and defaults when neither exists", () => {
    expect(migrateDockState(dock(true, 500), dock(true, 380)).view).toBe("files");
    expect(migrateDockState(null, null)).toEqual(DEFAULT);
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
