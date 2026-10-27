// Human heroes at one art pixel per CSS pixel: a chibi adventurer in the
// slimes' 60x66 frame, painted part by part (`paint.ts`): legs, cape, torso,
// head, hair, face, then arms over the body. The class outfit grows with the
// tiers like the slimes' gear (`heroTiers`): starter gear, the full outfit,
// a jewel, the class item in hand, a cape, gold, wings, an aura (CSS) and a
// legendary crown.

import { TIER } from "./heroTiers";
import { CAPE, GOLD, LEAF, MAGE_CLOTH, PINK, RED, ROGUE_CLOTH, WHITE, crown, halo, item, jewel, metal, sparkle, wing, type GearClass } from "./hiGear";
import { FRAME_H, FRAME_W, type Jelly } from "./hiSlime";
import { Raster, adjust, capsule, dome, ellipse, flat, intersect, minus, paint, polygon, ramp, rect, slope, union, type Light, type Ramp } from "./paint";

export type Arms = "down" | "raise" | "type" | "typeUp" | "up" | "mug" | "sip" | "swingA" | "swingB";
export type Legs = "stand" | "stepA" | "stepB";
export type HumanEyes = "open" | "shut" | "happy" | "blink";

export interface HumanPose {
  arms: Arms;
  legs: Legs;
  eyes: HumanEyes;
}

const SKIN = ramp("#ffdcc0", "#7a4a32");
/// Faces are lit softly: mostly the middle and light tones.
const soft = (l: Light): Light => (x, y) => 0.5 + 0.5 * l(x, y);
const TROUSERS = ramp("#4a5a7a", "#1e2638");
const SHOES = ramp("#6a4024", "#2a160a");

/// A tunic by class (plain clothes before the first level).
const TUNICS: Record<GearClass | "plain", Ramp> = {
  plain: ramp("#8aa36a", "#2e3e1e"),
  warrior: ramp("#b84a3a", "#40140e"),
  mage: MAGE_CLOTH,
  rogue: ROGUE_CLOTH,
  cleric: ramp("#ece8dc", "#5a5448"),
  cat: ramp("#f0a0b8", "#6a2a40"),
};

const CX = FRAME_W / 2;
const GROUND = 63;
const HEAD_CY = 25;
const HEAD_RX = 12.5;
const HEAD_RY = 12;
const HEAD_TOP = HEAD_CY - HEAD_RY;
const SHOULDER_Y = 39;
const HIP_Y = 53;
const EYE_Y = 28;

/// Robes reach down over the legs.
function robed(cls: GearClass, tier: number): boolean {
  return tier >= TIER.OUTFIT && (cls === "mage" || cls === "cleric");
}

/// Where each hand is, by arm pose: [left, right].
function hands(arms: Arms): [[number, number], [number, number]] {
  switch (arms) {
    case "raise":
      return [[CX - 12, 51], [CX + 13, 25]];
    case "up":
      return [[CX - 14, 26], [CX + 14, 26]];
    case "type":
      return [[CX - 7, 50], [CX + 7, 51]];
    case "typeUp":
      return [[CX - 7, 48], [CX + 7, 47]];
    case "mug":
      return [[CX - 12, 51], [CX + 8, 47]];
    case "sip":
      return [[CX - 12, 51], [CX + 5, 38]];
    case "swingA":
      return [[CX - 13, 48], [CX + 11, 52]];
    case "swingB":
      return [[CX - 11, 52], [CX + 13, 48]];
    case "down":
    default:
      return [[CX - 12, 52], [CX + 12, 52]];
  }
}

function paintLegs(out: Raster, legs: Legs): void {
  for (const side of [-1, 1] as const) {
    const x = CX + side * 4;
    const lift = (legs === "stepA" && side < 0) || (legs === "stepB" && side > 0) ? 2 : 0;
    const foot = GROUND - 2 - lift;
    paint(out, capsule(x, HIP_Y - 2, x, foot - 1, 2.8), TROUSERS, { light: slope(x - 3, HIP_Y, 6, foot - HIP_Y) });
    paint(out, ellipse(x + side * 0.8, foot + 0.5, 3.8, 2.4), SHOES, { light: dome(x - 1, foot - 1, 4, 3) });
  }
}

function paintCape(out: Raster, cls: GearClass): void {
  const c = CAPE[cls];
  paint(out, polygon([[CX - 9, SHOULDER_Y - 1], [CX + 9, SHOULDER_Y - 1], [CX + 15, GROUND - 3], [CX + 8, GROUND - 1], [CX, GROUND - 3], [CX - 8, GROUND - 1], [CX - 15, GROUND - 3]]), c, {
    light: slope(CX - 15, SHOULDER_Y, 30, GROUND - SHOULDER_Y),
  });
}

function paintTorso(out: Raster, cls: GearClass, tier: number): void {
  const t = TUNICS[tier >= TIER.STARTER ? cls : "plain"];
  const robe = robed(cls, tier);
  const bottom = robe ? GROUND - 2 : HIP_Y + 2;
  const flare = robe ? 13 : 10;
  const body = polygon([[CX - 8, SHOULDER_Y - 2], [CX + 8, SHOULDER_Y - 2], [CX + flare, bottom], [CX - flare, bottom]]);
  paint(out, body, t, { light: slope(CX - flare, SHOULDER_Y, flare * 2, bottom - SHOULDER_Y) });
  // A belt with a buckle (over a robe, a sash-like cord).
  const by = HIP_Y - 3;
  paint(out, intersect(body, rect(0, by, FRAME_W, 3)), robe ? GOLD : ramp("#6a4024", "#2a160a"), { light: slope(0, by, FRAME_W, 3) });
  paint(out, rect(CX - 2, by - 0.5, 4, 4), GOLD, { light: slope(CX - 2, by, 4, 4) });
  // A collar.
  paint(out, polygon([[CX - 5, SHOULDER_Y - 2], [CX + 5, SHOULDER_Y - 2], [CX, SHOULDER_Y + 3]]), flat(t.tones[0]), { outline: false });
  if (cls === "warrior" && tier >= TIER.OUTFIT) {
    // A breastplate and shoulder pads.
    const m = metal(tier);
    paint(out, polygon([[CX - 7, SHOULDER_Y], [CX + 7, SHOULDER_Y], [CX + 6, by], [CX - 6, by]]), m, { light: dome(CX - 3, SHOULDER_Y + 2, 9, 9) });
    for (const side of [-1, 1] as const) paint(out, ellipse(CX + side * 9, SHOULDER_Y + 1, 4.2, 3.2), m, { light: dome(CX + side * 9 - 1, SHOULDER_Y, 4, 3) });
    if (tier >= TIER.JEWEL) jewel(out, CX, SHOULDER_Y + 5, 2.2);
  }
}

function paintHead(out: Raster, hair: Ramp, cls: GearClass, tier: number): void {
  const head = ellipse(CX, HEAD_CY, HEAD_RX, HEAD_RY);
  // Hair at the back, behind the face, reaching the jaw.
  paint(out, ellipse(CX, HEAD_CY + 1, HEAD_RX + 1.5, HEAD_RY + 1), hair, { light: dome(CX - 4, HEAD_TOP + 3, HEAD_RX + 2, HEAD_RY + 2) });
  paint(out, head, SKIN, { light: soft(dome(CX - 4, HEAD_CY - 3, HEAD_RX + 2, HEAD_RY + 2)) });
  // Ears.
  for (const side of [-1, 1] as const) paint(out, ellipse(CX + side * (HEAD_RX - 0.5), EYE_Y + 1, 2.2, 3), SKIN, { light: dome(CX + side * HEAD_RX, EYE_Y, 2, 3) });
  // Hair on top: a cap over the forehead cut into a fringe.
  const fringe = minus(
    intersect(ellipse(CX, HEAD_CY - 1, HEAD_RX + 1.2, HEAD_RY + 0.5), rect(0, 0, FRAME_W, EYE_Y - 4)),
    union(polygon([[CX - 6, EYE_Y - 4], [CX - 3, EYE_Y - 9], [CX, EYE_Y - 4]]), polygon([[CX + 1, EYE_Y - 4], [CX + 5, EYE_Y - 10], [CX + 8, EYE_Y - 4]])),
  );
  const hooded = cls === "rogue" && tier >= TIER.OUTFIT;
  if (!hooded) paint(out, fringe, hair, { light: dome(CX - 4, HEAD_TOP + 2, HEAD_RX + 2, HEAD_RY) });
  // A shine on the hair.
  if (!hooded) for (let i = 0; i < 5; i++) out.set(CX - 8 + i, HEAD_TOP + 4 - (i > 1 && i < 4 ? 1 : 0), hair.tones[4]);
}

function paintFace(out: Raster, eyes: HumanEyes, jelly: Jelly): void {
  const ink = "#2a1f1a";
  for (const side of [-1, 1] as const) {
    const ex = CX + side * 5;
    if (eyes === "open") {
      paint(out, ellipse(ex, EYE_Y, 1.8, 2.8), flat(ink), { outline: false });
      // An iris in the agent's colour, and a glint.
      out.set(ex - 0.5, EYE_Y + 1, jelly.body);
      out.set(ex + 0.5, EYE_Y + 1, jelly.body);
      out.set(ex - 1, EYE_Y - 1.5, "#ffffff");
    } else {
      for (let dx = -2; dx <= 2; dx++) {
        const bend = eyes === "blink" ? 0 : dx === 0 ? 1 : Math.abs(dx) === 1 ? 1 : 0;
        out.set(ex + dx, eyes === "happy" ? EYE_Y - bend : eyes === "shut" ? EYE_Y + bend - 1 : EYE_Y, ink);
      }
    }
    paint(out, ellipse(CX + side * 8, EYE_Y + 4, 2.4, 1.2), flat("#ffa0a0"), { outline: false });
  }
  const my = EYE_Y + 5;
  if (eyes === "happy") {
    paint(out, intersect(ellipse(CX, my, 2.5, 2.2), rect(0, my, FRAME_W, 4)), flat("#b0503a"), { outline: false });
  } else if (eyes !== "shut") {
    out.set(CX - 1, my + 1, "#b0503a");
    out.set(CX, my + 1, "#b0503a");
  }
}

/// An arm from the shoulder to `hand`, in the sleeve colour, with a hand.
function paintArm(out: Raster, side: -1 | 1, hand: [number, number], sleeve: Ramp): void {
  const sx = CX + side * 8;
  const sy = SHOULDER_Y + 1;
  paint(out, capsule(sx, sy, hand[0], hand[1], 2.6), sleeve, { light: slope(Math.min(sx, hand[0]) - 3, Math.min(sy, hand[1]) - 3, 8, 16) });
  paint(out, ellipse(hand[0], hand[1], 2.6, 2.6), SKIN, { light: dome(hand[0] - 1, hand[1] - 1, 3, 3) });
}

function paintMug(out: Raster, at: [number, number]): void {
  const [x, y] = at;
  paint(out, minus(ellipse(x + 5, y, 2.6, 2.6), ellipse(x + 5, y, 1, 1)), WHITE);
  paint(out, rect(x - 3, y - 4, 7, 8), WHITE, { light: slope(x - 3, y - 4, 7, 8) });
  paint(out, rect(x - 2, y - 3, 5, 1), flat("#6b3e22"), { outline: false });
}

/// The head gear, drawn after the face.
function paintHeadGear(out: Raster, cls: GearClass, tier: number, hair: Ramp): void {
  const m = metal(tier);
  switch (cls) {
    case "warrior":
      if (tier === TIER.STARTER) {
        const y = EYE_Y - 7;
        paint(out, intersect(ellipse(CX, HEAD_CY, HEAD_RX + 1.5, HEAD_RY + 1), rect(0, y, FRAME_W, 3)), RED, { light: slope(0, y, FRAME_W, 3) });
        paint(out, capsule(CX + HEAD_RX, y + 1, CX + HEAD_RX + 6, y + 5, 1.3), RED);
      } else {
        // A helmet down to the brow, with cheek guards and a plume.
        const helm = intersect(ellipse(CX, HEAD_CY, HEAD_RX + 2, HEAD_RY + 1.5), rect(0, 0, FRAME_W, EYE_Y - 4));
        paint(out, helm, m, { light: dome(CX - 4, HEAD_TOP, HEAD_RX + 2, HEAD_RY) });
        paint(out, intersect(ellipse(CX, HEAD_CY, HEAD_RX + 2.5, HEAD_RY + 2), rect(0, EYE_Y - 6, FRAME_W, 2)), m, { light: slope(0, EYE_Y - 6, FRAME_W, 2) });
        for (const side of [-1, 1] as const) paint(out, rect(CX + side * (HEAD_RX + 0.5) - 1.5, EYE_Y - 5, 3, 9), m, { light: slope(CX + side * HEAD_RX - 2, EYE_Y - 5, 3, 9) });
        paint(out, polygon([[CX - 2, HEAD_TOP + 1], [CX + 1, HEAD_TOP - 7], [CX + 9, HEAD_TOP - 9], [CX + 6, HEAD_TOP - 3], [CX + 3, HEAD_TOP + 1]]), RED, {
          light: slope(CX - 2, HEAD_TOP - 9, 11, 10),
        });
        if (tier >= TIER.JEWEL) jewel(out, CX, EYE_Y - 8, 2.2);
      }
      break;
    case "mage": {
      const brim = HEAD_TOP + 5;
      if (tier === TIER.STARTER) {
        paint(out, ellipse(CX, brim, 14, 2.6), MAGE_CLOTH, { light: slope(CX - 14, brim - 3, 28, 5) });
        paint(out, polygon([[CX - 8, brim], [CX + 2, brim - 11], [CX + 8, brim]]), MAGE_CLOTH, { light: slope(CX - 8, brim - 11, 16, 11) });
      } else {
        paint(out, ellipse(CX, brim, 17, 3.2), MAGE_CLOTH, { light: slope(CX - 17, brim - 3, 34, 6) });
        paint(out, polygon([[CX - 10, brim], [CX - 4, brim - 9], [CX + 2, brim - 16], [CX + 11, brim - 14], [CX + 5, brim - 10], [CX + 10, brim]]), MAGE_CLOTH, {
          light: slope(CX - 10, brim - 16, 21, 16),
        });
        paint(out, polygon([[CX - 10, brim - 1], [CX + 10, brim - 1], [CX + 9, brim - 4], [CX - 9, brim - 4]]), tier >= TIER.GOLD ? GOLD : ramp("#3a4ab8"), {
          light: slope(CX - 10, brim - 4, 20, 3),
        });
        if (tier >= TIER.JEWEL) jewel(out, CX, brim - 2.5, 2);
        sparkle(out, CX + 2, brim - 9);
        if (tier >= TIER.LEGEND) for (const [dx, dy] of [[-16, -4], [16, -12], [-10, -14]] as const) sparkle(out, CX + dx, brim + dy);
      }
      break;
    }
    case "rogue":
      if (tier === TIER.STARTER) {
        const y = HEAD_TOP + 3;
        paint(out, intersect(ellipse(CX, HEAD_CY, HEAD_RX + 1.5, HEAD_RY + 1), rect(0, 0, FRAME_W, y + 4)), ROGUE_CLOTH, { light: dome(CX - 4, HEAD_TOP, HEAD_RX, 8) });
        paint(out, capsule(CX - HEAD_RX, y + 3, CX - HEAD_RX - 6, y + 7, 1.3), ROGUE_CLOTH);
      } else {
        // A hood framing the face, a scarf over the chin.
        const hood = minus(ellipse(CX, HEAD_CY, HEAD_RX + 3, HEAD_RY + 2.5), ellipse(CX, EYE_Y + 1, HEAD_RX - 3, HEAD_RY - 4));
        paint(out, hood, ROGUE_CLOTH, { light: dome(CX - 4, HEAD_TOP, HEAD_RX + 3, HEAD_RY + 2) });
        // Hair peeking out under the hood.
        paint(out, intersect(ellipse(CX, EYE_Y + 1, HEAD_RX - 3, HEAD_RY - 4), rect(0, 0, FRAME_W, EYE_Y - 4)), hair, { outline: false, light: slope(0, EYE_Y - 9, FRAME_W, 5) });
        if (tier >= TIER.JEWEL) jewel(out, CX, HEAD_TOP + 1, 2);
      }
      break;
    case "cleric": {
      const y = EYE_Y - 8;
      paint(out, intersect(ellipse(CX, HEAD_CY, HEAD_RX + 1.5, HEAD_RY + 1), rect(0, y, FRAME_W, 3)), LEAF, { light: slope(0, y, FRAME_W, 3) });
      for (let x = CX - 10; x <= CX + 10; x += 5) {
        paint(out, ellipse(x, y + 1, 2, 2), PINK, { light: dome(x - 1, y, 2, 2) });
        out.set(x - 0.5, y + 0.5, "#fff2a0");
      }
      if (tier >= TIER.JEWEL) jewel(out, CX, y + 1, 2);
      if (tier >= TIER.OUTFIT) halo(out, CX, HEAD_TOP - 4, 11, tier >= TIER.LEGEND);
      break;
    }
    case "cat": {
      for (const side of [-1, 1] as const) {
        const ex = CX + side * 8;
        paint(out, polygon([[ex - 5, HEAD_TOP + 5], [ex + side * 2, HEAD_TOP - 6], [ex + 5, HEAD_TOP + 5]]), hair, { light: slope(ex - 5, HEAD_TOP - 6, 10, 11) });
        paint(out, polygon([[ex - 2.5, HEAD_TOP + 4], [ex + side * 1.5, HEAD_TOP - 2], [ex + 2.5, HEAD_TOP + 4]]), flat("#ff9fb8"), { outline: false });
      }
      for (const side of [-1, 1] as const) for (const dy of [-1, 1]) for (let i = 0; i < 4; i++) out.set(CX + side * (HEAD_RX - 1 + i), EYE_Y + 3 + dy + (i > 2 ? dy : 0), "#2a1f1a");
      if (tier >= TIER.CAPE) {
        const bx = CX - 10;
        const by = HEAD_TOP + 3;
        paint(out, polygon([[bx - 5, by - 3], [bx, by], [bx - 5, by + 3]]), CAPE.cat);
        paint(out, polygon([[bx + 5, by - 3], [bx, by], [bx + 5, by + 3]]), CAPE.cat);
        paint(out, ellipse(bx, by, 1.6, 1.6), CAPE.cat);
      }
      if (tier >= TIER.GOLD && tier < TIER.LEGEND) crown(out, CX, HEAD_TOP + 2, 12);
      break;
    }
  }
  if (tier >= TIER.LEGEND && cls !== "mage" && cls !== "cleric") crown(out, CX, HEAD_TOP + (cls === "warrior" ? 0 : 2), 18, true);
}

/// The cat's collar and tail.
function paintCatBits(out: Raster, tier: number): void {
  if (tier >= TIER.OUTFIT) {
    paint(out, rect(CX - 6, SHOULDER_Y - 3, 12, 3), RED, { light: slope(CX - 6, SHOULDER_Y - 3, 12, 3) });
    if (tier >= TIER.JEWEL) jewel(out, CX, SHOULDER_Y + 1, 2.2);
    else paint(out, ellipse(CX, SHOULDER_Y + 1, 2.2, 2.2), GOLD, { light: dome(CX - 1, SHOULDER_Y, 2, 2) });
  }
}

/// One frame of a human hero.
export function humanFrame(jelly: Jelly, cls: GearClass, tier: number, pose: HumanPose): Raster {
  const out = new Raster(FRAME_W, FRAME_H);
  // Hair a shade darker than the agent colour, so the face stands out.
  const hair = ramp(adjust(jelly.body, -0.06, 0.04));
  const sleeve = TUNICS[tier >= TIER.STARTER ? cls : "plain"];
  const [left, right] = hands(pose.arms);
  if (tier >= TIER.WINGS) {
    wing(out, CX - 7, SHOULDER_Y + 2, -1);
    wing(out, CX + 7, SHOULDER_Y + 2, 1);
  }
  if (tier >= TIER.CAPE && cls !== "cat") paintCape(out, cls);
  if (cls === "cat" && tier >= TIER.STARTER) {
    // A tail behind, curling up.
    paint(out, union(capsule(CX + 6, HIP_Y - 1, CX + 15, HIP_Y - 4, 2), capsule(CX + 15, HIP_Y - 4, CX + 17, HIP_Y - 12, 2)), hair, { light: slope(CX + 4, HIP_Y - 14, 15, 14) });
  }
  paintLegs(out, robed(cls, tier) ? "stand" : pose.legs);
  paintTorso(out, cls, tier);
  if (cls === "cat") paintCatBits(out, tier);
  paintHead(out, hair, cls, tier);
  paintFace(out, pose.eyes, jelly);
  if (tier > 0) paintHeadGear(out, cls, tier, hair);
  paintArm(out, -1, left, sleeve);
  // The class item in the right hand (the cat's fish too), drawn before the
  // hand so the fingers close over it.
  if (tier >= TIER.ITEM && pose.arms !== "mug" && pose.arms !== "sip" && pose.arms !== "type" && pose.arms !== "typeUp") {
    const foot = cls === "warrior" || cls === "rogue" ? right[1] + 4 : cls === "cat" ? right[1] + 2 : right[1] + 9;
    item(out, cls, right[0], foot, tier);
  }
  paintArm(out, 1, right, sleeve);
  if (pose.arms === "mug" || pose.arms === "sip") paintMug(out, [right[0] + 1, right[1] - 2]);
  return out;
}
