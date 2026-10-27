// Pixel-art SVG for the office's furniture and fixtures. The grids live in
// `pixelArt.ts` and `themeArt.ts`; this turns them into inline SVG (no
// external assets, so the app's CSP is happy) with
// `shape-rendering="crispEdges"`, one <rect> per run of equal pixels.
// Characters and pets are hi-res canvas sheets (`sheet.ts`).

import type { PaneKind } from "../types";
import * as art from "./pixelArt";
import { PROPS, THEMES, type Palette, type Piece, type ThemeId } from "./themes";

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
