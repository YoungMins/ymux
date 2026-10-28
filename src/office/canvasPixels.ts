// Rasters onto canvases: the one place that turns `#rrggbb` pixels into
// RGBA bytes, shared by the sprite sheets, the hi-res pieces and the tiles.

/// jsdom (the tests) has no canvas; asking it for one only logs an error.
export const NO_CANVAS = typeof navigator !== "undefined" && /jsdom/i.test(navigator.userAgent);

/// Canvas drawing is possible here.
export function canDraw(): boolean {
  return !NO_CANVAS && typeof ImageData !== "undefined";
}

const rgbs = new Map<string, number>();

/// `#rrggbb` as 0xRRGGBB, parsed once per colour.
function rgbOf(c: string): number {
  let v = rgbs.get(c);
  if (v === undefined) {
    v = parseInt(c.slice(1, 7), 16);
    rgbs.set(c, v);
  }
  return v;
}

/// Write the `w`-wide pixels `px` into RGBA `data` whose rows are `stride`
/// pixels wide, starting at column `x0`. Null pixels are left transparent.
export function writeRgba(data: Uint8ClampedArray, px: readonly (string | null)[], w: number, stride = w, x0 = 0): void {
  for (let i = 0; i < px.length; i++) {
    const c = px[i];
    if (c === null) continue;
    const v = rgbOf(c);
    const o = (Math.floor(i / w) * stride + x0 + (i % w)) * 4;
    data[o] = v >> 16;
    data[o + 1] = (v >> 8) & 255;
    data[o + 2] = v & 255;
    data[o + 3] = 255;
  }
}
