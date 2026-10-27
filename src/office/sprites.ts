// Pixel-art SVG for the office view. The grids live in `pixelArt.ts`; this
// turns them into inline SVG (no external assets, so the app's CSP is happy)
// with `shape-rendering="crispEdges"`, one <rect> per run of equal pixels.
// A character is a stack of pose groups (`px-still-…`, `px-anim-…`) that the
// CSS shows one of at a time.

import type { PaneKind } from "../types";
import * as art from "./pixelArt";
import { humanBlinkGrid, humanGrid, type HumanPose } from "./humanArt";
import { TIER } from "./heroTiers";
import { PROPS, THEMES, type Palette, type Piece, type ThemeId } from "./themes";
import { MUG, SLIME_H, SLIME_W, blinkGrid, bodyGrid, eyesGrid, gearGrid, sproutGrid, tieGrid, type Body, type Eyes, type GearClass } from "./slimeArt";


/// Shared colours. A value starting with "." is a CSS class instead of a
/// fill, for pixels the stylesheet colours (a desk's screen follows its
/// pane's status).
const BASE: Palette = {
  o: "#2a1f1a",
  u: "#f4efe6",
  U: "#cfc6b6",
  r: "#d64545",
  R: "#9e2b2b",
  g: "#7ad35a",
  G: "#3f9a3a",
};

interface Jelly {
  /// Body, shade, shine, eyes, blush, mouth.
  c: string;
  C: string;
  w: string;
  e: string;
  k: string;
  m: string;
}

/// Hero gear: steel, mage cloth, rogue cloth, gold, jewel, flowers, ears.
const GEAR: Palette = {
  a: "#d5dbe4",
  A: "#8a94a3",
  b: "#5a6fe0",
  B: "#33449e",
  n: "#5b4f8a",
  N: "#8a7cc4",
  y: "#f5c542",
  Y: "#c8962a",
  j: "#4fe0e8",
  l: "#ff8fc0",
  i: "#ffb3c6",
  d: "#8a5a34",
  D: "#6a4024",
};
/// From the gold tier: the metal and trim turn gold.
const GOLD: Palette = { a: "#f5c542", A: "#c8962a", B: "#c8962a", N: "#c8962a" };

/// The sash (slimes) or cape (humans) colour of each class.
const CAPES: Record<GearClass, string> = {
  warrior: "#c4402f",
  mage: "#7a4fd0",
  rogue: "#2f2a40",
  cleric: "#f5e6a8",
  cat: "#ff8fc0",
};

/// The gear colours for `hero`: gold from its tier, its class's cape.
function heroPalette(hero: HeroLook): Palette {
  return { ...GEAR, ...(hero.tier >= TIER.GOLD ? GOLD : {}), v: CAPES[hero.cls] };
}

/// What a lead wears for its account (`heroModel`).
export interface HeroLook {
  cls: GearClass;
  tier: number;
  /// Drawn as a slime (default) or a human adventurer.
  form?: "slime" | "human";
}

/// A human's tunic by class (plain clothes before the first level).
const TUNICS: Record<GearClass | "plain", { t: string; T: string }> = {
  plain: { t: "#8aa36a", T: "#6a8350" },
  warrior: { t: "#b84a3a", T: "#8a3328" },
  mage: { t: "#5a6fe0", T: "#33449e" },
  rogue: { t: "#5b4f8a", T: "#3e3566" },
  cleric: { t: "#f0ede4", T: "#c9c2b0" },
  cat: { t: "#f0a0b8", T: "#c9788f" },
};

/// A human adventurer: hair in its agent's colour, the tunic of its class.
function humanSvg(kind: string, hero: HeroLook): string {
  const jelly = JELLIES[kind] ?? FALLBACK;
  const tunic = TUNICS[hero.tier >= 1 ? hero.cls : "plain"];
  const pal: Palette = {
    ...BASE,
    ...heroPalette(hero),
    s: "#ffd9b8",
    S: "#e8b48e",
    h: jelly.c,
    H: jelly.C,
    t: tunic.t,
    T: tunic.T,
    q: "#4a5a7a",
    d: "#6a4024",
    e: "#2a1f1a",
    k: "#ff9f9f",
    m: "#b0503a",
  };
  const frame = (pose: HumanPose, blink = false): string => {
    const g = humanGrid(pose, hero.cls, hero.tier);
    return rects(g.back, pal) + rects(g.front, pal) + (blink ? `<g class="px-blink">${rects(humanBlinkGrid(), pal)}</g>` : "");
  };
  const strip = (cls: string, a: string, b: string, dy = 0): string =>
    `<g class="px-pose px-anim ${cls}">${a}<g transform="translate(${SLIME_W} ${-dy})">${b}</g></g>`;
  const still = (cls: string, f: string): string => `<g class="px-pose ${cls}">${f}</g>`;
  const body = [
    still("px-still-normal", frame({ arms: "down", legs: "stand", eyes: "open" }, true)),
    still("px-still-raise", frame({ arms: "raise", legs: "stand", eyes: "open" }, true)),
    still("px-still-sleep", frame({ arms: "down", legs: "stand", eyes: "shut" })),
    strip("px-anim-walk", frame({ arms: "swingA", legs: "stepA", eyes: "open" }), frame({ arms: "swingB", legs: "stepB", eyes: "open" })),
    strip("px-anim-type", frame({ arms: "type", legs: "stand", eyes: "open" }), frame({ arms: "typeUp", legs: "stand", eyes: "open" })),
    strip("px-anim-relax", frame({ arms: "mug", legs: "stand", eyes: "happy" }), frame({ arms: "sip", legs: "stand", eyes: "happy" })),
    strip("px-anim-cheer", frame({ arms: "up", legs: "stand", eyes: "happy" }), frame({ arms: "up", legs: "stand", eyes: "happy" }), 2),
  ].join("");
  return svg(SLIME_W, SLIME_H, PX, body, "office-sprite");
}

const JELLIES: Record<string, Jelly> = {
  claude: { c: "#f0905f", C: "#c9673c", w: "#ffe4d2", e: "#2a1f1a", k: "#ff8c8c", m: "#7a2e1a" },
  codex: { c: "#3d4450", C: "#262b33", w: "#8f9aa8", e: "#66f0a8", k: "#4d5664", m: "#66f0a8" },
  gemini: { c: "#6ea0ff", C: "#4775d1", w: "#e2ecff", e: "#1d2440", k: "#ffa3c6", m: "#1d2440" },
  opencode: { c: "#9a8cff", C: "#6f60d9", w: "#ece8ff", e: "#231a4a", k: "#ffb0d0", m: "#231a4a" },
  aider: { c: "#4fd1c5", C: "#2a9d92", w: "#e0fffb", e: "#123b37", k: "#ffb0b0", m: "#123b37" },
  copilot: { c: "#b48cf2", C: "#8a5fd1", w: "#f3eaff", e: "#2a1745", k: "#ffb0d0", m: "#2a1745" },
  manager: { c: "#7fd3b4", C: "#4fa58a", w: "#e6fff6", e: "#1d2a24", k: "#ff9fb0", m: "#1d2a24" },
};
const FALLBACK: Jelly = { c: "#a7b0bb", C: "#7d8794", w: "#f0f3f6", e: "#22262b", k: "#f4a3a0", m: "#22262b" };

/// The colour a kind wears, for the header legend.
export function paletteOf(kind: string): { shirt: string } {
  return { shirt: (JELLIES[kind] ?? FALLBACK).c };
}

const FIXTURES: Palette = {
  ...PROPS,
  w: "#e9e2d2",
  G: "#5bbf5b",
  B: "#ffffff",
  r: "#d64545",
  R: "#a83232",
  y: "#f5c542",
  P: "#6b5b4b",
  k: "#8a94a3",
  q: "#c89a6a",
  Q: "#a9774a",
};
const LETTER: Palette = { ...FIXTURES, w: "#ffffff" };
const ICON: Palette = { ...PROPS, w: "#ffffff", k: "#8a94a3", g: "#3aa55a", r: "#d64545" };

/// The <rect>s of one grid, merging horizontal runs of the same key.
function rects(rows: readonly string[], pal: Palette): string {
  let out = "";
  rows.forEach((row, y) => {
    let x = 0;
    while (x < row.length) {
      const key = row[x];
      let end = x + 1;
      while (end < row.length && row[end] === key) end++;
      const colour = pal[key];
      if (key !== "." && colour) {
        const paint = colour.startsWith(".") ? `class="${colour.slice(1)}"` : `fill="${colour}"`;
        out += `<rect x="${x}" y="${y}" width="${end - x}" height="1" ${paint}/>`;
      }
      x = end;
    }
  });
  return out;
}

function svg(w: number, h: number, scale: number, body: string, cls: string): string {
  return `<svg class="${cls}" viewBox="0 0 ${w} ${h}" width="${w * scale}" height="${h * scale}" shape-rendering="crispEdges" aria-hidden="true">${body}</svg>`;
}

function width(rows: readonly string[]): number {
  return rows.reduce((m, r) => Math.max(m, r.length), 0);
}

/// Art pixels are drawn this many CSS pixels wide (before the view's scale).
export const PX = 3;

/// A slime, as one group per pose; the CSS shows the group for the pose.
/// Still poses are one frame (`px-still-…`); moving ones (`px-anim-…`) are
/// a strip of two frames side by side that a single animation slides
/// across, so the frames can never drift out of step. `role` "sub" grows a
/// sprout; "manager" (the user) wears a bow tie.
export function characterSvg(kind: string, role: "lead" | "sub" | "manager", hero?: HeroLook): string {
  if (hero && role === "lead" && hero.form === "human") return humanSvg(kind, hero);
  const jelly = role === "manager" ? JELLIES.manager : (JELLIES[kind] ?? FALLBACK);
  const pal: Palette = { ...BASE, ...(hero ? heroPalette(hero) : GEAR), ...jelly };
  const gear = (body: Body) => (hero && role === "lead" ? gearGrid(body, hero.cls, hero.tier) : null);
  const frame = (body: Body, eyes: Eyes, opts: { mug?: boolean; blink?: boolean } = {}): string => {
    const g = gear(body);
    return (
      (g ? rects(g.back, pal) : "") +
      rects(bodyGrid(body), pal) +
      rects(eyesGrid(body, eyes), pal) +
      (role === "sub" ? rects(sproutGrid(body), pal) : "") +
      (role === "manager" ? rects(tieGrid(body), pal) : "") +
      (opts.mug ? rects(MUG, pal) : "") +
      (opts.blink ? `<g class="px-blink">${rects(blinkGrid(body), pal)}</g>` : "") +
      (g ? rects(g.front, pal) : "")
    );
  };
  // Second frame of a strip, one canvas to the right (`dy` lifts it: the
  // top of a hop).
  const strip = (cls: string, a: string, b: string, dy = 0): string =>
    `<g class="px-pose px-anim ${cls}">${a}<g transform="translate(${SLIME_W} ${-dy})">${b}</g></g>`;
  const still = (cls: string, f: string): string => `<g class="px-pose ${cls}">${f}</g>`;
  const body = [
    still("px-still-normal", frame("normal", "open", { blink: true })),
    still("px-still-raise", frame("stretch", "open", { blink: true })),
    still("px-still-sleep", frame("melt", "shut")),
    strip("px-anim-walk", frame("squash", "open"), frame("stretch", "open"), 2),
    strip("px-anim-type", frame("normal", "open"), frame("squash", "open")),
    strip("px-anim-relax", frame("normal", "happy", { mug: true }), frame("squash", "happy", { mug: true })),
    strip("px-anim-cheer", frame("stretch", "happy"), frame("squash", "happy"), 2),
  ].join("");
  return svg(SLIME_W, SLIME_H, PX, body, "office-sprite");
}

/// A desk (a monitor, a crystal, a lantern, … by theme); the `n`/`N` pixels
/// are coloured by the desk's status class.
export function deskSvg(theme: ThemeId = "office"): string {
  const t = THEMES[theme];
  return svg(width(t.desk), t.desk.length, PX, rects(t.desk, t.pal), "office-desk__svg");
}

/// Furniture is sized in art pixels (`--px`, which follows the view's
/// scale) rather than transformed, so it takes up its real size in layout.
function prop(rows: readonly string[], pal: Palette, cls: string): string {
  const w = width(rows);
  const h = rows.length;
  return `<svg class="office-prop ${cls}" viewBox="0 0 ${w} ${h}" style="width:calc(${w} * var(--px));height:calc(${h} * var(--px))" shape-rendering="crispEdges" aria-hidden="true">${rects(rows, pal)}</svg>`;
}

function piece(p: Piece, pal: Palette, cls: string): string {
  return "rows" in p ? prop(p.rows, p.pal, cls) : prop(p, pal, cls);
}

const wall = (items: string[]): string => `<div class="office-wall-row">${items.join("")}</div>`;

/// The furniture of a room, by the kind of pane it is and the theme: a row
/// along the back wall, plus floor pieces placed by their class
/// (`office-prop--…`).
export function roomProps(kind: PaneKind, theme: ThemeId = "office"): string {
  const t = THEMES[theme];
  const back = (list: Piece[]): string => wall(list.map((p) => piece(p, t.pal, "")));
  const corner = piece(t.corner, t.pal, "office-prop--plant");
  switch (kind) {
    case "files":
      return back(t.wall.files) + corner;
    case "git":
      return back(t.wall.git) + corner;
    case "browser":
    case "native_browser":
    case "embedded_browser":
      return back(t.wall.main) + piece(t.lounge, t.pal, "office-prop--sofa") + corner;
    case "editor":
      return back(t.wall.main) + piece(t.side, t.pal, "office-prop--side") + corner;
    case "token_usage":
      return back(t.wall.board) + corner;
    case "terminal":
    default:
      return back(t.wall.main) + corner;
  }
}

/// The lobby's furniture: a centrepiece (reception, campfire, throne, …),
/// a side piece and two small ones.
export function lobbyProps(theme: ThemeId = "office"): string {
  const t = THEMES[theme];
  return (
    piece(t.lobby.centre, t.pal, "office-prop--reception") +
    piece(t.lobby.side, t.pal, "office-prop--cooler") +
    piece(t.lobby.left, t.pal, "office-prop--lobby-plant-l") +
    piece(t.lobby.right, t.pal, "office-prop--lobby-plant-r")
  );
}

export type IconName = keyof typeof art.ICONS;

/// A small pixel icon for a speech bubble.
export function iconSvg(name: string): string {
  const rows = art.ICONS[name];
  return rows ? svg(width(rows), rows.length, 2, rects(rows, ICON), "office-icon") : "";
}

/// Like `prop`, without the furniture class: for sprites inside buttons,
/// which must stay in flow so the button takes their size.
function fixture(rows: readonly string[], pal: Palette, cls: string, over = ""): string {
  const w = width(rows);
  const h = rows.length;
  return `<svg class="${cls}" viewBox="0 0 ${w} ${h}" style="width:calc(${w} * var(--px));height:calc(${h} * var(--px))" shape-rendering="crispEdges" aria-hidden="true">${rects(rows, pal)}${over}</svg>`;
}

/// The done flag outside a room.
export function flagSvg(): string {
  return fixture(art.FLAG, FIXTURES, "office-flag__svg");
}

/// A mailbox; `letter` sticks out of the slot while it holds mail.
export function mailboxSvg(letter: "sealed" | "open" | null): string {
  const top = letter === "sealed" ? art.LETTER_SEALED : letter === "open" ? art.LETTER_OPEN : null;
  return fixture(art.MAILBOX, FIXTURES, "office-mailbox__svg", top ? rects(top, LETTER) : "");
}

/// The lobby's cork board, `w` x `h` art pixels.
export function corkSvg(w: number, h: number): string {
  return fixture(art.corkGrid(w, h), FIXTURES, "office-board__svg");
}
