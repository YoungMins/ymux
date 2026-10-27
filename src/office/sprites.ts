// Pixel-art SVG for the office view. The grids live in `pixelArt.ts`; this
// turns them into inline SVG (no external assets, so the app's CSP is happy)
// with `shape-rendering="crispEdges"`, one <rect> per run of equal pixels.
// A character is a stack of pose groups (`px-still-…`, `px-anim-…`) that the
// CSS shows one of at a time.

import type { PaneKind } from "../types";
import * as art from "./pixelArt";
import { MUG, SLIME_H, SLIME_W, blinkGrid, bodyGrid, eyesGrid, sproutGrid, tieGrid, type Body, type Eyes } from "./slimeArt";

type Palette = Readonly<Record<string, string>>;

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

const PROPS: Palette = {
  o: "#2a1f1a",
  M: "#2d333b",
  n: ".px-screen",
  N: ".px-screen-hi",
  k: "#d8dde3",
  K: "#9aa3ad",
  d: "#c98e57",
  D: "#a56f40",
  f: "#9a6236",
  F: "#7a4a28",
  g: "#e1c08a",
  G: "#2f7a3b",
  a: "#b0623a",
  A: "#8a4a2a",
  w: "#f3ece0",
  "1": "#d64545",
  "2": "#4a90d9",
  "3": "#e0b84f",
  "4": "#5cb85c",
  "5": "#9b6bd6",
  "6": "#e07a3a",
  R: "#4a5260",
  y: "#f5c542",
  b: "#8ecbf0",
  B: "#ffffff",
  r: "#d64545",
  c: "#5b8c5a",
  C: "#4a7449",
};
const PLANT: Palette = { ...PROPS, g: "#4fa35a", G: "#2f7a3b" };
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
const RACK: Palette = { ...PROPS, g: ".px-led", y: ".px-led-warn", "1": "#7d8794" };
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
export function characterSvg(kind: string, role: "lead" | "sub" | "manager"): string {
  const jelly = role === "manager" ? JELLIES.manager : (JELLIES[kind] ?? FALLBACK);
  const pal: Palette = { ...BASE, ...jelly };
  const frame = (body: Body, eyes: Eyes, opts: { mug?: boolean; blink?: boolean } = {}): string =>
    rects(bodyGrid(body), pal) +
    rects(eyesGrid(body, eyes), pal) +
    (role === "sub" ? rects(sproutGrid(body), pal) : "") +
    (role === "manager" ? rects(tieGrid(body), pal) : "") +
    (opts.mug ? rects(MUG, pal) : "") +
    (opts.blink ? `<g class="px-blink">${rects(blinkGrid(body), pal)}</g>` : "");
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

/// A desk with a monitor; the screen pixels are coloured by the desk's
/// status class.
export function deskSvg(): string {
  return svg(width(art.DESK), art.DESK.length, PX, rects(art.DESK, PROPS), "office-desk__svg");
}

/// Furniture is sized in art pixels (`--px`, which follows the view's
/// scale) rather than transformed, so it takes up its real size in layout.
function prop(rows: readonly string[], pal: Palette, cls: string): string {
  const w = width(rows);
  const h = rows.length;
  return `<svg class="office-prop ${cls}" viewBox="0 0 ${w} ${h}" style="width:calc(${w} * var(--px));height:calc(${h} * var(--px))" shape-rendering="crispEdges" aria-hidden="true">${rects(rows, pal)}</svg>`;
}

const wall = (...items: string[]): string => `<div class="office-wall-row">${items.join("")}</div>`;

/// The furniture of a room, by the kind of pane it is: a row along the back
/// wall, plus floor pieces placed by their class (`office-prop--…`).
export function roomProps(kind: PaneKind): string {
  const plant = prop(art.PLANT, PLANT, "office-prop--plant");
  switch (kind) {
    case "files":
      return wall(prop(art.SHELF, PROPS, ""), prop(art.SHELF, PROPS, "")) + plant;
    case "git":
      return wall(prop(art.RACK, RACK, ""), prop(art.RACK, RACK, ""), prop(art.RACK, RACK, "")) + plant;
    case "browser":
    case "native_browser":
    case "embedded_browser":
      return wall(prop(art.WINDOW, PROPS, "")) + prop(art.SOFA, PROPS, "office-prop--sofa") + plant;
    case "editor":
      return wall(prop(art.WINDOW, PROPS, "")) + prop(art.PAPERS, PROPS, "office-prop--side") + plant;
    case "token_usage":
      return wall(prop(art.BOARD, PROPS, "")) + plant;
    case "terminal":
    default:
      return wall(prop(art.WINDOW, PROPS, "")) + plant;
  }
}

/// The lobby's furniture: reception desk, water cooler, plants.
export function lobbyProps(): string {
  return (
    prop(art.RECEPTION, PROPS, "office-prop--reception") +
    prop(art.COOLER, PROPS, "office-prop--cooler") +
    prop(art.PLANT, PLANT, "office-prop--lobby-plant-l") +
    prop(art.PLANT, PLANT, "office-prop--lobby-plant-r")
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
