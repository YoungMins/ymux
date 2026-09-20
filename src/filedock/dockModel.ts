// Pure pieces of the file dock: the persisted open/width state
// (localStorage `ymux.fileDock`), the width clamp used while dragging, and
// the ydir command line.

export const DOCK_MIN_WIDTH = 200;
export const DOCK_DEFAULT_WIDTH = 320;

export interface DockState {
  open: boolean;
  width: number;
}

export function parseDockState(raw: string | null): DockState {
  const fallback: DockState = { open: false, width: DOCK_DEFAULT_WIDTH };
  if (!raw) return fallback;
  try {
    const v = JSON.parse(raw) as Partial<Record<keyof DockState, unknown>>;
    const width =
      typeof v.width === "number" && Number.isFinite(v.width) && v.width >= DOCK_MIN_WIDTH
        ? Math.round(v.width)
        : DOCK_DEFAULT_WIDTH;
    return { open: v.open === true, width };
  } catch {
    return fallback;
  }
}

export function serializeDockState(s: DockState): string {
  return JSON.stringify(s);
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
