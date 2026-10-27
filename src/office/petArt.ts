// The office's pets: a cat, a dog, a rabbit and a chick that wander the
// corridors and the lobby on their own, whatever the agents do. Drawn as
// fill-only grids facing right (the outline is traced around them), 18x12,
// which `traceOutline` grows to the slimes' 20-wide canvas so the walk strips
// slide by the same 20 units. Keys: b body, B shade, w white, p pink,
// e eye, n nose / beak / feet.

import { traceOutline } from "./slimeArt";

export const PET_SPECIES = ["cat", "dog", "rabbit", "chick"] as const;
export type PetSpecies = (typeof PET_SPECIES)[number];

export interface PetFrames {
  walkA: string[];
  walkB: string[];
  sit: string[];
  sleep: string[];
}

const legs = (rows: string[], a: string, b: string): string[] => [...rows.slice(0, 10), a, b];

const CAT_BODY = [
  "............b...b.",
  "............bb.bb.",
  "............bbbbb.",
  "............bebeb.",
  "b...........bbpbb.",
  "b........bbbbbbb..",
  ".b.....bbbbbbbbb..",
  ".bbbbbbbbbbbbbbb..",
  "..bBbbBbbBbbbbbb..",
  "..bbbbbbbbbbbbbb..",
];
const CAT: PetFrames = {
  walkA: legs(CAT_BODY, "..bb..bb...bb..bb.", "..bb..bb...bb..bb."),
  walkB: legs(CAT_BODY, "...bb..bb.bb..bb..", "...bb..bb.bb..bb.."),
  sit: [
    "..........b...b...",
    "..........bb.bb...",
    "..........bbbbb...",
    "..........bebeb...",
    "..........bbpbb...",
    "...........bbb....",
    ".........bbbbbb...",
    "........bbbbbbbb..",
    ".b......bbbwwbbb..",
    ".bb.....bbbwwbbb..",
    "..bbbbbbbbbbbbbb..",
    "........bb..bb....",
  ],
  sleep: [
    "..................",
    "..................",
    "..................",
    "..................",
    "..................",
    "............b..b..",
    "...........bbbbbb.",
    "....bbbbbbbbbbbbb.",
    "...bbBbbBbbbeebeb.",
    "..bbbbbbbbbbbbpbb.",
    "..bbbbbbbbbbbbbb..",
    "...bbbbbbbbbbbb...",
  ],
};

const DOG_BODY = [
  "..................",
  ".............bbb..",
  "............bbbbbb",
  "b..........Bbbebbb",
  "b..........BBbbbbn",
  ".b.........BBbbww.",
  ".b.......bbbbbbb..",
  ".bbbbbbbbbbbbbbb..",
  "..bbbbbbbbbbbbbb..",
  "..bwwwwwwwwwwbbb..",
];
const DOG: PetFrames = {
  walkA: legs(DOG_BODY, "..bb..bb...bb..bb.", "..ww..ww...ww..ww."),
  walkB: legs(DOG_BODY, "...bb..bb.bb..bb..", "...ww..ww.ww..ww.."),
  sit: [
    "..................",
    "...........bbb....",
    "..........bbbbbb..",
    ".........Bbbebbb..",
    ".........BBbbbbn..",
    ".........BBbbww...",
    "..........bbbb....",
    ".........bbbbbb...",
    "........bbwwwbbb..",
    "b.......bbwwwbbb..",
    ".bbbbbbbbbwwwbbb..",
    "........ww...ww...",
  ],
  sleep: [
    "..................",
    "..................",
    "..................",
    "..................",
    "..................",
    "..................",
    "...........bbbb...",
    "..........Bbbbbbb.",
    "...bbbbbbbBBeebbn.",
    "..bbbbbbbbBBbbbww.",
    "..bwwwwwwwwwwbbbb.",
    "...bbbbbbbbbbbbb..",
  ],
};

const RABBIT_BODY = [
  "............b..b..",
  "............bp.bp.",
  "............bp.bp.",
  "...........bbbbbb.",
  "...........bbbebb.",
  "...........bbbbbp.",
  ".....bbbbbbbbbbb..",
  "....bbbbbbbbbbbb..",
  "...wbbbbbbbbbbbb..",
  "....bbbbbbbbbbbb..",
];
const RABBIT: PetFrames = {
  walkA: legs(RABBIT_BODY, ".....bb.....bb....", "....bbb.....bbb..."),
  walkB: legs(RABBIT_BODY, "....bb.......bb...", "...bb.........bb.."),
  sit: [
    "...........b..b...",
    "...........bp.bp..",
    "...........bp.bp..",
    "..........bbbbbb..",
    "..........bbbebb..",
    "..........bbbbbp..",
    "...........bbbb...",
    ".........bbbbbbb..",
    "........bbbwwbbb..",
    ".......wbbbwwbbb..",
    "........bbbbbbbb..",
    "........bbb..bbb..",
  ],
  sleep: [
    "..................",
    "..................",
    "..................",
    "..................",
    "..................",
    "..................",
    "..........bbbbbb..",
    "......bbbbbbbbbbbb",
    ".....bbbbbbbbbbeep",
    "....wbbbbbbbbbbbb.",
    ".....bbbbbbbbbbbb.",
    "......bbbbbbbbbb..",
  ],
};

const CHICK_BODY = [
  "..................",
  "..................",
  "..................",
  "..................",
  "..........bbb.....",
  ".........bbbbb....",
  ".........bbebbn...",
  "...b....bbbbbbn...",
  "....bbbbbbbbbb....",
  ".....bBBBbbbb.....",
];
const CHICK: PetFrames = {
  walkA: legs(CHICK_BODY, "......bbbbbb......", ".......n..n......."),
  walkB: legs(CHICK_BODY, "......bbbbbb......", "......n....n......"),
  sit: [
    "..................",
    "..................",
    "..................",
    "..................",
    "..........bbb.....",
    ".........bbbbb....",
    ".........bbebbn...",
    "...b....bbbbbbn...",
    "....bbbbbbbbbb....",
    ".....bBBBbbbb.....",
    "......bbbbbb......",
    "..................",
  ],
  sleep: [
    "..................",
    "..................",
    "..................",
    "..................",
    "..................",
    "..........bbb.....",
    ".........bbbbb....",
    ".........beebbn...",
    "...b....bbbbbb....",
    "....bbbbbbbbbb....",
    ".....bBBBbbbb.....",
    "......bbbbbb......",
  ],
};

const FILL: Record<PetSpecies, PetFrames> = { cat: CAT, dog: DOG, rabbit: RABBIT, chick: CHICK };

/// A pet's frames, outlined: 20x14 each.
export function petFrames(species: PetSpecies): PetFrames {
  const f = FILL[species];
  return { walkA: traceOutline(f.walkA), walkB: traceOutline(f.walkB), sit: traceOutline(f.sit), sleep: traceOutline(f.sleep) };
}

export const PET_W = 20;
export const PET_H = 14;
