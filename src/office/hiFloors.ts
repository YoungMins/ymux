// The office's floors and walls at one art pixel per CSS pixel: small tiles
// (planks, bricks, flagstones, carpet, metal plates, grass, sand, snow, …)
// painted pixel by pixel and repeated across a surface by a canvas pattern
// (`tiles.ts`). Each theme names a tile for every surface: the corridors,
// the lobby, a room's floor (by pane kind in the office) and a room's wall.
// Pure: tiles are rasters, seeded so they look the same every time.

import type { PaneKind } from "../types";
import { Raster, adjust } from "./paint";
import type { ThemeId } from "./themes";

export type Surface = "corridor" | "lobby" | "room" | "wall";

/// The wall's face between its top and base bands, in pixels.
export const WALL_FACE = 36;

/// A small seeded random source (mulberry32).
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function fill(w: number, h: number, c: string): Raster {
  const r = new Raster(w, h);
  r.px.fill(c);
  return r;
}

/// Set (x, y), wrapping round the tile so patterns repeat without seams.
function put(r: Raster, x: number, y: number, c: string): void {
  r.set(((Math.round(x) % r.w) + r.w) % r.w, ((Math.round(y) % r.h) + r.h) % r.h, c);
}

/// Sprinkle `n` single pixels of the colours `cs`.
function speckle(r: Raster, cs: string[], n: number, rand: () => number): void {
  for (let i = 0; i < n; i++) put(r, rand() * r.w, rand() * r.h, cs[Math.floor(rand() * cs.length)]);
}

/// Horizontal planks `ph` tall, their end joints staggered, with grain.
function planks(w: number, h: number, base: string, ph: number, seed: number): Raster {
  const rand = rng(seed);
  const r = fill(w, h, base);
  for (let row = 0; row * ph < h; row++) {
    const tint = adjust(base, (rand() - 0.5) * 0.06, 0, (rand() - 0.5) * 0.01);
    const y0 = row * ph;
    for (let y = y0; y < y0 + ph; y++) for (let x = 0; x < w; x++) put(r, x, y, tint);
    for (let x = 0; x < w; x++) {
      put(r, x, y0, adjust(tint, 0.07));
      put(r, x, y0 + ph - 1, adjust(tint, -0.16));
    }
    // Grain: short darker streaks, and a knot now and then.
    for (let i = 0; i < w / 6; i++) {
      const gx = rand() * w;
      const gy = y0 + 2 + Math.floor(rand() * (ph - 4));
      const len = 4 + rand() * 10;
      for (let k = 0; k < len; k++) put(r, gx + k, gy, adjust(tint, -0.06));
    }
    if (rand() < 0.5) {
      const kx = rand() * w;
      const ky = y0 + ph / 2;
      put(r, kx, ky, adjust(tint, -0.14));
      put(r, kx + 1, ky, adjust(tint, -0.1));
    }
    // A joint at a staggered place along the row.
    const jx = (row * 23 + Math.floor(rand() * 8)) % w;
    for (let y = y0 + 1; y < y0 + ph - 1; y++) {
      put(r, jx, y, adjust(tint, -0.18));
      put(r, jx + 1, y, adjust(tint, 0.05));
    }
  }
  return r;
}

/// Bricks or flagstones `bw` x `bh`, rows offset by half, in mortar.
function bricks(w: number, h: number, base: string, mortar: string, bw: number, bh: number, seed: number, rough = 0.06): Raster {
  const rand = rng(seed);
  const r = fill(w, h, mortar);
  for (let row = 0; row * bh < h; row++) {
    const off = row % 2 ? bw / 2 : 0;
    for (let col = -1; col * bw < w; col++) {
      const x0 = Math.round(col * bw + off);
      const y0 = row * bh;
      const tint = adjust(base, (rand() - 0.5) * rough * 2, 0, (rand() - 0.5) * 0.01);
      for (let y = y0 + 1; y < y0 + bh; y++)
        for (let x = x0 + 1; x < x0 + bw; x++) {
          const top = y === y0 + 1;
          const left = x === x0 + 1;
          const bottom = y === y0 + bh - 1;
          const right = x === x0 + bw - 1;
          const c = top ? adjust(tint, 0.1) : left ? adjust(tint, 0.05) : bottom ? adjust(tint, -0.12) : right ? adjust(tint, -0.07) : tint;
          put(r, x, y, c);
        }
      for (let i = 0; i < (bw * bh) / 30; i++) put(r, x0 + 2 + rand() * (bw - 4), y0 + 2 + rand() * (bh - 4), adjust(tint, rand() < 0.5 ? -0.07 : 0.05));
    }
  }
  return r;
}

/// Square tiles `size` wide with grout; `alt` makes a checkerboard.
function tiles(w: number, h: number, base: string, grout: string, size: number, alt?: string): Raster {
  const r = fill(w, h, grout);
  for (let ty = 0; ty * size < h; ty++)
    for (let tx = 0; tx * size < w; tx++) {
      const c = alt && (tx + ty) % 2 ? alt : base;
      const x0 = tx * size;
      const y0 = ty * size;
      for (let y = y0 + 1; y < y0 + size; y++)
        for (let x = x0 + 1; x < x0 + size; x++) {
          let v = c;
          if (y === y0 + 1 || x === x0 + 1) v = adjust(c, 0.06);
          else if (y === y0 + size - 1 || x === x0 + size - 1) v = adjust(c, -0.07);
          // A soft sheen across the tile.
          else if (x - x0 + (y - y0) === Math.floor(size * 0.55) || x - x0 + (y - y0) === Math.floor(size * 0.55) + 1) v = adjust(c, 0.035);
          put(r, x, y, v);
        }
    }
  return r;
}

/// Carpet: two close tones in a fine weave, with a small repeating motif.
function carpet(w: number, h: number, base: string, seed: number): Raster {
  const rand = rng(seed);
  const r = fill(w, h, base);
  const lo = adjust(base, -0.035);
  const hi = adjust(base, 0.03);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) if ((x + y * 2) % 4 === 0) put(r, x, y, lo);
  for (let y = 0; y < h; y += 12)
    for (let x = (y / 12) % 2 ? 6 : 0; x < w; x += 12) {
      for (const [dx, dy] of [[0, -1], [-1, 0], [1, 0], [0, 1]] as const) put(r, x + dx, y + dy, hi);
      put(r, x, y, adjust(base, 0.06));
    }
  speckle(r, [lo, hi], (w * h) / 40, rand);
  return r;
}

/// Metal plates `size` wide, bevelled, with a rivet in each corner.
function plates(w: number, h: number, base: string, size: number, seed: number): Raster {
  const rand = rng(seed);
  const r = tiles(w, h, base, adjust(base, -0.22), size);
  for (let ty = 0; ty * size < h; ty++)
    for (let tx = 0; tx * size < w; tx++)
      for (const [dx, dy] of [[3, 3], [size - 3, 3], [3, size - 3], [size - 3, size - 3]] as const) {
        put(r, tx * size + dx, ty * size + dy, adjust(base, 0.18));
        put(r, tx * size + dx + 1, ty * size + dy + 1, adjust(base, -0.18));
      }
  speckle(r, [adjust(base, -0.05), adjust(base, 0.04)], (w * h) / 50, rand);
  return r;
}

/// Floor grating: dark slots in a lighter frame.
function grating(w: number, h: number, base: string): Raster {
  const r = fill(w, h, base);
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++) {
      const gx = x % 6;
      const gy = y % 6;
      if (gx >= 1 && gx <= 4 && gy >= 2 && gy <= 3) put(r, x, y, adjust(base, -0.2));
      else if (gx >= 1 && gx <= 4 && gy === 1) put(r, x, y, adjust(base, 0.08));
    }
  return r;
}

/// Ground with tufts: grass blades, flowers or pebbles scattered.
function ground(w: number, h: number, base: string, marks: string[], n: number, seed: number, blades = false): Raster {
  const rand = rng(seed);
  const r = fill(w, h, base);
  // A gentle mottle.
  for (let i = 0; i < (w * h) / 10; i++) put(r, rand() * w, rand() * h, adjust(base, (rand() - 0.5) * 0.06));
  for (let i = 0; i < n; i++) {
    const x = rand() * w;
    const y = rand() * h;
    const c = marks[Math.floor(rand() * marks.length)];
    if (blades) {
      put(r, x, y, c);
      put(r, x, y - 1, c);
      put(r, x + 1, y - 2, c);
      put(r, x - 1, y, adjust(c, -0.1));
    } else {
      put(r, x, y, c);
      put(r, x + 1, y, c);
      put(r, x, y + 1, adjust(c, -0.12));
      put(r, x + 1, y + 1, adjust(c, -0.12));
    }
  }
  return r;
}

/// Sand (or snow) with wind ripples: shallow arcs of shade and light.
function ripples(w: number, h: number, base: string, seed: number, gap = 8): Raster {
  const rand = rng(seed);
  const r = fill(w, h, base);
  for (let y0 = 0; y0 < h; y0 += gap) {
    const phase = rand() * w;
    for (let x = 0; x < w; x++) {
      const y = y0 + Math.round(Math.sin(((x + phase) / w) * Math.PI * 4) * 1.5);
      put(r, x, y, adjust(base, -0.07));
      put(r, x, y + 1, adjust(base, 0.05));
    }
  }
  speckle(r, [adjust(base, -0.1), adjust(base, 0.08)], (w * h) / 45, rand);
  return r;
}

/// A night sky: dark, with stars of a few sizes.
function stars(w: number, h: number, base: string, seed: number): Raster {
  const rand = rng(seed);
  const r = fill(w, h, base);
  for (let i = 0; i < (w * h) / 90; i++) {
    const x = rand() * w;
    const y = rand() * h;
    const big = rand() < 0.15;
    const c = rand() < 0.3 ? "#8ecbf0" : rand() < 0.2 ? "#ffe08a" : "#ffffff";
    put(r, x, y, c);
    if (big) for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]] as const) put(r, x + dx, y + dy, adjust(base, 0.25));
  }
  return r;
}

/// Under water: a gradient from deep to light, with bubbles and light rays.
function water(w: number, h: number, top: string, bottom: string, seed: number): Raster {
  const rand = rng(seed);
  const r = new Raster(w, h);
  const [a, b] = [top, bottom];
  for (let y = 0; y < h; y++) {
    const t = y / (h - 1);
    // Banded rather than smooth: six steps.
    const step = Math.round(t * 5) / 5;
    const c = adjust(a, (adjustL(b) - adjustL(a)) * step);
    for (let x = 0; x < w; x++) r.set(x, y, c);
  }
  for (let x0 = 0; x0 < w; x0 += 16) for (let y = 0; y < h; y++) if ((y + x0) % 3 !== 0) put(r, x0 + y / 3, y, adjust(r.get(x0 % w, y)!, 0.04));
  for (let i = 0; i < w / 10; i++) {
    const x = rand() * w;
    const y = rand() * h;
    put(r, x, y, "#d8f4ff");
    put(r, x + 1, y - 1, "#ffffff");
  }
  return r;
}

function adjustL(hex: string): number {
  const v = parseInt(hex.slice(1), 16);
  return (Math.max(v >> 16, (v >> 8) & 255, v & 255) + Math.min(v >> 16, (v >> 8) & 255, v & 255)) / 510;
}

/// Wallpaper: fine vertical stripes, for the office's plain walls.
function wallpaper(w: number, h: number, base: string): Raster {
  const r = fill(w, h, base);
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x += 8) {
      put(r, x, y, adjust(base, -0.025));
      if (y % 4 === 0) put(r, x + 4, y, adjust(base, -0.02));
    }
  // A dado rail along the bottom.
  for (let x = 0; x < w; x++) {
    put(r, x, h - 6, adjust(base, 0.05));
    put(r, x, h - 5, adjust(base, -0.12));
  }
  return r;
}

/// Wooden boards standing on end (a barn, a cabin).
function boards(w: number, h: number, base: string, bw: number, seed: number): Raster {
  const t = planks(h, w, base, bw, seed);
  // Turn the planks upright.
  const r = new Raster(w, h);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) r.set(x, y, t.get(y, x));
  return r;
}

const OFFICE_ROOMS: Partial<Record<PaneKind, () => Raster>> = {
  terminal: () => planks(64, 48, "#c08a5a", 8, 11),
  files: () => carpet(48, 48, "#7a94b2", 12),
  git: () => plates(48, 48, "#4a5164", 24, 13),
  editor: () => tiles(48, 48, "#d9c8a0", "#bba67a", 16, "#d1bf95"),
  token_usage: () => carpet(48, 48, "#978bc0", 14),
  browser: () => carpet(48, 48, "#7cae87", 15),
  native_browser: () => carpet(48, 48, "#7cae87", 15),
  embedded_browser: () => carpet(48, 48, "#7cae87", 15),
};

const OFFICE_WALLS: Partial<Record<PaneKind, string>> = {
  files: "#dfe6ee",
  git: "#6c7387",
  token_usage: "#ece6f7",
  browser: "#f1e4cf",
  native_browser: "#f1e4cf",
  embedded_browser: "#f1e4cf",
};

type Painter = () => Raster;

/// The tiles of each theme's surfaces (a room's floor in the office by kind).
const THEME_TILES: Record<ThemeId, { corridor: Painter; lobby: Painter; room: Painter | null; wall: Painter | null }> = {
  office: {
    corridor: () => tiles(48, 48, "#5c5e72", "#46485a", 24),
    lobby: () => tiles(48, 48, "#cbc3b0", "#a59c86", 24, "#b5ac97"),
    room: null,
    wall: null,
  },
  cave: {
    corridor: () => ground(64, 64, "#3b322c", ["#5a4c41", "#2a231f", "#4a3e35"], 26, 21),
    lobby: () => bricks(64, 48, "#6b5a48", "#3e332a", 24, 16, 22, 0.08),
    room: () => ground(64, 64, "#7d6a57", ["#5f4f40", "#948068", "#6b5a48"], 22, 23),
    wall: () => bricks(64, WALL_FACE, "#5a4c41", "#2e2520", 20, 12, 24, 0.1),
  },
  castle: {
    corridor: () => bricks(64, 48, "#6b6d7a", "#4a4c58", 32, 16, 31),
    lobby: () => tiles(48, 48, "#b9bac4", "#7a7c88", 24, "#8d8f9c"),
    room: () => bricks(64, 48, "#a6a8b4", "#7d7f8b", 24, 24, 33, 0.05),
    wall: () => bricks(60, WALL_FACE, "#8d8f9c", "#5a5c68", 20, 12, 34),
  },
  ranch: {
    corridor: () => ground(64, 64, "#6cbf4f", ["#4f9a3a", "#8fd46e", "#5aab42"], 70, 41, true),
    lobby: () => ground(64, 64, "#c9a46a", ["#a8844e", "#dcbb84", "#9a9a9a"], 24, 42),
    room: () => planks(64, 45, "#c49a62", 9, 43),
    wall: () => boards(60, WALL_FACE, "#b84a3a", 10, 44),
  },
  space: {
    corridor: () => grating(48, 48, "#353b4a"),
    lobby: () => plates(60, 60, "#4a5160", 30, 52),
    room: () => plates(60, 60, "#5d6570", 30, 53),
    wall: () => stars(64, WALL_FACE, "#1b1f3a", 54),
  },
  sea: {
    corridor: () => ripples(64, 48, "#d9c48a", 61),
    lobby: () => ground(64, 64, "#e6d39a", ["#f7efe0", "#ff9a8a", "#c9b27a"], 14, 62),
    room: () => tiles(48, 48, "#7cc4c8", "#5ea6ab", 24, "#74bcc0"),
    wall: () => water(64, WALL_FACE, "#1f6f9a", "#3a9fc4", 64),
  },
  snow: {
    corridor: () => ripples(64, 48, "#eef4f8", 71, 12),
    lobby: () => ground(64, 64, "#dbe7f0", ["#ffffff", "#c4d4e2"], 30, 72),
    room: () => planks(64, 45, "#b98a5a", 9, 73),
    wall: () => bricks(60, WALL_FACE, "#d6effb", "#9fc6dc", 20, 12, 74, 0.03),
  },
  desert: {
    corridor: () => ripples(64, 48, "#e8cf8f", 81, 10),
    lobby: () => tiles(48, 48, "#d9b77a", "#b8945a", 24, "#cfab6c"),
    room: () => bricks(54, 54, "#d9b77a", "#b8945a", 27, 27, 83, 0.04),
    wall: () => {
      const r = bricks(60, WALL_FACE, "#c49e62", "#9a7a46", 20, 12, 84, 0.05);
      // A painted band of blue and gold.
      for (let x = 0; x < r.w; x++) {
        for (const y of [20, 21, 22]) r.set(x, y, "#3a6fb0");
        r.set(x, 23, "#f5c542");
        r.set(x, 19, "#f5c542");
        if (x % 6 === 0) r.set(x, 21, "#f5c542");
      }
      return r;
    },
  },
};

const cache = new Map<string, Raster | null>();

/// The tile for `surface` in `theme`; `kind` picks an office room's floor
/// and wall. Null where the stylesheet's flat colour is the look.
export function surfaceTile(theme: ThemeId, surface: Surface, kind: PaneKind = "terminal"): Raster | null {
  const key = `${theme}:${surface}:${theme === "office" ? kind : ""}`;
  if (!cache.has(key)) {
    const t = THEME_TILES[theme];
    let tile: Raster | null;
    if (surface === "room") tile = t.room ? t.room() : (OFFICE_ROOMS[kind] ?? OFFICE_ROOMS.terminal!)();
    else if (surface === "wall") tile = t.wall ? t.wall() : wallpaper(64, WALL_FACE, OFFICE_WALLS[kind] ?? "#e8d9b8");
    else tile = t[surface]();
    cache.set(key, tile);
  }
  return cache.get(key) ?? null;
}
