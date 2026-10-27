// Pixel-art SVG for the office view. The grids live in `pixelArt.ts`; this
// turns them into inline SVG (no external assets, so the app's CSP is happy)
// with `shape-rendering="crispEdges"`, one <rect> per run of equal pixels.
// A character is a stack of named frame groups (`px-legs-a`, `px-arms-raise`,
// …) that the CSS shows or flips between per pose.

import type { PaneKind } from "../types";
import * as art from "./pixelArt";

type Palette = Readonly<Record<string, string>>;

/// Shared colours. A value starting with "." is a CSS class instead of a
/// fill, for pixels the stylesheet colours (a desk's screen follows its
/// pane's status).
const BASE: Palette = {
  o: "#2a1f1a",
  s: "#f7cfa8",
  S: "#e5aa83",
  e: "#241c1c",
  w: "#ffffff",
  k: "#f4a3a0",
  m: "#b4553f",
  p: "#3c4a64",
  P: "#2b364b",
  b: "#26262e",
  u: "#f4efe6",
  U: "#cfc6b6",
  r: "#d64545",
};

interface Outfit {
  h: string;
  H: string;
  c: string;
  C: string;
  t: string;
}

const OUTFITS: Record<string, Outfit> = {
  claude: { h: "#6b4226", H: "#4e2f1a", c: "#d97757", C: "#b85c3f", t: "#fbe3d6" },
  codex: { h: "#23262b", H: "#15171a", c: "#2d333b", C: "#1c2128", t: "#10a37f" },
  gemini: { h: "#3a2a20", H: "#24170f", c: "#4285f4", C: "#2f69c9", t: "#fbbc05" },
  opencode: { h: "#30264f", H: "#1f1835", c: "#6d5dfc", C: "#5144d6", t: "#d6d1ff" },
  aider: { h: "#3f3f46", H: "#27272a", c: "#14b8a6", C: "#0f8f81", t: "#ccfbf1" },
  copilot: { h: "#2d2d2d", H: "#1a1a1a", c: "#8957e5", C: "#6e40c9", t: "#e2d5fb" },
  manager: { h: "#4a3428", H: "#33231a", c: "#3d5a80", C: "#2b4260", t: "#f0f0f0" },
};
const FALLBACK: Outfit = { h: "#3a3a3a", H: "#262626", c: "#7d8794", C: "#5f6873", t: "#d9dee4" };

/// The colour a kind wears, for the header legend.
export function paletteOf(kind: string): { shirt: string } {
  return { shirt: (OUTFITS[kind] ?? FALLBACK).c };
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

/// A character: every frame group, the CSS picks which ones show.
/// `role` "sub" wears a cap; "manager" (the user) wears a tie.
export function characterSvg(kind: string, role: "lead" | "sub" | "manager"): string {
  const outfit = role === "manager" ? OUTFITS.manager : (OUTFITS[kind] ?? FALLBACK);
  const pal: Palette = { ...BASE, ...outfit };
  const g = (cls: string, rows: readonly string[]): string => `<g class="${cls}">${rects(rows, pal)}</g>`;
  const body = [
    `<rect class="px-shadow" x="4" y="23" width="14" height="1" fill="rgba(0,0,0,0.3)"/>`,
    g("px-legs px-legs-stand", art.LEGS_STAND),
    g("px-legs px-legs-a", art.LEGS_A),
    g("px-legs px-legs-b", art.LEGS_B),
    g("px-torso", art.TORSO),
    role === "manager" ? g("px-tie", art.TIE) : "",
    g("px-head", art.HEAD),
    g("px-eyes px-eyes-open", art.EYES_OPEN),
    g("px-eyes px-eyes-shut", art.EYES_SHUT),
    role === "sub" ? g("px-cap", art.CAP) : "",
    g("px-arms px-arms-down", art.ARMS_DOWN),
    g("px-arms px-arms-type-a", art.ARMS_TYPE_A),
    g("px-arms px-arms-type-b", art.ARMS_TYPE_B),
    g("px-arms px-arms-raise", art.ARMS_RAISE),
    g("px-arms px-arms-mug", art.ARMS_MUG),
  ].join("");
  return svg(art.CHAR_W, art.CHAR_H, PX, body, "office-sprite");
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
