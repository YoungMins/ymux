// The office's furniture and fixtures on the page. The grids live in
// `pixelArt.ts` and `themeArt.ts`; `hiProps` redraws each at three times its
// resolution, which goes on a <canvas> (no external assets or data: URLs, so
// the app's CSP is happy) with the pixels the stylesheet colours (screens,
// LEDs, flames) as an SVG overlay. The markup is a string; `paintProps` fills
// the canvases once it is in the document. Bubble icons stay small SVG.
// Characters and pets are canvas sheets too (`sheet.ts`).

import type { PaneKind } from "../types";
import * as art from "./pixelArt";
import { hiPiece, type HiPiece } from "./hiProps";
import { FUN_ART, FUN_PAL } from "./funArt";
import { PROPS, THEMES, type Palette, type Piece, type ThemeId } from "./themes";
import { canDraw, writeRgba } from "./canvasPixels";

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
const LETTER: Palette = { ...FIXTURES, L: "#ffffff" };
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

/// Pieces already redrawn, by grid and palette; `data-hp` on a canvas is
/// the index into `pieces`.
const pieceIds = new Map<string, number>();
const pieces: HiPiece[] = [];

function pieceId(rows: readonly string[], pal: Palette): number {
  const key = `${rows.join("/")}#${JSON.stringify(pal)}`;
  let id = pieceIds.get(key);
  if (id === undefined) {
    id = pieces.length;
    pieces.push(hiPiece(rows, pal));
    pieceIds.set(key, id);
  }
  return id;
}

/// A piece as markup: a box its size holding the canvas and, when it has
/// stylesheet-coloured pixels, the SVG over it.
function hi(rows: readonly string[], pal: Palette, cls: string): string {
  const id = pieceId(rows, pal);
  const { raster, classes } = pieces[id];
  const { w, h } = raster;
  const over = classes.length
    ? `<svg class="hp-over" viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" shape-rendering="crispEdges">${classes
        .map((c) => `<rect x="${c.x}" y="${c.y}" width="${c.w}" height="1" class="${c.cls}"/>`)
        .join("")}</svg>`
    : "";
  return `<span class="hp ${cls}" style="width:${w}px;height:${h}px" aria-hidden="true"><canvas class="hp-canvas" data-hp="${id}" width="${w}" height="${h}"></canvas>${over}</span>`;
}

/// Draw every piece canvas under `root` not drawn yet.
export function paintProps(root: ParentNode): void {
  if (!canDraw()) return;
  for (const c of root.querySelectorAll<HTMLCanvasElement>("canvas.hp-canvas[data-hp]")) {
    const p = pieces[Number(c.dataset.hp)];
    const ctx = p ? c.getContext("2d") : null;
    if (ctx) ctx.putImageData(imageOf(p.raster.px, p.raster.w, p.raster.h), 0, 0);
    c.removeAttribute("data-hp");
  }
}

const images = new Map<HiPiece["raster"]["px"], ImageData>();

function imageOf(px: (string | null)[], w: number, h: number): ImageData {
  let img = images.get(px);
  if (img) return img;
  const data = new Uint8ClampedArray(w * h * 4);
  writeRgba(data, px, w);
  img = new ImageData(data, w, h);
  images.set(px, img);
  return img;
}

/// A desk (a monitor, a crystal, a lantern, … by theme); its screen pixels
/// are coloured by the desk's status class.
export function deskSvg(theme: ThemeId = "office"): string {
  const t = THEMES[theme];
  return hi(t.desk, t.pal, "office-desk__svg");
}

function prop(rows: readonly string[], pal: Palette, cls: string): string {
  return hi(rows, pal, `office-prop ${cls}`);
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

/// The done flag outside a room.
export function flagSvg(): string {
  return hi(art.FLAG, FIXTURES, "office-flag__svg");
}

/// A mailbox; `letter` sticks out of the slot while it holds mail.
export function mailboxSvg(letter: "sealed" | "open" | null): string {
  const top = letter === "sealed" ? art.LETTER_SEALED : letter === "open" ? art.LETTER_OPEN : null;
  // The letter, drawn into the box's grid (its white keyed apart).
  const rows = top
    ? art.MAILBOX.map((r, y) => [...r].map((k, x) => (top[y]?.[x] && top[y][x] !== "." ? (top[y][x] === "w" ? "L" : top[y][x]) : k)).join(""))
    : art.MAILBOX;
  return hi(rows, LETTER, "office-mailbox__svg");
}

/// The lobby's cork board, `w` x `h` art pixels.
export function corkSvg(w: number, h: number): string {
  return hi(art.corkGrid(w, h), FIXTURES, "office-board__svg");
}

/// A piece of the office games' art (`funArt`): shop furniture, the
/// seasons' and events' props. "" for an unknown id.
export function funProp(id: string, cls: string): string {
  const rows = FUN_ART[id];
  return rows ? hi(rows, FUN_PAL, cls) : "";
}

/// A theme piece by name from another theme (the snow theme's tree at
/// Christmas, whatever the theme).
export function themePiece(theme: ThemeId, pick: (t: (typeof THEMES)[ThemeId]) => Piece, cls: string): string {
  const t = THEMES[theme];
  return piece(pick(t), t.pal, cls);
}
