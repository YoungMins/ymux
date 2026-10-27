// The office's slimes at one art pixel per CSS pixel: a 60x66 frame per pose,
// painted from shapes (`paint.ts`) rather than drawn by hand. The body is a
// dome with a flattened base, shaded from the upper left with a rim of
// reflected light along the bottom; the face sits by the body's height. A
// hero's gear (`hiGear.ts`) is painted around the body, part by part, behind
// it or in front.

import { TIER } from "./heroTiers";
import { CAPE, GOLD, LEAF, MAGE_CLOTH, PINK, RED, ROGUE_CLOTH, WHITE, crown, halo, item, jewel, metal, sparkle, wing, type GearClass } from "./hiGear";
import { Raster, capsule, dome, ellipse, flat, intersect, minus, paint, polygon, ramp, rect, slope, union, type Mask, type Ramp } from "./paint";

export const FRAME_W = 60;
export const FRAME_H = 66;

export type Body = "normal" | "squash" | "stretch" | "melt";
export type Eyes = "open" | "shut" | "happy" | "blink";

/// A slime's colours: body, eyes, blush.
export interface Jelly {
  body: string;
  eye: string;
  blush: string;
}

export interface SlimeLook {
  jelly: Jelly;
  role: "lead" | "sub" | "manager";
  hero?: { cls: GearClass; tier: number };
}

export interface SlimePose {
  body: Body;
  eyes: Eyes;
  mug?: boolean;
  /// Lifted off the ground this many pixels (the top of a hop).
  lift?: number;
}

/// Width and height of each body, before a helper's shrink.
const SIZES: Record<Body, [number, number]> = {
  normal: [48, 36],
  squash: [54, 30],
  stretch: [40, 42],
  melt: [56, 20],
};

/// Where the body's base sits in the frame.
const GROUND = 63;
/// Helpers are drawn smaller (in the same frame, so they line up).
const SUB_SCALE = 0.64;

/// A body's box in the frame.
interface Box {
  cx: number;
  cy: number;
  w: number;
  h: number;
  top: number;
  bottom: number;
  /// The size scale (1, or smaller for helpers): gear and face follow it.
  s: number;
}

function box(body: Body, role: SlimeLook["role"], lift: number): Box {
  const s = role === "sub" ? SUB_SCALE : 1;
  const [w0, h0] = SIZES[body];
  const w = Math.round(w0 * s);
  const h = Math.round(h0 * s);
  const bottom = GROUND - lift;
  return { cx: FRAME_W / 2, cy: bottom - h / 2, w, h, top: bottom - h, bottom, s };
}

/// A dome over a flattened base: a plain ellipse above the middle, a squarer
/// curve below it so the slime sits on the floor.
function slimeMask(b: Box, grow = 0): Mask {
  const rx = b.w / 2 + grow;
  const ry = b.h / 2 + grow;
  const m: Mask = (x, y) => {
    const dx = (x + 0.5 - b.cx) / rx;
    const dy = (y + 0.5 - b.cy) / ry;
    if (dy <= 0) return dx * dx + dy * dy <= 1;
    return dx * dx + Math.pow(dy, 4) <= 1;
  };
  m.box = [Math.floor(b.cx - rx) - 1, Math.floor(b.cy - ry) - 1, Math.ceil(b.cx + rx) + 1, Math.ceil(b.cy + ry) + 1];
  return m;
}

function scaled(b: Box, v: number): number {
  return v * b.s;
}

// ── Body and face ────────────────────────────────────────────────────────

function paintBody(out: Raster, b: Box, r: Ramp): void {
  const m = slimeMask(b);
  paint(out, m, r, { light: dome(b.cx - b.w * 0.12, b.cy - b.h * 0.1, b.w * 0.62, b.h * 0.62) });
  // Reflected light along the base: a lighter band one pixel inside the
  // outline, broken up at the sides.
  for (let x = 0; x < out.w; x++) {
    const y = Math.floor(b.bottom) - 2;
    if (m(x, y) && m(x, y + 1) && m(x - 2, y) && m(x + 2, y) && Math.abs(x + 0.5 - b.cx) < b.w * 0.34) out.set(x, y, r.tones[2]);
  }
  // A glossy highlight at the upper left, and a smaller one beside it.
  const hx = b.cx - b.w * 0.24;
  const hy = b.cy - b.h * 0.22;
  paint(out, ellipse(hx, hy, Math.max(1.6, scaled(b, 4.2)), Math.max(1.2, scaled(b, 2.6))), flat(r.tones[4]), { outline: false });
  paint(out, ellipse(hx - scaled(b, 0.6), hy - scaled(b, 0.4), Math.max(1, scaled(b, 2.2)), Math.max(0.8, scaled(b, 1.2))), flat("#ffffff"), { outline: false });
  out.set(hx + scaled(b, 5), hy - scaled(b, 1.5), "#ffffff");
}

/// Where the eyes sit: a little above the body's middle (lower on a melt).
function eyeY(b: Box, body: Body): number {
  return body === "melt" ? b.cy - b.h * 0.05 : b.cy - b.h * 0.04;
}

function paintFace(out: Raster, b: Box, body: Body, eyes: Eyes, jelly: Jelly, r: Ramp): void {
  const ey = eyeY(b, body);
  const gap = body === "stretch" ? b.w * 0.2 : b.w * 0.17;
  const ink = jelly.eye;
  const small = b.s < 1;
  for (const side of [-1, 1] as const) {
    const ex = b.cx + side * gap;
    if (eyes === "open") {
      const rx = small ? 1.6 : 2.6;
      const ry = small ? 2.4 : 3.8;
      paint(out, ellipse(ex, ey, rx, ry), flat(ink), { outline: false });
      // A glint, and a softer second one low on the eye.
      out.set(ex - (small ? 1 : 1), ey - (small ? 1 : 2), "#ffffff");
      if (!small) {
        out.set(ex, ey - 2, "#ffffff");
        out.set(ex - 1, ey - 1, "#ffffff");
        out.set(ex + 1, ey + 2, r.tones[3]);
      }
    } else {
      // Closed: a line (blink), a smile-shaped arc (asleep) or ^ (happy).
      const half = small ? 2 : 3;
      for (let dx = -half; dx <= half; dx++) {
        const t = dx / half;
        const bend = eyes === "blink" ? 0 : Math.round((1 - t * t) * (small ? 1 : 2));
        const y = eyes === "happy" ? ey - bend : eyes === "shut" ? ey + bend - 1 : ey;
        out.set(ex + dx, y, ink);
      }
    }
    // Blush below and outside the eye.
    const bx = ex + side * (small ? 3 : 5);
    const by = ey + (small ? 3 : 5);
    paint(out, ellipse(bx, by, small ? 2 : 3.4, small ? 1 : 1.6), flat(jelly.blush), { outline: false });
  }
  // The mouth: a small open smile, wider when happy, none asleep.
  const my = ey + (small ? 3 : 5);
  if (eyes === "happy") {
    const half = small ? 2 : 3;
    paint(out, intersect(ellipse(b.cx, my, half + 0.5, small ? 2 : 3), rect(0, Math.floor(my), FRAME_W, FRAME_H)), flat(ink), { outline: false });
    if (!small) out.set(b.cx - 0.5, my + 1, jelly.blush), out.set(b.cx + 0.5, my + 1, jelly.blush);
  } else if (eyes !== "shut") {
    for (const dx of small ? [-1, 0] : [-2, -1, 0, 1]) out.set(b.cx + dx, my + (Math.abs(dx + 0.5) > 1 ? 0 : 1), ink);
  } else {
    // Asleep: a tiny "o", breathing.
    out.set(b.cx - 0.5, my + 1, ink);
  }
}

// ── Extras: a helper's sprout, the user's bow tie, a mug ──────────────────

function paintSprout(out: Raster, b: Box): void {
  const x = b.cx;
  const y = b.top + 1;
  paint(out, capsule(x, y, x, y - 6, 0.7), flat("#3f9a3a"), { outline: false });
  paint(out, ellipse(x - 3.5, y - 7, 3.4, 2), LEAF, { light: dome(x - 5, y - 8, 3, 2) });
  paint(out, ellipse(x + 3.5, y - 8, 3.4, 2), LEAF, { light: dome(x + 2, y - 9, 3, 2) });
}

function paintTie(out: Raster, b: Box): void {
  const y = b.cy + b.h * 0.26;
  const x = b.cx;
  paint(out, polygon([[x - 7, y - 4], [x, y - 1], [x, y + 1], [x - 7, y + 4]]), RED, { light: slope(x - 7, y - 4, 7, 8) });
  paint(out, polygon([[x + 7, y - 4], [x, y - 1], [x, y + 1], [x + 7, y + 4]]), RED, { light: slope(x, y - 4, 7, 8) });
  paint(out, rect(x - 1.5, y - 2, 3, 4), RED, { light: slope(x - 2, y - 2, 3, 4) });
}

function paintMug(out: Raster, b: Box): void {
  const x = b.cx + b.w / 2 - 6;
  const y = b.cy + 2;
  paint(out, minus(ellipse(x + 10, y + 5, 3.4, 3.4), ellipse(x + 10, y + 5, 1.2, 1.4)), WHITE);
  paint(out, rect(x, y, 10, 11), WHITE, { light: slope(x, y, 10, 11) });
  paint(out, rect(x + 1, y + 1, 8, 2), flat("#6b3e22"), { outline: false });
  out.set(x + 3, y - 2, "#e8e4dc");
  out.set(x + 4, y - 3, "#e8e4dc");
  out.set(x + 6, y - 3, "#e8e4dc");
  out.set(x + 7, y - 4, "#e8e4dc");
}

// ── Hero gear on a slime ────────────────────────────────────────────────

/// A band around the head at `y` (relative to the top), with a knot whose
/// tails hang off one side.
function headband(out: Raster, b: Box, dy: number, r: Ramp, side: 1 | -1): void {
  const y = b.top + dy;
  paint(out, intersect(slimeMask(b, 0.5), rect(0, y, FRAME_W, 4)), r, { light: slope(b.cx - b.w / 2, y, b.w, 4) });
  const kx = b.cx + side * (b.w / 2 - 1);
  paint(out, capsule(kx, y + 2, kx + side * 6, y + 6, 1.4), r, { light: slope(kx - 6, y, 12, 8) });
  paint(out, capsule(kx, y + 2, kx + side * 7, y + 1, 1.3), r, { light: slope(kx - 6, y, 12, 8) });
}

/// The top `frac` of the body, grown a pixel: helmets and hoods.
function cap(b: Box, frac: number): Mask {
  return intersect(slimeMask(b, 1), rect(0, 0, FRAME_W, b.top + b.h * frac));
}

function paintGearBack(out: Raster, b: Box, tier: number): void {
  if (tier >= TIER.WINGS) {
    wing(out, b.cx - b.w * 0.34, b.top + b.h * 0.42, -1);
    wing(out, b.cx + b.w * 0.34, b.top + b.h * 0.42, 1);
  }
}

function paintGearFront(out: Raster, b: Box, body: Body, cls: GearClass, tier: number, skin: Ramp): void {
  const m = metal(tier);
  switch (cls) {
    case "warrior":
      if (tier === TIER.STARTER) headband(out, b, b.h * 0.18, RED, 1);
      else {
        // A rounded helmet with a rim, a nose guard and a plume.
        const rim = b.top + b.h * 0.38;
        paint(out, cap(b, 0.38), m, { light: dome(b.cx - b.w * 0.15, b.top + 2, b.w * 0.6, b.h * 0.5) });
        paint(out, intersect(slimeMask(b, 1.5), rect(0, rim - 2, FRAME_W, 3)), m, { light: slope(0, rim - 2, FRAME_W, 3) });
        paint(out, rect(b.cx - 1.5, rim - 1, 3, 6), m, { light: slope(b.cx - 2, rim, 3, 6) });
        for (let i = 0; i < 5; i++) out.set(b.cx - b.w * 0.3 + i * 2, b.top + 3 + (i % 2), m.tones[4]);
        paint(out, polygon([[b.cx - 2, b.top + 1], [b.cx + 2, b.top - 7], [b.cx + 9, b.top - 9], [b.cx + 6, b.top - 4], [b.cx + 3, b.top + 1]]), RED, {
          light: slope(b.cx - 2, b.top - 9, 11, 10),
        });
        if (tier >= TIER.JEWEL) jewel(out, b.cx, b.top + b.h * 0.2, 2.2);
      }
      break;
    case "mage": {
      const brimY = b.top + 3;
      if (tier === TIER.STARTER) {
        paint(out, ellipse(b.cx, brimY, b.w * 0.42, 2.6), MAGE_CLOTH, { light: slope(b.cx - b.w * 0.42, brimY - 3, b.w * 0.84, 5) });
        paint(out, polygon([[b.cx - b.w * 0.24, brimY], [b.cx + 2, brimY - 13], [b.cx + b.w * 0.24, brimY]]), MAGE_CLOTH, { light: slope(b.cx - 10, brimY - 13, 20, 13) });
      } else {
        const tall = 21;
        paint(out, ellipse(b.cx, brimY, b.w * 0.52, 3.2), MAGE_CLOTH, { light: slope(b.cx - b.w * 0.5, brimY - 3, b.w, 6) });
        paint(
          out,
          polygon([[b.cx - b.w * 0.3, brimY], [b.cx - 4, brimY - tall + 8], [b.cx + 2, brimY - tall + 1], [b.cx + 11, brimY - tall + 3], [b.cx + 5, brimY - tall + 6], [b.cx + b.w * 0.3, brimY]]),
          MAGE_CLOTH,
          { light: slope(b.cx - 14, brimY - tall, 28, tall) },
        );
        paint(out, polygon([[b.cx - b.w * 0.29, brimY - 1], [b.cx + b.w * 0.29, brimY - 1], [b.cx + b.w * 0.26, brimY - 5], [b.cx - b.w * 0.26, brimY - 5]]), tier >= TIER.GOLD ? GOLD : ramp("#3a4ab8"), {
          light: slope(b.cx - 14, brimY - 5, 28, 4),
        });
        if (tier >= TIER.JEWEL) jewel(out, b.cx, brimY - 3, 2.2);
        sparkle(out, b.cx + 3, brimY - 12, "#fff6b0");
        if (tier >= TIER.LEGEND) for (const [dx, dy] of [[-16, -6], [15, -14], [-9, -17]] as const) sparkle(out, b.cx + dx, brimY + dy);
      }
      break;
    }
    case "rogue":
      if (tier === TIER.STARTER) headband(out, b, b.h * 0.2, ROGUE_CLOTH, -1);
      else {
        // A hood: over the head and down both sides, the face left open.
        const face = ellipse(b.cx, eyeY(b, body) + 1, b.w * 0.34, b.h * 0.3);
        const hood = minus(union(cap(b, 0.42), intersect(slimeMask(b, 1), rect(0, 0, FRAME_W, b.top + b.h * 0.8))), union(face, rect(b.cx - b.w * 0.34, eyeY(b, body), b.w * 0.68, FRAME_H)));
        paint(out, hood, ROGUE_CLOTH, { light: dome(b.cx - b.w * 0.15, b.top + 2, b.w * 0.6, b.h * 0.6) });
        if (tier >= TIER.JEWEL) jewel(out, b.cx, b.top + 4, 2);
      }
      break;
    case "cleric": {
      // A circlet of leaves with flowers, a halo above from the outfit.
      const y = b.top + b.h * 0.2;
      paint(out, intersect(slimeMask(b, 0.5), rect(0, y, FRAME_W, 3)), LEAF, { light: slope(0, y, FRAME_W, 3) });
      for (let x = b.cx - b.w / 2 + 6; x < b.cx + b.w / 2 - 4; x += 7) {
        paint(out, ellipse(x, y + 1, 2.2, 2.2), PINK, { light: dome(x - 1, y, 2, 2) });
        out.set(x - 0.5, y + 0.5, "#fff2a0");
      }
      if (tier >= TIER.JEWEL) jewel(out, b.cx, y + 1, 2.2);
      if (tier >= TIER.OUTFIT) halo(out, b.cx, b.top - 5, 12, tier >= TIER.LEGEND);
      break;
    }
    case "cat": {
      // Ears with pink insides, whiskers, a collar with a bell from the outfit.
      for (const side of [-1, 1] as const) {
        const ex = b.cx + side * b.w * 0.27;
        const ear = polygon([[ex - 6, b.top + 5], [ex + side * 2, b.top - 7], [ex + 6, b.top + 5]]);
        paint(out, ear, skin, { light: slope(ex - 6, b.top - 7, 12, 12) });
        paint(out, polygon([[ex - 3, b.top + 4], [ex + side * 1.5, b.top - 3], [ex + 3, b.top + 4]]), flat("#ff9fb8"), { outline: false });
      }
      const wy = eyeY(b, body) + 4;
      for (const side of [-1, 1] as const)
        for (const [dy, len] of [[-1, 6], [1, 6]] as const)
          for (let i = 0; i < len; i++) out.set(b.cx + side * (b.w / 2 - 4 + i), wy + dy + (i > 3 ? dy : 0), "#2a1f1a");
      if (tier >= TIER.OUTFIT) {
        const cy = b.bottom - b.h * 0.22;
        paint(out, intersect(slimeMask(b, 0.5), rect(0, cy, FRAME_W, 3)), RED, { light: slope(0, cy, FRAME_W, 3) });
        if (tier >= TIER.JEWEL) jewel(out, b.cx, cy + 3, 2.6);
        else paint(out, ellipse(b.cx, cy + 3.5, 2.6, 2.6), GOLD, { light: dome(b.cx - 1, cy + 2, 2.6, 2.6) });
      }
      if (tier >= TIER.CAPE) {
        // A bow by the ear.
        const bx = b.cx - b.w * 0.36;
        const by = b.top + 2;
        const cape = CAPE.cat;
        paint(out, polygon([[bx - 6, by - 4], [bx, by], [bx - 6, by + 4]]), cape, { light: slope(bx - 6, by - 4, 6, 8) });
        paint(out, polygon([[bx + 6, by - 4], [bx, by], [bx + 6, by + 4]]), cape, { light: slope(bx, by - 4, 6, 8) });
        paint(out, ellipse(bx, by, 1.8, 1.8), cape);
      }
      if (tier >= TIER.GOLD && tier < TIER.LEGEND) crown(out, b.cx, b.top + 2, 14);
      break;
    }
  }

  // A sash across the body with a gold buckle (cats have their bow).
  if (tier >= TIER.CAPE && cls !== "cat") {
    const y0 = b.cy + b.h * 0.02;
    const sash = intersect(slimeMask(b), capsule(b.cx - b.w / 2, y0, b.cx + b.w / 2, y0 + b.h * 0.34, 2.6));
    paint(out, sash, CAPE[cls], { light: slope(b.cx - b.w / 2, y0 - 3, b.w, b.h * 0.4) });
    const bx = b.cx + 2;
    const by = y0 + b.h * 0.19;
    paint(out, rect(bx - 3, by - 3, 6, 6), GOLD, { light: slope(bx - 3, by - 3, 6, 6) });
    out.set(bx - 1, by - 1, GOLD.outline);
    out.set(bx, by - 1, GOLD.outline);
  }

  // The class item, held at the right side (the cat's fish low in front).
  if (tier >= TIER.ITEM) {
    if (cls === "cat") item(out, cls, Math.round(b.cx + b.w * 0.12), Math.round(b.bottom - 2), tier);
    else item(out, cls, Math.round(b.cx + b.w / 2 - 3), Math.round(b.bottom - 1), tier);
  }

  // Legendary: a big crown over whatever the head wears (mage stars and the
  // cleric's jewelled halo are drawn with their hats).
  if (tier >= TIER.LEGEND && cls !== "mage" && cls !== "cleric") crown(out, b.cx, b.top + (cls === "warrior" ? 1 : 3), 20, true);
}

/// One frame of a slime.
export function slimeFrame(look: SlimeLook, pose: SlimePose): Raster {
  const out = new Raster(FRAME_W, FRAME_H);
  const b = box(pose.body, look.role, pose.lift ?? 0);
  const r = ramp(look.jelly.body);
  const hero = look.role === "lead" ? look.hero : undefined;
  if (hero && hero.tier > 0) paintGearBack(out, b, hero.tier);
  paintBody(out, b, r);
  paintFace(out, b, pose.body, pose.eyes, look.jelly, r);
  if (look.role === "sub") paintSprout(out, b);
  if (look.role === "manager") paintTie(out, b);
  if (hero && hero.tier > 0) paintGearFront(out, b, pose.body, hero.cls, hero.tier, r);
  if (pose.mug) paintMug(out, b);
  return out;
}
