// Floors and walls on the page: a tile (`hiFloors`) repeated across a
// surface by a canvas pattern, on a <canvas> behind the surface's content.
// The surface keeps its CSS background as the fallback (and in tests, where
// there is no canvas). A surface is repainted only when its size or tile
// changes, so calling this on every render is cheap.

import type { Raster } from "./paint";

import { NO_CANVAS, writeRgba } from "./canvasPixels";

const tileCanvases = new WeakMap<Raster, HTMLCanvasElement>();
const drawn = new WeakMap<HTMLCanvasElement, { tile: Raster; w: number; h: number }>();

function tileCanvas(tile: Raster): HTMLCanvasElement | null {
  let c = tileCanvases.get(tile);
  if (c) return c;
  c = document.createElement("canvas");
  c.width = tile.w;
  c.height = tile.h;
  const ctx = c.getContext("2d");
  if (!ctx) return null;
  const img = ctx.createImageData(tile.w, tile.h);
  writeRgba(img.data, tile.px, tile.w);
  ctx.putImageData(img, 0, 0);
  tileCanvases.set(tile, c);
  return c;
}

/// Cover `el` with `tile` (or take the cover away for null). `cls` adds a
/// class to the canvas, for surfaces it covers only part of.
export function tileSurface(el: HTMLElement, tile: Raster | null, cls = ""): void {
  if (NO_CANVAS) return;
  let canvas = el.querySelector<HTMLCanvasElement>(":scope > canvas.hp-tiles");
  if (!tile) {
    canvas?.remove();
    return;
  }
  if (!canvas) {
    canvas = document.createElement("canvas");
    canvas.setAttribute("aria-hidden", "true");
    el.prepend(canvas);
  }
  canvas.className = `hp-tiles ${cls}`.trim();
  const w = Math.max(0, Math.round(canvas.clientWidth));
  const h = Math.max(0, Math.round(canvas.clientHeight));
  const last = drawn.get(canvas);
  if (w === 0 || h === 0 || (last && last.tile === tile && last.w === w && last.h === h)) return;
  const src = tileCanvas(tile);
  const ctx = canvas.getContext("2d");
  if (!src || !ctx) return;
  canvas.width = w;
  canvas.height = h;
  const pattern = ctx.createPattern(src, "repeat");
  if (!pattern) return;
  ctx.fillStyle = pattern;
  ctx.fillRect(0, 0, w, h);
  drawn.set(canvas, { tile, w, h });
}
