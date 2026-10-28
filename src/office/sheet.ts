// Hi-res sprites on the page: every frame of a character or pet laid side by
// side on one <canvas> (a sprite sheet), inside a box one frame wide that
// crops it. The CSS picks the frame for the pose (`--f`) and slides the sheet
// a frame along for the moving poses, so one animation flips both frames and
// they can never drift apart. Canvas rather than images: the app's CSP
// (`default-src 'self'`) refuses data: URLs, and an SVG <rect> per pixel would
// be thousands of nodes per character.

import { TIER } from "./heroTiers";
import type { GearClass } from "./hiGear";
import { humanFrame, type HumanPose } from "./hiHuman";
import { PET_H, PET_POSES, PET_W, petFrame, type PetSpecies } from "./hiPets";
import { FRAME_H, FRAME_W, slimeFrame, type Jelly, type SlimePose } from "./hiSlime";
import { hexToRgb, type Raster } from "./paint";

/// What a lead wears for its account (`heroModel`).
export interface HeroLook {
  cls: GearClass;
  tier: number;
  /// Drawn as a slime (default) or a human adventurer.
  form?: "slime" | "human";
}

/// The order of a character's frames on its sheet; the stylesheet's `--f`
/// values index into this.
export const CHAR_FRAMES = ["normal", "normalBlink", "raise", "raiseBlink", "sleep", "walkA", "walkB", "typeA", "typeB", "relaxA", "relaxB", "cheerA", "cheerB"] as const;
export type CharFrame = (typeof CHAR_FRAMES)[number];

const JELLIES: Record<string, Jelly> = {
  claude: { body: "#f0905f", eye: "#2a1f1a", blush: "#ff8c8c" },
  codex: { body: "#3d4450", eye: "#66f0a8", blush: "#4d5664" },
  gemini: { body: "#6ea0ff", eye: "#1d2440", blush: "#ffa3c6" },
  opencode: { body: "#9a8cff", eye: "#231a4a", blush: "#ffb0d0" },
  aider: { body: "#4fd1c5", eye: "#123b37", blush: "#ffb0b0" },
  copilot: { body: "#b48cf2", eye: "#2a1745", blush: "#ffb0d0" },
  manager: { body: "#7fd3b4", eye: "#1d2a24", blush: "#ff9fb0" },
  // The office games' visitors: a courier, and the boss of a long task.
  courier: { body: "#c89a5a", eye: "#2a1f1a", blush: "#ff9f8f" },
  boss: { body: "#8a3a8a", eye: "#ffe066", blush: "#c85a9a" },
};
const FALLBACK: Jelly = { body: "#a7b0bb", eye: "#22262b", blush: "#f4a3a0" };

/// The colour a kind wears, for the header legend.
export function paletteOf(kind: string): { shirt: string } {
  return { shirt: (JELLIES[kind] ?? FALLBACK).body };
}

const SLIME_POSES: Record<CharFrame, SlimePose> = {
  normal: { body: "normal", eyes: "open" },
  normalBlink: { body: "normal", eyes: "blink" },
  raise: { body: "stretch", eyes: "open" },
  raiseBlink: { body: "stretch", eyes: "blink" },
  sleep: { body: "melt", eyes: "shut" },
  walkA: { body: "squash", eyes: "open" },
  walkB: { body: "stretch", eyes: "open", lift: 2 },
  typeA: { body: "normal", eyes: "open" },
  typeB: { body: "squash", eyes: "open" },
  relaxA: { body: "normal", eyes: "happy", mug: true },
  relaxB: { body: "squash", eyes: "happy", mug: true },
  cheerA: { body: "stretch", eyes: "happy", lift: 2 },
  cheerB: { body: "squash", eyes: "happy" },
};

const HUMAN_POSES: Record<CharFrame, HumanPose> = {
  normal: { arms: "down", legs: "stand", eyes: "open" },
  normalBlink: { arms: "down", legs: "stand", eyes: "blink" },
  raise: { arms: "raise", legs: "stand", eyes: "open" },
  raiseBlink: { arms: "raise", legs: "stand", eyes: "blink" },
  sleep: { arms: "down", legs: "stand", eyes: "shut" },
  walkA: { arms: "swingA", legs: "stepA", eyes: "open" },
  walkB: { arms: "swingB", legs: "stepB", eyes: "open" },
  typeA: { arms: "type", legs: "stand", eyes: "open" },
  typeB: { arms: "typeUp", legs: "stand", eyes: "open" },
  relaxA: { arms: "mug", legs: "stand", eyes: "happy" },
  relaxB: { arms: "sip", legs: "stand", eyes: "happy" },
  cheerA: { arms: "up", legs: "stand", eyes: "happy" },
  cheerB: { arms: "up", legs: "stand", eyes: "happy" },
};

/// Every frame of a character, in `CHAR_FRAMES` order. `role` "sub" is a
/// smaller slime with a sprout; "manager" (the user) wears a bow tie.
export function characterFrames(kind: string, role: "lead" | "sub" | "manager", hero?: HeroLook): Raster[] {
  const jelly = role === "manager" ? JELLIES.manager : (JELLIES[kind] ?? FALLBACK);
  const lead = role === "lead" ? hero : undefined;
  if (lead?.form === "human") {
    return CHAR_FRAMES.map((f) => {
      const r = humanFrame(jelly, lead.cls, lead.tier, HUMAN_POSES[f]);
      // The top of a cheer: up on the toes.
      return f === "cheerA" ? lifted(r, 2) : r;
    });
  }
  const look = { jelly, role, hero: lead ? { cls: lead.cls, tier: lead.tier } : undefined };
  return CHAR_FRAMES.map((f) => slimeFrame(look, SLIME_POSES[f]));
}

/// `r` moved up `dy` pixels.
function lifted(r: Raster, dy: number): Raster {
  const out = r.clone();
  out.px.fill(null);
  return out.blit(r, 0, -dy);
}

/// Every frame of a pet, in `PET_POSES` order.
export function petFrames(species: PetSpecies): Raster[] {
  return PET_POSES.map((p) => petFrame(species, p));
}

// ── On the page ─────────────────────────────────────────────────────────

/// Sheets already painted, by look: a dozen frames are a few milliseconds of
/// painting, so each look is painted once per session.
const cache = new Map<string, ImageData | null>();

/// jsdom (the tests) has no canvas; asking it for one only logs an error.
const NO_CANVAS = typeof navigator !== "undefined" && /jsdom/i.test(navigator.userAgent);

function imageOf(frames: Raster[]): ImageData | null {
  if (NO_CANVAS || typeof ImageData === "undefined") return null;
  const fw = frames[0].w;
  const fh = frames[0].h;
  const w = fw * frames.length;
  const data = new Uint8ClampedArray(w * fh * 4);
  const rgb = new Map<string, [number, number, number]>();
  frames.forEach((f, i) => {
    for (let y = 0; y < fh; y++)
      for (let x = 0; x < fw; x++) {
        const c = f.px[y * fw + x];
        if (c === null) continue;
        let v = rgb.get(c);
        if (!v) rgb.set(c, (v = hexToRgb(c)));
        const o = (y * w + i * fw + x) * 4;
        data[o] = v[0];
        data[o + 1] = v[1];
        data[o + 2] = v[2];
        data[o + 3] = 255;
      }
  });
  return new ImageData(data, w, fh);
}

/// A sprite box holding the sheet `key`, painting it from `frames()` the
/// first time it is asked for.
function sprite(key: string, fw: number, fh: number, count: number, frames: () => Raster[], cls: string): HTMLElement {
  const box = document.createElement("div");
  box.className = `office-sprite ${cls}`;
  box.setAttribute("aria-hidden", "true");
  const canvas = document.createElement("canvas");
  canvas.className = "px-sheet";
  canvas.width = fw * count;
  canvas.height = fh;
  box.appendChild(canvas);
  if (!NO_CANVAS) {
    if (!cache.has(key)) cache.set(key, imageOf(frames()));
    const img = cache.get(key);
    if (img) canvas.getContext("2d")?.putImageData(img, 0, 0);
  }
  return box;
}

/// A character's sprite: all its frames on one sheet; the CSS shows the one
/// for the pose class on its `.office-char` (`office-char--walking`, …).
export function characterSprite(kind: string, role: "lead" | "sub" | "manager", hero?: HeroLook): HTMLElement {
  const lead = role === "lead" ? hero : undefined;
  const key = `char:${role === "manager" ? "manager" : kind}:${role}:${lead ? `${lead.form ?? "slime"}:${lead.cls}:${Math.min(lead.tier, TIER.LEGEND)}` : ""}`;
  return sprite(key, FRAME_W, FRAME_H, CHAR_FRAMES.length, () => characterFrames(kind, role, hero), "office-sprite--char");
}

/// A pet's sprite: walk (two frames), sit and sleep.
export function petSprite(species: PetSpecies): HTMLElement {
  return sprite(`pet:${species}`, PET_W, PET_H, PET_POSES.length, () => petFrames(species), "office-sprite--pet");
}
