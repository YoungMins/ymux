// Slime characters, drawn from shapes rather than by hand: a body is a stack
// of row widths on a 20x22 canvas (the top rows are headroom for a hero's
// hat) (centred, outlined, a shine at the top
// left, a darker band at the bottom), and the face is placed by the body's
// height. Four bodies cover every pose: `normal`, `squash` (wide, low: the
// landing of a hop, the down-beat of typing), `stretch` (tall: the top of a
// hop, "pick me!") and `melt` (asleep, flat).

import { TIER } from "./heroTiers";

export const SLIME_W = 20;
export const SLIME_H = 22;

export type Body = "normal" | "squash" | "stretch" | "melt";
export const BODIES: readonly Body[] = ["normal", "squash", "stretch", "melt"];
export type Eyes = "open" | "shut" | "happy";
export const EYES: readonly Eyes[] = ["open", "shut", "happy"];

interface Shape {
  /// First row of the body.
  top: number;
  /// Width of each row from the top; even, so the slime stays centred.
  widths: readonly number[];
}

const SHAPES: Record<Body, Shape> = {
  normal: { top: 10, widths: [6, 10, 12, 14, 16, 16, 18, 18, 18, 18, 16, 14] },
  squash: { top: 11, widths: [8, 12, 16, 18, 20, 20, 20, 20, 20, 18, 16] },
  stretch: { top: 6, widths: [4, 8, 10, 10, 10, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 10] },
  melt: { top: 13, widths: [6, 12, 16, 18, 20, 20, 20, 20, 18] },
};

function blank(): string[][] {
  return Array.from({ length: SLIME_H }, () => Array.from({ length: SLIME_W }, () => "."));
}

function rows(grid: string[][]): string[] {
  return grid.map((r) => r.join(""));
}

function left(w: number): number {
  return (SLIME_W - w) / 2;
}

/// The body: outline, jelly, shine and shade.
export function bodyGrid(body: Body): string[] {
  const { top, widths } = SHAPES[body];
  const g = blank();
  const last = widths.length - 1;
  widths.forEach((w, i) => {
    const y = top + i;
    const l = left(w);
    for (let x = l; x < l + w; x++) {
      if (i === 0 || i === last || x === l || x === l + w - 1) g[y][x] = "o";
      else if (i === last - 1) g[y][x] = "C";
      else if (i === last - 2 && (x <= l + 2 || x >= l + w - 3)) g[y][x] = "C";
      else g[y][x] = "c";
    }
  });
  // A shine near the top left, inside the outline.
  const shine = (i: number, dx: number) => {
    const w = widths[i];
    if (w === undefined || dx >= w - 2) return;
    const x = left(w) + dx;
    if (g[top + i][x] === "c") g[top + i][x] = "w";
  };
  if (body === "melt") {
    shine(1, 3);
    shine(1, 4);
  } else {
    shine(1, 2);
    shine(2, 2);
    shine(2, 3);
    shine(3, 2);
  }
  return rows(g);
}

/// Row of the eyes' lower pixel for `body`.
export function eyeRow(body: Body): number {
  const { top, widths } = SHAPES[body];
  return body === "melt" ? top + 4 : top + Math.floor(widths.length / 2);
}

/// The face: eyes, blush and mouth, for `body`.
export function eyesGrid(body: Body, eyes: Eyes): string[] {
  const g = blank();
  const y = eyeRow(body);
  const put = (yy: number, x: number, k: string) => {
    if (yy >= 0 && yy < SLIME_H) g[yy][x] = k;
  };
  if (eyes === "open") {
    for (const x of [7, 12]) {
      put(y - 1, x, "e");
      put(y, x, "e");
    }
  } else if (eyes === "shut") {
    for (const x of [6, 7, 12, 13]) put(y, x, "e");
  } else {
    // ^ ^
    for (const x of [7, 12]) put(y - 1, x, "e");
    for (const x of [6, 8, 11, 13]) put(y, x, "e");
  }
  put(y + 1, 5, "k");
  put(y + 1, 14, "k");
  if (eyes === "happy") for (const x of [9, 10]) put(y + 1, x, "m");
  else if (eyes === "open") put(y + 1, 9, "m"), put(y + 1, 10, "m");
  return rows(g);
}

/// Closed eyes drawn over open ones: jelly over the open eyes, then the
/// lines. One overlay toggled by one animation is a blink that can never
/// show both faces at once.
export function blinkGrid(body: Body): string[] {
  const g = blank();
  const y = eyeRow(body);
  for (const x of [7, 12]) {
    g[y - 1][x] = "c";
    g[y][x] = "c";
  }
  for (const x of [6, 7, 12, 13]) g[y][x] = "e";
  return rows(g);
}

/// A helper's sprout, on top of `body`.
export function sproutGrid(body: Body): string[] {
  const g = blank();
  const t = SHAPES[body].top;
  const put = (y: number, x: number, k: string) => {
    if (y >= 0) g[y][x] = k;
  };
  put(t - 1, 10, "g");
  put(t - 2, 10, "g");
  put(t - 2, 9, "G");
  put(t - 3, 8, "G");
  put(t - 2, 11, "g");
  put(t - 3, 11, "G");
  put(t - 3, 12, "G");
  return rows(g);
}

/// The user's bow tie, low on the front of `body`.
export function tieGrid(body: Body): string[] {
  const g = blank();
  const y = Math.min(eyeRow(body) + 3, SHAPES[body].top + SHAPES[body].widths.length - 3);
  for (const [x, k] of [
    [7, "r"],
    [8, "r"],
    [9, "R"],
    [10, "R"],
    [11, "r"],
    [12, "r"],
  ] as const) {
    g[y][x] = k;
  }
  g[y - 1][7] = "r";
  g[y - 1][12] = "r";
  g[y + 1][7] = "r";
  g[y + 1][12] = "r";
  return rows(g);
}

/// A coffee mug held at the slime's right side.
export const MUG: readonly string[] = [
  ...Array.from({ length: 16 }, () => ".".repeat(SLIME_W)),
  "...............oooo.",
  "...............ouuoo",
  "...............ouuUo",
  "...............ouuUo",
  "...............oooo.",
  "....................",
];

// ── Hero gear ─────────────────────────────────────────────────────────────
// What an account's hero wears (`heroModel`), step by step (`heroTiers`):
// the class's starter gear, its full outfit, a jewel, the class item, a sash,
// gold (a palette swap in `sprites.ts`), wings, then an aura and the
// legendary crown (the glow and the orbiting star are CSS). `back` is drawn
// behind the body.

export type GearClass = "warrior" | "mage" | "rogue" | "cleric" | "cat";

/// Blit `rows` with its bottom row on `y` and centred on the canvas.
function blitOn(g: string[][], rows: readonly string[], y: number, dx = 0): void {
  const w = rows[0].length;
  const x0 = Math.floor((SLIME_W - w) / 2) + dx;
  rows.forEach((r, i) => {
    const yy = y - (rows.length - 1) + i;
    if (yy < 0 || yy >= SLIME_H) return;
    [...r].forEach((k, j) => {
      const xx = x0 + j;
      if (k !== "." && xx >= 0 && xx < SLIME_W) g[yy][xx] = k;
    });
  });
}

/// `rows` grown by a pixel on every side with an outline ("o") traced
/// around what is drawn (every empty pixel with a drawn neighbour).
export function traceOutline(rowsIn: readonly string[]): string[] {
  const w = rowsIn[0].length + 2;
  const g = [".".repeat(w), ...rowsIn.map((r) => `.${r}.`), ".".repeat(w)].map((r) => [...r]);
  const drawn = (x: number, y: number) => y >= 0 && y < g.length && x >= 0 && x < w && g[y][x] !== "." && g[y][x] !== "*";
  for (let y = 0; y < g.length; y++)
    for (let x = 0; x < w; x++)
      if (g[y][x] === "." && (drawn(x - 1, y) || drawn(x + 1, y) || drawn(x, y - 1) || drawn(x, y + 1))) g[y][x] = "*";
  return g.map((r) => r.join("").replace(/\*/g, "o"));
}

/// Interior columns of `body`'s row `i` (inside the outline).
function interior(body: Body, i: number): [number, number] {
  const w = SHAPES[body].widths[i] ?? 0;
  const l = left(w);
  return [l + 1, l + w - 2];
}

const MAGE_HAT = ["....o.....", "...obo....", "..obbbo...", ".obbbbbo..", "oBBBBBBBBo"];
const MAGE_HAT_TALL = [
  ".......obo..",
  "......obbo..",
  ".....obbo...",
  "....obbbo...",
  "...obbybbo..",
  "..obbbbbbbo.",
  "oBBBBJBBBBBo",
];
const HALO = [".yyyyyy.", "y......y", ".yyyyyy."];
const CAT_EARS = [".o........o.", "oco......oco", "ocio....oico"];
const CROWN = ["y.yy.y", "yyJyyy", "YYYYYY"];
export const BIG_CROWN = ["y..y..y", "yy.y.yy", "yyyjyyy", "YYYYYYY"];
const WING = ["..ooo", ".ouuu", "ouuuo", "ouuo.", ".oo.."];

/// The class items, fill only (outlined when drawn), held upright.
export const ITEMS: Readonly<Record<GearClass, readonly string[]>> = {
  warrior: [".a.", ".a.", ".a.", ".a.", ".A.", "yyy", ".d.", ".d."],
  mage: [".j.", "jjj", ".j.", ".d.", ".d.", ".d.", ".d.", ".d.", ".d."],
  rogue: [".a.", ".a.", ".A.", "yyy", ".n."],
  cleric: [".y.", "yyy", ".y.", ".y.", ".d.", ".d.", ".d.", ".d."],
  cat: ["a.aaa.", "aaaaea", "a.aaa."],
};

/// The class colour of a sash or cape, as a gear palette key.
export const CAPE_KEY = "v";

export function gearGrid(body: Body, cls: GearClass, tier: number): { back: string[]; front: string[] } {
  const back = blank();
  const g = blank();
  if (tier <= 0) return { back: rows(back), front: rows(g) };
  const { top, widths } = SHAPES[body];
  // `J` marks where a jewel goes from the jewel tier; `plain` fills it before.
  const gem = (art: readonly string[], plain: string): string[] => art.map((r) => r.replace(/J/g, tier >= TIER.JEWEL ? "j" : plain));
  const band = (y: number, k: string, tail: 1 | -1) => {
    const i = y - top;
    if (i < 0 || i >= widths.length) return;
    const [a, b] = interior(body, i);
    for (let x = a; x <= b; x++) g[y][x] = k;
    const edge = tail > 0 ? b + 2 : a - 2;
    if (edge >= 0 && edge < SLIME_W) g[y][edge] = k;
    const edge2 = tail > 0 ? b + 3 : a - 3;
    if (edge2 >= 0 && edge2 < SLIME_W && y + 1 < SLIME_H) g[y + 1][edge2] = k;
  };
  // A cap over the head: a dome above the top row, the first `rowsN` rows
  // filled (the last as a rim), then only the edges down to `sidesTo` (a hood).
  const cap = (fillKey: string, rimKey: string, rowsN: number, sidesTo = 0) => {
    const [l0, r0] = interior(body, 0);
    for (let x = l0; x <= r0; x++) g[top - 1][x] = x === l0 || x === r0 ? "o" : fillKey;
    for (let i = 0; i < Math.min(Math.max(rowsN, sidesTo), widths.length); i++) {
      const [a, b] = interior(body, i);
      const y = top + i;
      if (i < rowsN) for (let x = a; x <= b; x++) g[y][x] = i === rowsN - 1 ? rimKey : fillKey;
      else for (const x of [a, a + 1, b - 1, b]) g[y][x] = fillKey;
    }
  };
  const bottom = top + widths.length - 1;

  // The class item, behind the body on its right, sticking up past the
  // shoulder (the cat carries its fish in front).
  if (tier >= TIER.ITEM) {
    const item = traceOutline(ITEMS[cls]);
    if (cls === "cat") blitOn(g, item, bottom - 1, 7);
    else blitOn(back, item, bottom - 2, 8);
  }

  switch (cls) {
    case "warrior":
      if (tier === TIER.STARTER) band(top + 2, "r", 1);
      else {
        cap("a", "A", 3);
        // A nose guard and a plume.
        g[top + 3][9] = "A";
        g[top + 3][10] = "A";
        g[top - 2][10] = "r";
        g[top - 2][11] = "r";
        g[top - 3][11] = "R";
        g[top - 3][12] = "r";
        if (tier >= TIER.JEWEL) {
          g[top + 1][9] = "j";
          g[top + 1][10] = "j";
        }
      }
      break;
    case "mage":
      blitOn(g, tier === TIER.STARTER ? MAGE_HAT : gem(MAGE_HAT_TALL, "B"), top);
      break;
    case "rogue":
      if (tier === TIER.STARTER) band(top + 2, "n", -1);
      else {
        // A hood: a cap with a trim, and cloth down both sides of the face.
        cap("n", "N", 3, 6);
        if (tier >= TIER.JEWEL) g[top + 2][10] = "j";
      }
      break;
    case "cleric":
      // A flower circlet (flowers between leaves), then a halo above it.
      {
        const [a, b] = interior(body, 2);
        for (let x = a; x <= b; x++) g[top + 2][x] = (x - a) % 3 === 1 ? "l" : "g";
      }
      if (tier >= TIER.OUTFIT) blitOn(g, HALO, top - 2);
      if (tier >= TIER.JEWEL) g[top + 2][10] = "j";
      break;
    case "cat": {
      blitOn(g, CAT_EARS, top);
      // Whiskers, and from the outfit tier a collar with a bell.
      const wy = eyeRow(body) + 1;
      const i = wy - top;
      if (i >= 0 && i < widths.length) {
        const [a, b] = interior(body, i);
        g[wy][a] = "e";
        g[wy][b] = "e";
      }
      if (tier >= TIER.OUTFIT) {
        const cy = bottom - 2;
        const [a, b] = interior(body, widths.length - 3);
        for (let x = a + 1; x < b; x++) g[cy][x] = "r";
        g[cy][9] = tier >= TIER.JEWEL ? "j" : "y";
        g[cy][10] = "y";
      }
      if (tier >= TIER.GOLD && tier < TIER.LEGEND) blitOn(g, gem(CROWN, "y"), top - 2);
      break;
    }
  }

  // A sash across the body's lower half, with a gold buckle; a cat, whose
  // collar sits there, wears a bow by its ear.
  if (tier >= TIER.CAPE && cls === "cat") {
    g[top - 2][5] = CAPE_KEY;
    g[top - 1][6] = "y";
    g[top - 2][7] = CAPE_KEY;
    g[top][5] = CAPE_KEY;
    g[top][7] = CAPE_KEY;
  } else if (tier >= TIER.CAPE) {
    const sy = Math.min(eyeRow(body) + 3, bottom - 2);
    const [a, b] = interior(body, sy - top);
    for (let x = a; x <= b; x++) g[sy][x] = CAPE_KEY;
    g[sy][9] = "y";
    g[sy][10] = "y";
  }
  if (tier >= TIER.WINGS) {
    const wy = top + 4;
    blitOn(back, WING, wy, -7);
    blitOn(back, WING.map((r) => [...r].reverse().join("")), wy, 8);
  }
  // Legendary: a big crown (over the helmet, hood or ears); a mage's hat is
  // ringed with stars instead, and a cleric's halo is jewelled.
  if (tier >= TIER.LEGEND) {
    if (cls === "mage") for (const [x, y] of [[4, top - 2], [15, top - 4], [6, top - 6]] as const) g[y][x] = "y";
    else if (cls === "cleric") g[top - 4][9] = g[top - 4][10] = "j";
    else blitOn(g, traceOutline(BIG_CROWN), top - 1);
  }
  return { back: rows(back), front: rows(g) };
}
