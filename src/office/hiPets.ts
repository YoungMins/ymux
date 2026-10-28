// The office's pets at one art pixel per CSS pixel: a cat, a dog, a rabbit and
// a chick that wander the corridors and the lobby on their own. Each is a
// 40x28 frame facing right, painted from parts (`paint.ts`): two walk frames
// (the rabbit hops), sitting and asleep.

import { Raster, capsule, dome, ellipse, flat, paint, polygon, ramp, rect, slope, union, type Ramp } from "./paint";

export const PET_SPECIES = ["cat", "dog", "rabbit", "chick"] as const;
/// Pets bought in the office shop (`funModel.SHOP`).
export const SHOP_PETS = ["penguin", "turtle", "blackcat"] as const;
export type PetSpecies = (typeof PET_SPECIES)[number] | (typeof SHOP_PETS)[number];
export type PetPose = "walkA" | "walkB" | "sit" | "sleep";
export const PET_POSES: readonly PetPose[] = ["walkA", "walkB", "sit", "sleep"];

export const PET_W = 40;
export const PET_H = 28;
const GROUND = 26;
const INK = "#2a1f1a";

interface Coat {
  body: Ramp;
  belly: Ramp;
  pink: string;
  nose: string;
}

const COATS: Record<PetSpecies, Coat> = {
  cat: { body: ramp("#f0a050"), belly: ramp("#fff4e0", "#6a5a40"), pink: "#ff9fb0", nose: "#e0708a" },
  dog: { body: ramp("#c8925a"), belly: ramp("#f8f0e0", "#6a5a40"), pink: "#ff9fb0", nose: INK },
  rabbit: { body: ramp("#f2f0ea", "#6a6458"), belly: ramp("#ffffff", "#6a6458"), pink: "#ffb0c0", nose: "#e0708a" },
  chick: { body: ramp("#ffd84a", "#7a5a10"), belly: ramp("#fff6c0", "#7a5a10"), pink: "#ffb0a0", nose: "#f08a2a" },
  penguin: { body: ramp("#2f3440", "#0e1016"), belly: ramp("#f4f4f0", "#5a5a58"), pink: "#ffb0b0", nose: "#f0a030" },
  turtle: { body: ramp("#7cbf5a", "#24461a"), belly: ramp("#c8a060", "#4a3418"), pink: "#ffb0b0", nose: "#2a1f1a" },
  blackcat: { body: ramp("#3a3442", "#121016"), belly: ramp("#5a5266", "#121016"), pink: "#ff9fb0", nose: "#e0708a" },
};

/// A leg from the body down to the ground, its foot `dx` from the hip.
function leg(out: Raster, x: number, top: number, dx: number, r: Ramp, lift = 0, w = 1.7): void {
  paint(out, capsule(x, top, x + dx, GROUND - 1 - lift, w), r, { light: slope(x - 2, top, 4, GROUND - top) });
}

function eye(out: Raster, x: number, y: number, open: boolean): void {
  if (open) {
    paint(out, ellipse(x, y, 1.2, 1.7), flat(INK), { outline: false });
    out.set(x - 0.5, y - 1, "#ffffff");
  } else for (let dx = -1; dx <= 1; dx++) out.set(x + dx, y + (dx === 0 ? 0 : -1), INK);
}

function cat(out: Raster, pose: PetPose, c: Coat): void {
  if (pose === "sleep") {
    paint(out, capsule(8, 22, 4, 25, 1.8), c.body);
    paint(out, ellipse(19, 21.5, 12, 5), c.body, { light: dome(16, 17, 12, 6) });
    paint(out, ellipse(29, 21, 5.5, 4.6), c.body, { light: dome(27, 18, 6, 5) });
    for (const ex of [25, 31]) paint(out, polygon([[ex - 2.5, 18], [ex, 13.5], [ex + 2.5, 18]]), c.body);
    eye(out, 30, 21, false);
    out.set(33.5, 22.5, c.nose);
    return;
  }
  const sit = pose === "sit";
  // Tail, legs at the back, body, head.
  paint(out, sit ? union(capsule(11, 22, 6, 24, 1.8), capsule(6, 24, 3, 20, 1.8)) : union(capsule(10, 17, 5, 13, 1.8), capsule(5, 13, 5, 7, 1.8)), c.body, {
    light: slope(2, 6, 10, 20),
  });
  if (sit) {
    paint(out, ellipse(19, 19, 9, 6.5), c.body, { light: dome(16, 15, 9, 7) });
    paint(out, ellipse(23, 21, 4, 4.4), c.belly, { light: dome(22, 19, 4, 4) });
    leg(out, 21, 20, 1, c.body);
    leg(out, 25, 20, 1, c.body);
  } else {
    const step = pose === "walkA" ? 2 : -2;
    leg(out, 12, 19, -step, c.body);
    leg(out, 25, 19, step, c.body);
    paint(out, ellipse(18, 18, 10, 5), c.body, { light: dome(15, 14, 10, 6) });
    paint(out, ellipse(20, 21, 6, 2), c.belly, { outline: false, light: () => 0.7 });
    leg(out, 15, 19, step, c.body);
    leg(out, 22, 19, -step, c.body);
  }
  const hx = sit ? 26 : 29;
  const hy = sit ? 10 : 12;
  for (const ex of [hx - 3.5, hx + 3]) {
    paint(out, polygon([[ex - 3, hy - 2], [ex, hy - 8], [ex + 3, hy - 2]]), c.body, { light: slope(ex - 3, hy - 8, 6, 6) });
    paint(out, polygon([[ex - 1.4, hy - 2.5], [ex, hy - 6], [ex + 1.4, hy - 2.5]]), flat(c.pink), { outline: false });
  }
  paint(out, ellipse(hx, hy, 6.5, 5.6), c.body, { light: dome(hx - 2, hy - 3, 7, 6) });
  paint(out, ellipse(hx + 2.5, hy + 2.5, 3.2, 2.2), c.belly, { outline: false, light: () => 0.8 });
  // Stripes.
  for (const sx of sit ? [15, 18] : [14, 18, 22]) for (let i = 0; i < 3; i++) out.set(sx + (i === 1 ? 1 : 0), (sit ? 13 : 13.5) + i, c.body.tones[0]);
  eye(out, hx + 2, hy - 0.5, true);
  out.set(hx + 5.5, hy + 1.5, c.nose);
  out.set(hx + 4.5, hy + 3.5, INK);
  for (let i = 0; i < 3; i++) out.set(hx + 6.5 + i, hy + 2 + (i === 2 ? 1 : 0), INK);
}

function dog(out: Raster, pose: PetPose, c: Coat): void {
  const dark = ramp("#7a4e2a");
  if (pose === "sleep") {
    paint(out, ellipse(19, 21.5, 12, 5), c.body, { light: dome(16, 17, 12, 6) });
    paint(out, ellipse(30, 21.5, 5.5, 4.5), c.body, { light: dome(28, 19, 6, 5) });
    paint(out, ellipse(35, 23, 3, 2.2), c.belly);
    paint(out, ellipse(28, 20, 2.4, 4), dark, { light: dome(27, 18, 2, 4) });
    eye(out, 31, 21, false);
    out.set(37.5, 22, c.nose);
    return;
  }
  const sit = pose === "sit";
  paint(out, capsule(9, sit ? 21 : 16, 5, sit ? 18 : 11, 1.8), c.body, { light: slope(4, 10, 6, 10) });
  if (sit) {
    paint(out, ellipse(19, 19, 9, 6.5), c.body, { light: dome(16, 15, 9, 7) });
    paint(out, ellipse(23, 20, 4, 4.4), c.belly, { light: dome(22, 18, 4, 4) });
    leg(out, 21, 20, 1, c.body, 0, 1.9);
    leg(out, 25, 20, 1, c.body, 0, 1.9);
  } else {
    const step = pose === "walkA" ? 2 : -2;
    leg(out, 12, 18, -step, c.body, 0, 1.9);
    leg(out, 25, 18, step, c.body, 0, 1.9);
    paint(out, ellipse(18, 17.5, 10.5, 5.5), c.body, { light: dome(15, 13, 10, 6) });
    // A patch on the back.
    paint(out, ellipse(15, 14.5, 4, 2.6), dark, { outline: false, light: dome(14, 13, 4, 3) });
    leg(out, 15, 19, step, c.body, 0, 1.9);
    leg(out, 22, 19, -step, c.body, 0, 1.9);
  }
  const hx = sit ? 27 : 30;
  const hy = sit ? 10 : 11;
  paint(out, ellipse(hx, hy, 6, 5.5), c.body, { light: dome(hx - 2, hy - 3, 6, 6) });
  paint(out, ellipse(hx + 5, hy + 2.5, 3.6, 2.6), c.belly, { light: dome(hx + 4, hy + 1, 3, 3) });
  // A floppy ear.
  paint(out, ellipse(hx - 2.5, hy + 1, 2.6, 4.4), dark, { light: dome(hx - 3, hy - 1, 3, 4) });
  eye(out, hx + 1.5, hy - 1, true);
  paint(out, ellipse(hx + 8, hy + 1.5, 1.4, 1.1), flat(c.nose), { outline: false });
  out.set(hx + 5, hy + 4.5, c.pink);
}

function rabbit(out: Raster, pose: PetPose, c: Coat): void {
  const lift = pose === "walkB" ? 3 : 0;
  const ears = (hx: number, hy: number, back: boolean) => {
    for (const [dx, tilt] of [[-1.5, back ? -4 : -2], [1.5, back ? -1 : 1]] as const) {
      paint(out, capsule(hx + dx, hy - 2, hx + dx + tilt, hy - 11, 1.9), c.body, { light: slope(hx - 4, hy - 12, 8, 10) });
      paint(out, capsule(hx + dx + 0.2, hy - 3, hx + dx + tilt * 0.9, hy - 9.5, 0.6), flat(c.pink), { outline: false });
    }
  };
  if (pose === "sleep") {
    paint(out, ellipse(19, 21.5, 10, 5.5), c.body, { light: dome(16, 17, 10, 6) });
    paint(out, ellipse(27, 21.5, 5, 4.5), c.body, { light: dome(25, 19, 5, 5) });
    paint(out, capsule(25, 18, 15, 16, 1.9), c.body);
    paint(out, ellipse(9, 20, 2.6, 2.6), c.belly);
    eye(out, 28, 21, false);
    out.set(31.5, 22.5, c.nose);
    return;
  }
  paint(out, ellipse(9, 19 - lift, 2.8, 2.8), c.belly, { light: dome(8, 18 - lift, 3, 3) });
  // Hind leg, a big foot.
  paint(out, ellipse(14, GROUND - 1.5 - lift, 4.5, 1.8), c.body);
  if (pose !== "sit") leg(out, 23, 20 - lift, lift ? 2 : 0, c.body, lift ? 1 : 0);
  paint(out, ellipse(17, 19.5 - lift, 8.5, 6), c.body, { light: dome(14, 15 - lift, 9, 7) });
  if (pose === "sit") leg(out, 22, 21, 0, c.body);
  const hx = pose === "sit" ? 24 : 26;
  const hy = 13.5 - lift;
  ears(hx, hy, pose !== "sit");
  paint(out, ellipse(hx, hy, 5.2, 4.6), c.body, { light: dome(hx - 2, hy - 2, 5, 5) });
  eye(out, hx + 1.5, hy - 0.5, true);
  out.set(hx + 4.5, hy + 1.5, c.nose);
  paint(out, ellipse(hx - 1.5, hy + 2.5, 1.6, 0.9), flat(c.pink), { outline: false });
}

function chick(out: Raster, pose: PetPose, c: Coat): void {
  const feet = flat(c.nose, "#8a4a10");
  if (pose === "sleep") {
    paint(out, ellipse(20, 21, 9, 6), c.body, { light: dome(18, 17, 9, 7) });
    paint(out, ellipse(18, 21, 4.5, 3), c.body, { light: dome(16, 19, 5, 3) });
    eye(out, 24, 19.5, false);
    paint(out, polygon([[28, 20], [31, 21], [28, 22]]), feet);
    return;
  }
  const lift = pose === "walkB" ? 1 : 0;
  const stepA = pose === "walkA";
  for (const [x, dx] of [[18, stepA ? -1 : 1], [21, stepA ? 1 : -1]] as const) {
    const fx = pose === "sit" ? x : x + dx;
    for (let y = 22; y < GROUND; y++) out.set(fx, y, c.nose);
    out.set(fx + 1, GROUND - 1, c.nose);
    out.set(fx - 1, GROUND - 1, c.nose);
  }
  paint(out, ellipse(19.5, 18 - lift, 8, 6.5), c.body, { light: dome(17, 14 - lift, 8, 7) });
  paint(out, ellipse(24, 11 - lift, 5.2, 5), c.body, { light: dome(22, 8 - lift, 5, 5) });
  // A tuft, a wing, the beak.
  paint(out, capsule(23, 6.5 - lift, 22, 3.5 - lift, 0.8), c.body, { outline: false });
  paint(out, ellipse(16.5, 18.5 - lift, 4.5, 3), c.body, { light: dome(15, 17 - lift, 4, 3) });
  paint(out, polygon([[28.5, 10 - lift], [32, 11.5 - lift], [28.5, 13 - lift]]), feet);
  eye(out, 25.5, 10 - lift, true);
  paint(out, ellipse(25, 13.5 - lift, 1.5, 0.8), flat(c.pink), { outline: false });
}

function penguin(out: Raster, pose: PetPose, c: Coat): void {
  const feet = flat(c.nose, "#8a4a10");
  if (pose === "sleep") {
    paint(out, ellipse(20, 21, 9, 5.5), c.body, { light: dome(18, 18, 9, 6) });
    paint(out, ellipse(23, 22, 5, 3.5), c.belly, { outline: false, light: () => 0.75 });
    eye(out, 26, 19.5, false);
    paint(out, polygon([[29, 20], [32, 21], [29, 22]]), feet);
    return;
  }
  // A waddle: the body tips from side to side.
  const tilt = pose === "walkA" ? -1 : pose === "walkB" ? 1 : 0;
  for (const [x, lift] of [[17, pose === "walkA" ? 1 : 0], [22, pose === "walkB" ? 1 : 0]] as const) {
    paint(out, ellipse(x, GROUND - 1 - lift, 2.6, 1.3), feet);
  }
  paint(out, ellipse(19.5 + tilt, 15.5, 7.5, 9.5), c.body, { light: dome(17 + tilt, 11, 8, 9) });
  paint(out, ellipse(21 + tilt, 17.5, 4.6, 6.5), c.belly, { outline: false, light: () => 0.8 });
  // Flippers.
  paint(out, ellipse(13 + tilt, 17, 1.8, 4.5), c.body, { light: dome(13, 15, 2, 4) });
  eye(out, 22.5 + tilt, 11, true);
  paint(out, polygon([[25 + tilt, 12], [29 + tilt, 13.5], [25 + tilt, 14.5]]), feet);
  paint(out, ellipse(22 + tilt, 14, 1.4, 0.7), flat(c.pink), { outline: false });
}

function turtle(out: Raster, pose: PetPose, c: Coat): void {
  const skin = ramp("#a8c87a", "#2e4a1e");
  const step = pose === "walkA" ? 1 : pose === "walkB" ? -1 : 0;
  if (pose !== "sleep") {
    // Head out front, legs below the shell.
    paint(out, ellipse(31 + Math.max(0, step), 19, 3.6, 3), skin, { light: dome(30, 17, 3, 3) });
    eye(out, 32 + Math.max(0, step), 18.2, true);
  }
  for (const [x, dx] of [[12, step], [24, -step]] as const) paint(out, ellipse(x + dx, GROUND - 2, 2.6, 1.8), skin);
  paint(out, ellipse(4, 22, 2, 1.2), skin);
  // The shell: a dome with plates.
  const shell = intersectTop(ellipse(18, 22, 12, 9), 22.5);
  paint(out, shell, c.body, { light: dome(15, 15, 12, 9) });
  for (const [x, y] of [[13, 18], [18, 16.5], [23, 18], [15.5, 21], [20.5, 21]] as const) paint(out, ellipse(x, y, 2.2, 1.6), ramp("#5a9a42", "#24461a"), { outline: false, light: () => 0.35 });
  paint(out, rect(6, 21.5, 24, 2), c.belly, { light: slope(6, 21, 24, 2) });
  if (pose === "sleep") paint(out, ellipse(30, 22, 2.4, 1.6), skin);
}

/// The part of `m` above `y`.
function intersectTop(m: ReturnType<typeof ellipse>, y: number): ReturnType<typeof ellipse> {
  const out = ((x: number, yy: number) => yy < y && m(x, yy)) as ReturnType<typeof ellipse>;
  out.box = m.box;
  return out;
}

/// One frame of a pet.
export function petFrame(species: PetSpecies, pose: PetPose): Raster {
  const out = new Raster(PET_W, PET_H);
  const c = COATS[species];
  ({ cat, dog, rabbit, chick, penguin, turtle, blackcat: cat })[species](out, pose, c);
  return out;
}
