// Slime characters, drawn from shapes rather than by hand: a body is a stack
// of row widths on a 20x18 canvas (centred, outlined, a shine at the top
// left, a darker band at the bottom), and the face is placed by the body's
// height. Four bodies cover every pose: `normal`, `squash` (wide, low: the
// landing of a hop, the down-beat of typing), `stretch` (tall: the top of a
// hop, "pick me!") and `melt` (asleep, flat).

export const SLIME_W = 20;
export const SLIME_H = 18;

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
  normal: { top: 6, widths: [6, 10, 12, 14, 16, 16, 18, 18, 18, 18, 16, 14] },
  squash: { top: 7, widths: [8, 12, 16, 18, 20, 20, 20, 20, 20, 18, 16] },
  stretch: { top: 2, widths: [4, 8, 10, 10, 10, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 10] },
  melt: { top: 9, widths: [6, 12, 16, 18, 20, 20, 20, 20, 18] },
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
  ...Array.from({ length: 12 }, () => ".".repeat(SLIME_W)),
  "...............oooo.",
  "...............ouuoo",
  "...............ouuUo",
  "...............ouuUo",
  "...............oooo.",
  "....................",
];
