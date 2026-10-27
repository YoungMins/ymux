// Human heroes: a chibi adventurer on the slimes' 20x22 canvas, for accounts
// whose hero form is "human" (`heroModel`). Parts are placed as fill
// pixels and the outline is traced around the whole figure afterwards, so a
// pose is just where the arms, legs and eyes go. The class outfit and its
// tiers mirror the slimes' gear (`slimeArt.gearGrid`): starter gear, full
// outfit, a jewel, gold (a palette swap in `sprites.ts`), wings.

import { SLIME_H, SLIME_W } from "./slimeArt";
import type { GearClass } from "./slimeArt";

export type Arms = "down" | "raise" | "type" | "typeUp" | "up" | "mug" | "sip" | "swingA" | "swingB";
export type Legs = "stand" | "stepA" | "stepB";
export type HumanEyes = "open" | "shut" | "happy";

export interface HumanPose {
  arms: Arms;
  legs: Legs;
  eyes: HumanEyes;
}

type Grid = string[][];

function blank(): Grid {
  return Array.from({ length: SLIME_H }, () => Array.from({ length: SLIME_W }, () => "."));
}

function put(g: Grid, x: number, y: number, k: string): void {
  if (x >= 0 && x < SLIME_W && y >= 0 && y < SLIME_H) g[y][x] = k;
}

function hline(g: Grid, x0: number, x1: number, y: number, k: string): void {
  for (let x = x0; x <= x1; x++) put(g, x, y, k);
}

/// Trace an outline ("o") around everything drawn: every empty pixel with a
/// drawn neighbour (left, right, up, down).
function outline(g: Grid): void {
  const filled = (x: number, y: number) => x >= 0 && x < SLIME_W && y >= 0 && y < SLIME_H && g[y][x] !== "." && g[y][x] !== "*";
  for (let y = 0; y < SLIME_H; y++) {
    for (let x = 0; x < SLIME_W; x++) {
      if (g[y][x] !== ".") continue;
      if (filled(x - 1, y) || filled(x + 1, y) || filled(x, y - 1) || filled(x, y + 1)) g[y][x] = "*";
    }
  }
  for (const row of g) for (let x = 0; x < row.length; x++) if (row[x] === "*") row[x] = "o";
}

const rows = (g: Grid): string[] => g.map((r) => r.join(""));

// Layout (x 0..19, y 0..21): hat room in rows 0..3, the head in 4..11 with
// its chin line on 12, the torso in 13..17 and the legs in 18..21.
const HEAD_TOP = 4;
const CHIN = 12;

/// Robes reach down over the legs.
function robed(cls: GearClass, tier: number): boolean {
  return tier >= 2 && (cls === "mage" || cls === "cleric");
}

export function humanGrid(pose: HumanPose, cls: GearClass, tier: number): { back: string[]; front: string[] } {
  const g = blank();
  const back = blank();

  // Legs, then the torso, then the arms over it.
  const leg = (x: number, lift: number) => {
    for (let y = 18; y <= 20 - lift; y++) hline(g, x, x + 1, y, "q");
    hline(g, x, x + 1, 21 - lift, "d");
  };
  leg(7, pose.legs === "stepA" ? 1 : 0);
  leg(11, pose.legs === "stepB" ? 1 : 0);
  for (let y = 13; y <= 17; y++) hline(g, 7, 12, y, y === 17 ? "T" : "t");
  hline(g, 7, 12, 16, "d");
  if (robed(cls, tier)) for (let y = 16; y <= 20; y++) hline(g, 7, 12, y, y === 20 ? "T" : "t");

  // Arms: [x, y] of sleeve pixels and of the hand.
  const arm = (cells: [number, number][], hand: [number, number]) => {
    for (const [x, y] of cells) put(g, x, y, "T");
    put(g, hand[0], hand[1], "s");
  };
  const left = (a: Arms) => {
    switch (a) {
      case "up":
        return arm([[6, 13], [5, 12], [5, 11]], [5, 10]);
      case "type":
        return arm([[6, 13], [6, 14]], [7, 15]);
      case "typeUp":
        return arm([[6, 13], [6, 14]], [7, 14]);
      case "swingA":
        return arm([[6, 13], [6, 14], [6, 15]], [6, 16]);
      case "swingB":
        return arm([[6, 13], [6, 14]], [6, 15]);
      default:
        return arm([[6, 13], [6, 14], [6, 15]], [6, 16]);
    }
  };
  const right = (a: Arms) => {
    switch (a) {
      case "up":
      case "raise":
        return arm([[13, 13], [14, 12], [14, 11]], [14, 10]);
      case "type":
        return arm([[13, 13], [13, 14]], [12, 15]);
      case "typeUp":
        return arm([[13, 13], [13, 14]], [12, 14]);
      case "mug":
        return arm([[13, 13], [13, 14]], [14, 15]);
      case "sip":
        return arm([[13, 13], [14, 12]], [14, 11]);
      case "swingA":
        return arm([[13, 13], [13, 14]], [13, 15]);
      case "swingB":
        return arm([[13, 13], [13, 14], [13, 15]], [13, 16]);
      default:
        return arm([[13, 13], [13, 14], [13, 15]], [13, 16]);
    }
  };
  left(pose.arms);
  right(pose.arms);

  // The head: skin, rounded corners, hair over the top and down the sides.
  for (let y = HEAD_TOP; y <= 11; y++) {
    const inset = y === HEAD_TOP || y === 11 ? 1 : 0;
    hline(g, 6 + inset, 13 - inset, y, "s");
  }
  hline(g, 7, 12, HEAD_TOP, "h");
  hline(g, 6, 13, HEAD_TOP + 1, "h");
  for (const x of [6, 7, 9, 12, 13]) put(g, x, HEAD_TOP + 2, x === 9 ? "H" : "h");
  for (let y = HEAD_TOP + 3; y <= HEAD_TOP + 5; y++) {
    put(g, 6, y, "h");
    put(g, 13, y, "h");
  }
  hline(g, 7, 12, CHIN, "*"); // the chin line, kept clear for the outline

  // The face.
  if (pose.eyes === "open") for (const x of [8, 11]) [8, 9].forEach((y) => put(g, x, y, "e"));
  else if (pose.eyes === "shut") for (const x of [7, 8, 11, 12]) put(g, x, 9, "e");
  else {
    // ^ ^, one stroke each so the two never touch.
    put(g, 7, 9, "e");
    put(g, 8, 8, "e");
    put(g, 11, 8, "e");
    put(g, 12, 9, "e");
  }
  put(g, 7, 10, "k");
  put(g, 12, 10, "k");
  if (pose.eyes !== "shut") hline(g, 9, 10, 11, "m");

  // A mug in the right hand.
  if (pose.arms === "mug" || pose.arms === "sip") {
    const [mx, my] = pose.arms === "mug" ? [15, 14] : [15, 10];
    hline(g, mx, mx + 1, my, "u");
    hline(g, mx, mx + 1, my + 1, "U");
  }

  gear(g, back, cls, tier);
  outline(g);
  outline(back);
  return { back: rows(back), front: rows(g) };
}

/// The class outfit by tier, drawn over the figure before it is outlined.
function gear(g: Grid, back: Grid, cls: GearClass, tier: number): void {
  if (tier <= 0) return;
  const jewel = tier >= 3;
  switch (cls) {
    case "warrior":
      if (tier === 1) {
        hline(g, 6, 13, HEAD_TOP + 2, "r");
        put(g, 14, HEAD_TOP + 2, "r");
        put(g, 15, HEAD_TOP + 3, "R");
      } else {
        // A helmet with cheek guards and a plume, and a breastplate.
        hline(g, 7, 12, HEAD_TOP - 1, "a");
        for (let y = HEAD_TOP; y <= HEAD_TOP + 1; y++) hline(g, 6, 13, y, "a");
        hline(g, 6, 13, HEAD_TOP + 2, "A");
        for (const x of [6, 13]) for (let y = HEAD_TOP + 3; y <= HEAD_TOP + 5; y++) put(g, x, y, "a");
        put(g, 10, HEAD_TOP - 2, "r");
        put(g, 11, HEAD_TOP - 3, "r");
        put(g, 12, HEAD_TOP - 3, "R");
        for (let y = 13; y <= 15; y++) hline(g, 7, 12, y, y === 15 ? "A" : "a");
        if (jewel) {
          put(g, 9, HEAD_TOP, "j");
          put(g, 10, HEAD_TOP, "j");
          put(g, 9, 14, "j");
          put(g, 10, 14, "j");
        }
      }
      break;
    case "mage": {
      // A pointed hat (tall from tier 2, with a star) and, from tier 2, a robe.
      const tall = tier >= 2;
      hline(g, 5, 14, HEAD_TOP + 1, "B");
      hline(g, 7, 12, HEAD_TOP, "b");
      hline(g, 8, 11, HEAD_TOP - 1, "b");
      hline(g, 9, 10, HEAD_TOP - 2, "b");
      if (tall) {
        put(g, 10, HEAD_TOP - 3, "b");
        put(g, 11, HEAD_TOP - 4, "b");
        put(g, 9, HEAD_TOP - 1, "y");
      }
      if (jewel) put(g, 10, HEAD_TOP + 1, "j");
      break;
    }
    case "rogue":
      if (tier === 1) {
        hline(g, 6, 13, HEAD_TOP + 2, "n");
        put(g, 5, HEAD_TOP + 2, "n");
        put(g, 4, HEAD_TOP + 3, "N");
      } else {
        // A hood framing the face, and a dark cloak.
        hline(g, 7, 12, HEAD_TOP - 1, "n");
        for (let y = HEAD_TOP; y <= HEAD_TOP + 1; y++) hline(g, 6, 13, y, "n");
        hline(g, 6, 13, HEAD_TOP + 2, "N");
        for (const x of [5, 6, 13, 14]) for (let y = HEAD_TOP + 3; y <= 11; y++) put(g, x, y, "n");
        for (let y = 13; y <= 16; y++) {
          put(g, 6, y, "n");
          put(g, 13, y, "n");
        }
        if (jewel) put(g, 9, 13, "j");
      }
      break;
    case "cleric":
      // A flower circlet, then a halo; a white robe from tier 2.
      for (let x = 6; x <= 13; x++) put(g, x, HEAD_TOP + 1, (x - 6) % 3 === 1 ? "l" : "g");
      if (tier >= 2) {
        // A ring floating over the head.
        hline(g, 8, 11, 0, "y");
        put(g, 7, 1, "y");
        put(g, 12, 1, "y");
        hline(g, 8, 11, 2, "y");
      }
      if (jewel) put(g, 10, HEAD_TOP + 1, "j");
      break;
    case "cat":
      // Cat ears in the hair colour, a tail, then a bell collar and a crown.
      for (const [x, y] of [[6, 2], [6, 3], [7, 3], [13, 2], [13, 3], [12, 3]] as const) put(g, x, y, "h");
      put(g, 7, 2, "i");
      put(g, 12, 2, "i");
      put(back, 15, 15, "h");
      put(back, 16, 14, "h");
      put(back, 16, 13, "h");
      if (tier >= 2) {
        hline(g, 7, 12, 13, "r");
        put(g, 9, 13, jewel ? "j" : "y");
        put(g, 10, 13, "y");
      }
      if (tier >= 4) {
        hline(g, 8, 11, 1, "y");
        put(g, 8, 0, "y");
        put(g, 10, 0, "y");
        put(g, 11, 0, "y");
      }
      break;
  }
  if (tier >= 5 || (tier >= 4 && cls === "cleric")) {
    for (const [x, y] of [[3, 12], [2, 13], [3, 13], [1, 14], [2, 14], [3, 14], [2, 15], [3, 15]] as const) {
      put(back, x, y, "u");
      put(back, SLIME_W - 1 - x, y, "u");
    }
  }
}

/// Closed eyes over open ones, for the blink overlay.
export function humanBlinkGrid(): string[] {
  const g = blank();
  for (const x of [8, 11]) {
    put(g, x, 8, "s");
    put(g, x, 9, "s");
  }
  for (const x of [7, 8, 11, 12]) put(g, x, 9, "e");
  return rows(g);
}
