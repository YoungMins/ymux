// Where the mini office sits: its gap from the right and bottom edges and its
// scale, moved by dragging its head and resized from its top-left corner.
// Kept per browser in localStorage, so it comes back where it was left.

/// The full-size office the mini one scales down.
export const MINI_W = 960;
export const MINI_H = 600;
export const MIN_SCALE = 0.35;
export const MAX_SCALE = 0.75;

export interface MiniPlace {
  right: number;
  bottom: number;
  scale: number;
}

export const DEFAULT_MINI: MiniPlace = { right: 12, bottom: 12, scale: 0.5 };

const KEY = "ymux.office.mini";

/// `p` kept on screen in a `w` × `h` window, its scale in range.
export function clampMini(p: MiniPlace, w: number, h: number): MiniPlace {
  const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, Number.isFinite(p.scale) ? p.scale : DEFAULT_MINI.scale));
  const fit = (v: number, room: number): number => Math.round(Math.max(0, Math.min(Math.max(0, room), Number.isFinite(v) ? v : 12)));
  return { scale, right: fit(p.right, w - MINI_W * scale), bottom: fit(p.bottom, h - MINI_H * scale) };
}

/// Dragged by (dx, dy) from `start`.
export function movedMini(start: MiniPlace, dx: number, dy: number, w: number, h: number): MiniPlace {
  return clampMini({ ...start, right: start.right - dx, bottom: start.bottom - dy }, w, h);
}

/// The top-left corner dragged by (dx, dy) from `start`: the bottom-right
/// corner stays put, the larger of the two pulls decides the size.
export function resizedMini(start: MiniPlace, dx: number, dy: number, w: number, h: number): MiniPlace {
  const byW = (MINI_W * start.scale - dx) / MINI_W;
  const byH = (MINI_H * start.scale - dy) / MINI_H;
  return clampMini({ ...start, scale: Math.max(byW, byH) }, w, h);
}

export function loadMini(): MiniPlace {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<MiniPlace> | null;
    if (v && typeof v === "object") return { right: Number(v.right), bottom: Number(v.bottom), scale: Number(v.scale) };
  } catch {
    /* fall through */
  }
  return { ...DEFAULT_MINI };
}

export function saveMini(p: MiniPlace): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    /* not remembered */
  }
}
