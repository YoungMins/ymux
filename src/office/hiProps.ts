// The office's furniture at one art pixel per CSS pixel. The pieces are
// drawn as small grids (`pixelArt.ts`, `themeArt.ts`); this redraws a grid at
// three times its resolution, the size it is shown at:
//
// 1. Scale3x: every pixel becomes 3x3, and stair-stepped diagonals are
//    rounded off instead of copied as blocks.
// 2. Outlines thinned: an outer edge three pixels thick is worn down to one,
//    and a dividing line inside a piece keeps a one-pixel core with the
//    darkest tone of the part beside it as a soft shadow.
// 3. Shading: each part is bevelled with a five-tone ramp of its colour,
//    lit from the top left like the characters.
//
// Pixels whose palette value is a CSS class (a desk's screen, a server's
// LEDs, a torch's flame) are kept out of the raster and returned as runs, so
// the page can draw them as SVG the stylesheet colours and animates.

import { Raster, ramp, tone, type Ramp } from "./paint";

export type Palette = Readonly<Record<string, string>>;

/// One run of a CSS-coloured pixel row: `cls` at (x, y), `w` pixels wide.
export interface ClassRun {
  cls: string;
  x: number;
  y: number;
  w: number;
}

export interface HiPiece {
  raster: Raster;
  classes: ClassRun[];
}

export const SCALE = 3;

/// Scale3x (AdvMAME3x) on a grid of keys; "." is clear.
export function scale3x(rows: readonly string[]): string[][] {
  const h = rows.length;
  const w = rows.reduce((m, r) => Math.max(m, r.length), 0);
  const at = (x: number, y: number) => (x < 0 || y < 0 || x >= w || y >= h ? "." : (rows[y][x] ?? "."));
  const out = Array.from({ length: h * 3 }, () => new Array<string>(w * 3).fill("."));
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const A = at(x - 1, y - 1), B = at(x, y - 1), C = at(x + 1, y - 1);
      const D = at(x - 1, y), E = at(x, y), F = at(x + 1, y);
      const G = at(x - 1, y + 1), H = at(x, y + 1), I = at(x + 1, y + 1);
      let e = [E, E, E, E, E, E, E, E, E];
      if (B !== H && D !== F) {
        e = [
          D === B ? D : E,
          (D === B && E !== C) || (B === F && E !== A) ? B : E,
          B === F ? F : E,
          (D === B && E !== G) || (D === H && E !== A) ? D : E,
          E,
          (B === F && E !== I) || (H === F && E !== C) ? F : E,
          D === H ? D : E,
          (D === H && E !== I) || (H === F && E !== G) ? H : E,
          H === F ? F : E,
        ];
      }
      for (let i = 0; i < 9; i++) out[y * 3 + Math.floor(i / 3)][x * 3 + (i % 3)] = e[i];
    }
  return out;
}

const DIRS = [
  [0, -1],
  [0, 1],
  [-1, 0],
  [1, 0],
] as const;

/// Redraw `rows` (coloured by `pal`, outline key "o") at three times the size.
export function hiPiece(rows: readonly string[], pal: Palette): HiPiece {
  const g = scale3x(rows);
  const h = g.length;
  const w = h ? g[0].length : 0;
  const key = (x: number, y: number) => (x < 0 || y < 0 || x >= w || y >= h ? "." : g[y][x]);
  const isClass = (k: string) => (pal[k] ?? "").startsWith(".");
  const isFill = (k: string) => k !== "." && k !== "o" && !!pal[k];

  // Wear the outer edge of the outline down to one pixel: two passes, each
  // clearing outline pixels that touch the outside and no fill.
  for (let pass = 0; pass < 2; pass++) {
    const clear: [number, number][] = [];
    for (let y = 0; y < h; y++)
      for (let x = 0; x < w; x++) {
        if (g[y][x] !== "o") continue;
        let outside = false;
        let fill = false;
        for (const [dx, dy] of DIRS) {
          const k = key(x + dx, y + dy);
          if (k === ".") outside = true;
          else if (k !== "o") fill = true;
        }
        if (outside && !fill) clear.push([x, y]);
      }
    for (const [x, y] of clear) g[y][x] = ".";
  }

  const ramps = new Map<string, Ramp>();
  const rampOf = (k: string) => {
    let r = ramps.get(k);
    if (!r) ramps.set(k, (r = ramp(pal[k])));
    return r;
  };

  const raster = new Raster(w, h);
  const classes: ClassRun[] = [];
  const outline = pal.o ?? "#2a1f1a";
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const k = g[y][x];
      if (k === ".") continue;
      if (isClass(k)) {
        const cls = pal[k].slice(1);
        const last = classes[classes.length - 1];
        if (last && last.cls === cls && last.y === y && last.x + last.w === x) last.w++;
        else classes.push({ cls, x, y, w: 1 });
        continue;
      }
      if (k === "o") {
        // A thick inner line: the pixels beside a part become its shadow,
        // leaving a one-pixel core.
        let shade: string | null = null;
        for (const [dx, dy] of DIRS) {
          const n = key(x + dx, y + dy);
          if (isFill(n) && !isClass(n) && key(x - dx, y - dy) === "o") {
            shade = rampOf(n).tones[0];
            break;
          }
        }
        raster.set(x, y, shade ?? outline);
        continue;
      }
      if (!pal[k]) continue;
      // Bevel: lit along the part's top and left edges, shaded along its
      // bottom and right.
      const same = (dx: number, dy: number) => key(x + dx, y + dy) === k;
      let light = 0.55;
      if (!same(0, -1)) light += 0.3;
      else if (!same(0, -2)) light += 0.12;
      if (!same(-1, 0)) light += 0.12;
      if (!same(0, 1)) light -= 0.32;
      else if (!same(0, 2)) light -= 0.1;
      if (!same(1, 0)) light -= 0.14;
      raster.set(x, y, tone(rampOf(k), Math.max(0, Math.min(1, light)), x, y, 0));
    }
  }
  return { raster, classes };
}
