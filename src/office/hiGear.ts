// Hero gear shared by the hi-res slimes and humans: the class items, wings,
// crowns, haloes, jewels and colour ramps. Each painter takes an anchor in
// sprite pixels; the slime and human artists decide where it goes.

import { TIER } from "./heroTiers";
import { capsule, dome, ellipse, minus, paint, polygon, ramp, rect, slope, type Ramp, type Raster } from "./paint";

export type GearClass = "warrior" | "mage" | "rogue" | "cleric" | "cat";

export const METAL = ramp("#b9c3d0", "#3c4250");
export const GOLD = ramp("#f0bf3a", "#6b4a12");
export const WOOD = ramp("#9a6236", "#3e2414");
export const RED = ramp("#c8473a", "#4a1712");
export const JEWEL = ramp("#46d6e6", "#13505a");
export const WHITE = ramp("#eeeae2", "#5a5650");
export const LEAF = ramp("#5fb84a", "#1f4a16");
export const PINK = ramp("#ff8fb0", "#7a2a44");
export const MAGE_CLOTH = ramp("#5a6fe0", "#1e2660");
export const ROGUE_CLOTH = ramp("#5b4f8a", "#1d1830");

/// The sash / cape colour of each class.
export const CAPE: Record<GearClass, Ramp> = {
  warrior: ramp("#c4402f", "#4a1410"),
  mage: ramp("#7a4fd0", "#2a1650"),
  rogue: ramp("#3a3550", "#14121c"),
  cleric: ramp("#f2e4a6", "#6b5a2a"),
  cat: ramp("#ff8fc0", "#7a2a50"),
};

/// The metal of the gear at `tier`: steel, then gold.
export function metal(tier: number): Ramp {
  return tier >= TIER.GOLD ? GOLD : METAL;
}

/// A small cut jewel centred on (cx, cy).
export function jewel(out: Raster, cx: number, cy: number, r = 2.5, j: Ramp = JEWEL): void {
  paint(out, polygon([[cx, cy - r], [cx + r, cy], [cx, cy + r], [cx - r, cy]]), j, { light: dome(cx - 1, cy - 1, r, r) });
  out.set(Math.floor(cx) - 1, Math.floor(cy) - 1, "#ffffff");
}

/// The class item standing upright with its foot at (x, foot), drawn on
/// `out` (behind the body for slimes, in hand for humans: the hand is at
/// `grip` pixels above the foot).
export function item(out: Raster, cls: GearClass, x: number, foot: number, tier: number): void {
  const m = metal(tier);
  switch (cls) {
    case "warrior": {
      // Blade, guard, grip, pommel.
      paint(out, polygon([[x - 1.5, foot - 9], [x + 1.5, foot - 9], [x + 1.5, foot - 26], [x, foot - 29], [x - 1.5, foot - 26]]), m, {
        light: slope(x - 2, foot - 29, 4, 20),
      });
      out.set(x - 1, foot - 25, "#ffffff");
      out.set(x - 1, foot - 21, "#ffffff");
      paint(out, rect(x - 5, foot - 10, 10, 3), GOLD, { light: slope(x - 5, foot - 10, 10, 3) });
      paint(out, rect(x - 1, foot - 7, 3, 6), WOOD);
      paint(out, ellipse(x + 0.5, foot - 1, 2.2, 2), GOLD, { light: dome(x, foot - 2, 2, 2) });
      break;
    }
    case "mage": {
      paint(out, capsule(x, foot - 22, x, foot, 1.3), WOOD, { light: slope(x - 1, foot - 22, 3, 22) });
      paint(out, ellipse(x + 0.5, foot - 25, 4.2, 4.2), JEWEL, { light: dome(x - 1, foot - 27, 4, 4) });
      out.set(x - 1, foot - 27, "#ffffff");
      out.set(x, foot - 27, "#ffffff");
      // Prongs holding the orb.
      paint(out, capsule(x - 3, foot - 21, x - 4, foot - 25, 0.8), m, { outline: false });
      paint(out, capsule(x + 4, foot - 21, x + 5, foot - 25, 0.8), m, { outline: false });
      break;
    }
    case "rogue": {
      paint(out, polygon([[x - 1.5, foot - 6], [x + 1.5, foot - 6], [x + 1.5, foot - 15], [x, foot - 18], [x - 1.5, foot - 15]]), m, {
        light: slope(x - 2, foot - 18, 4, 12),
      });
      out.set(x - 1, foot - 14, "#ffffff");
      paint(out, rect(x - 4, foot - 7, 8, 2), GOLD);
      paint(out, rect(x - 1, foot - 5, 3, 5), ROGUE_CLOTH);
      break;
    }
    case "cleric": {
      paint(out, capsule(x, foot - 20, x, foot, 1.3), WOOD, { light: slope(x - 1, foot - 20, 3, 20) });
      // A cross with a jewel.
      paint(out, rect(x - 1, foot - 30, 3, 11), GOLD, { light: slope(x - 1, foot - 30, 3, 11) });
      paint(out, rect(x - 4, foot - 27, 9, 3), GOLD, { light: slope(x - 4, foot - 27, 9, 3) });
      jewel(out, x + 0.5, foot - 25.5, 1.6);
      break;
    }
    case "cat": {
      // A fish, held by the tail.
      const fish = tier >= TIER.GOLD ? GOLD : ramp("#a9c4d8", "#2c3e50");
      paint(out, ellipse(x + 6, foot - 4, 6.5, 3.4), fish, { light: dome(x + 4, foot - 6, 6, 3) });
      paint(out, polygon([[x - 1, foot - 4], [x - 5, foot - 8], [x - 5, foot]]), fish, { light: slope(x - 5, foot - 8, 5, 8) });
      out.set(x + 9, foot - 5, "#1d1a18");
      break;
    }
  }
}

/// A feathered wing whose root is at (x, y), sweeping up and out to the
/// left (`dir` -1) or right (1).
export function wing(out: Raster, x: number, y: number, dir: 1 | -1): void {
  const p = (dx: number, dy: number): [number, number] => [x + dir * dx, y + dy];
  const shape = polygon([p(0, 2), p(1, -6), p(5, -13), p(10, -17), p(13, -18), p(12, -13), p(14, -11), p(11, -7), p(13, -5), p(9, -1), p(10, 1), p(4, 4)]);
  paint(out, shape, WHITE, { light: slope(Math.min(x, x + dir * 14), y - 18, 14, 22) });
  // Feather lines.
  for (const [a, b] of [
    [p(4, -4), p(11, -12)],
    [p(4, 0), p(10, -5)],
  ] as const) {
    const n = 8;
    for (let i = 0; i <= n; i++) out.set(a[0] + ((b[0] - a[0]) * i) / n, a[1] + ((b[1] - a[1]) * i) / n, WHITE.tones[1]);
  }
}

/// A golden ring floating with its centre at (cx, cy).
export function halo(out: Raster, cx: number, cy: number, rx = 12, jewelled = false): void {
  paint(out, minus(ellipse(cx, cy, rx, 3.6), ellipse(cx, cy + 0.3, rx - 3.5, 1.6)), GOLD, { light: dome(cx - 3, cy - 2, rx, 4) });
  if (jewelled) for (const dx of [-rx + 2, 0, rx - 2]) jewel(out, cx + dx, cy + (dx === 0 ? -2.2 : 0), 1.6);
}

/// A crown sitting with its band's bottom on y, centred on cx.
export function crown(out: Raster, cx: number, y: number, w = 18, big = false): void {
  const h = big ? 11 : 7;
  const half = w / 2;
  const spikes: [number, number][] = big
    ? [[cx - half, y], [cx - half, y - h + 2], [cx - half / 2, y - h + 6], [cx - 2, y - h], [cx + 2, y - h], [cx + half / 2, y - h + 6], [cx + half, y - h + 2], [cx + half, y]]
    : [[cx - half, y], [cx - half, y - h], [cx - half / 3, y - h + 3], [cx, y - h], [cx + half / 3, y - h + 3], [cx + half, y - h], [cx + half, y]];
  paint(out, polygon(spikes), GOLD, { light: slope(cx - half, y - h, w, h) });
  if (big) {
    jewel(out, cx, y - 4, 2, ramp("#e04a5a", "#5a1018"));
    jewel(out, cx - half / 2 - 1, y - 3, 1.6);
    jewel(out, cx + half / 2 + 1, y - 3, 1.6, LEAF);
  } else jewel(out, cx, y - 3, 1.6, ramp("#e04a5a", "#5a1018"));
}

/// A four-point sparkle.
export function sparkle(out: Raster, x: number, y: number, c = "#fff6b0"): void {
  for (const [dx, dy] of [[0, 0], [1, 0], [-1, 0], [0, 1], [0, -1], [0, 2], [0, -2]] as const) out.set(x + dx, y + dy, c);
}
