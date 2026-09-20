// Pure pieces of the file dock: the persisted open/width state
// (localStorage `ymux.fileDock`), the width clamp used while dragging, and
// the ydir command line.

/// Widths are in px, and what they buy is columns of yDir. At the default
/// 13 px terminal font a monospace cell is ~7.6 px wide, and the dock
/// spends 2 cells on yDir's panel border plus ~6 px on its resizer.
///
/// yDir's dock layout costs 4 cells for the `[D] ` prefix, 11 for Size and
/// 17 for Modified, dropping the last two in that order as the width falls
/// (`Columns::adaptive`). So:
///
/// - 440 px ≈ 58 columns ≈ 56 inner: name, Size and Modified, with 24
///   cells of filename. A real file manager.
/// - 260 px ≈ 34 columns ≈ 32 inner: name and Size, 17 cells of filename.
///   Cramped but legible, which is what a minimum should be.
///
/// The old 320/200 gave 0 and 0 cells of filename under the fixed column
/// layout — the dock was reported as "too narrow to be useful" for exactly
/// this reason.
export const DOCK_MIN_WIDTH = 260;
export const DOCK_DEFAULT_WIDTH = 440;

/// Bumped when the stored width stops meaning what it used to. A width
/// written by an older ymux was clamped against a 200 px minimum and is
/// almost certainly the old, unusably narrow default, so it is replaced
/// once — deliberately overriding one earlier drag rather than leaving
/// every existing user on the width they complained about. A drag after
/// the upgrade is stored at the current version and kept from then on.
export const DOCK_STATE_VERSION = 2;

export interface DockState {
  open: boolean;
  width: number;
}

export function parseDockState(raw: string | null): DockState {
  const fallback: DockState = { open: false, width: DOCK_DEFAULT_WIDTH };
  if (!raw) return fallback;
  try {
    const v = JSON.parse(raw) as Record<string, unknown>;
    const current = v.v === DOCK_STATE_VERSION;
    const width =
      current &&
      typeof v.width === "number" &&
      Number.isFinite(v.width) &&
      v.width >= DOCK_MIN_WIDTH
        ? Math.round(v.width)
        : DOCK_DEFAULT_WIDTH;
    return { open: v.open === true, width };
  } catch {
    return fallback;
  }
}

export function serializeDockState(s: DockState): string {
  return JSON.stringify({ ...s, v: DOCK_STATE_VERSION });
}

/// Width in px, kept between DOCK_MIN_WIDTH and half of `containerWidth`.
/// The minimum wins in a window too narrow to satisfy both.
export function clampDockWidth(width: number, containerWidth: number): number {
  if (!Number.isFinite(width)) return DOCK_MIN_WIDTH;
  const max = Math.max(DOCK_MIN_WIDTH, Math.floor(containerWidth / 2));
  return Math.min(max, Math.max(DOCK_MIN_WIDTH, Math.round(width)));
}

/// The dock's process. `--dock` makes ydir follow ChangeDir over yipc. The
/// dir is one argv element, so spaces need no quoting.
export function dockArgv(cwd: string | null): string[] {
  return cwd ? ["ydir", "--dock", cwd] : ["ydir", "--dock"];
}
