// A tiny pixel painter for the office's hi-res sprites (one art pixel = one
// CSS pixel). Parts are masks (ellipses, polygons, capsules) painted with a
// colour ramp: lit by a fixed light from the upper left, banded into a few
// tones like hand-made pixel art, and edged with the ramp's own dark outline
// so overlapping parts stay readable. Pure: rasters are arrays of colours;
// `sheet.ts` puts them on a canvas.

export type Color = string;

export class Raster {
  readonly px: (Color | null)[];
  constructor(
    readonly w: number,
    readonly h: number,
  ) {
    this.px = new Array<Color | null>(w * h).fill(null);
  }

  get(x: number, y: number): Color | null {
    return x >= 0 && y >= 0 && x < this.w && y < this.h ? this.px[y * this.w + x] : null;
  }

  set(x: number, y: number, c: Color | null): void {
    const xi = Math.round(x);
    const yi = Math.round(y);
    if (xi >= 0 && yi >= 0 && xi < this.w && yi < this.h) this.px[yi * this.w + xi] = c;
  }

  /// Copy `src` over this raster, `dx`/`dy` pixels in; empty pixels of `src`
  /// leave this one alone.
  blit(src: Raster, dx = 0, dy = 0): this {
    for (let y = 0; y < src.h; y++)
      for (let x = 0; x < src.w; x++) {
        const c = src.px[y * src.w + x];
        if (c !== null) this.set(x + dx, y + dy, c);
      }
    return this;
  }

  clone(): Raster {
    const r = new Raster(this.w, this.h);
    for (let i = 0; i < this.px.length; i++) r.px[i] = this.px[i];
    return r;
  }

  /// Mirror left to right.
  flipped(): Raster {
    const r = new Raster(this.w, this.h);
    for (let y = 0; y < this.h; y++) for (let x = 0; x < this.w; x++) r.px[y * this.w + x] = this.px[y * this.w + (this.w - 1 - x)];
    return r;
  }

  /// Pixels drawn.
  count(): number {
    return this.px.reduce((n, c) => n + (c === null ? 0 : 1), 0);
  }
}

// ── Colour ──────────────────────────────────────────────────────────────

function clamp01(v: number): number {
  return Math.max(0, Math.min(1, v));
}

export function hexToRgb(hex: Color): [number, number, number] {
  const h = hex.replace("#", "");
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}

export function rgbToHex(r: number, g: number, b: number): Color {
  const c = (v: number) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0");
  return `#${c(r)}${c(g)}${c(b)}`;
}

function toHsl(hex: Color): [number, number, number] {
  const [r, g, b] = hexToRgb(hex).map((v) => v / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  h /= 6;
  return [h, s, l];
}

function fromHsl(h: number, s: number, l: number): Color {
  const hue = (p: number, q: number, t: number) => {
    const tt = (t + 1) % 1;
    if (tt < 1 / 6) return p + (q - p) * 6 * tt;
    if (tt < 1 / 2) return q;
    if (tt < 2 / 3) return p + (q - p) * (2 / 3 - tt) * 6;
    return p;
  };
  if (s === 0) return rgbToHex(l * 255, l * 255, l * 255);
  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  return rgbToHex(hue(p, q, h + 1 / 3) * 255, hue(p, q, h) * 255, hue(p, q, h - 1 / 3) * 255);
}

/// `hex` with its lightness moved by `dl`, saturation by `ds` and hue by `dh`
/// (in turns): the hue-shifted shading of pixel art, shadows cooler and
/// highlights warmer.
export function adjust(hex: Color, dl: number, ds = 0, dh = 0): Color {
  const [h, s, l] = toHsl(hex);
  return fromHsl((h + dh + 1) % 1, clamp01(s + ds), clamp01(l + dl));
}

/// The tones a part is painted with, dark to light, plus its outline.
export interface Ramp {
  outline: Color;
  tones: [Color, Color, Color, Color, Color];
}

/// A five-tone ramp around `base` (the middle tone).
export function ramp(base: Color, outline?: Color): Ramp {
  return {
    outline: outline ?? adjust(base, -0.42, 0.05, -0.03),
    tones: [
      adjust(base, -0.24, 0.04, -0.025),
      adjust(base, -0.12, 0.02, -0.012),
      base,
      adjust(base, 0.1, -0.02, 0.01),
      adjust(base, 0.22, -0.08, 0.02),
    ],
  };
}

/// A flat ramp: every tone `c` (eyes, small details).
export function flat(c: Color, outline?: Color): Ramp {
  return { outline: outline ?? c, tones: [c, c, c, c, c] };
}

// ── Masks and light ─────────────────────────────────────────────────────

/// Whether pixel (x, y) is in a shape; tested at the pixel's centre. A mask
/// may carry the box it lies in (`bounded`), so painting it scans only that.
export type Mask = ((x: number, y: number) => boolean) & { box?: Box };
/// [x0, y0, x1, y1], inclusive pixel bounds.
export type Box = [number, number, number, number];
/// How lit pixel (x, y) is, 0 (dark) to 1 (bright).
export type Light = (x: number, y: number) => number;

function bounded(m: (x: number, y: number) => boolean, box: Box | undefined): Mask {
  const out = m as Mask;
  if (box) out.box = box;
  return out;
}

function boxOf(x0: number, y0: number, x1: number, y1: number): Box {
  return [Math.floor(x0) - 1, Math.floor(y0) - 1, Math.ceil(x1) + 1, Math.ceil(y1) + 1];
}

export function ellipse(cx: number, cy: number, rx: number, ry: number): Mask {
  return bounded((x, y) => {
    const dx = (x + 0.5 - cx) / rx;
    const dy = (y + 0.5 - cy) / ry;
    return dx * dx + dy * dy <= 1;
  }, boxOf(cx - rx, cy - ry, cx + rx, cy + ry));
}

export function rect(x0: number, y0: number, w: number, h: number): Mask {
  return bounded((x, y) => x >= x0 && x < x0 + w && y >= y0 && y < y0 + h, boxOf(x0, y0, x0 + w, y0 + h));
}

export function polygon(points: readonly [number, number][]): Mask {
  const xs = points.map((p) => p[0]);
  const ys = points.map((p) => p[1]);
  return bounded(
    (x, y) => {
      const px = x + 0.5;
      const py = y + 0.5;
      let inside = false;
      for (let i = 0, j = points.length - 1; i < points.length; j = i++) {
        const [xi, yi] = points[i];
        const [xj, yj] = points[j];
        if (yi > py !== yj > py && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi) inside = !inside;
      }
      return inside;
    },
    boxOf(Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)),
  );
}

/// A thick line with round ends, from (x0, y0) to (x1, y1).
export function capsule(x0: number, y0: number, x1: number, y1: number, r: number): Mask {
  return bounded(
    (x, y) => {
      const px = x + 0.5;
      const py = y + 0.5;
      const vx = x1 - x0;
      const vy = y1 - y0;
      const len2 = vx * vx + vy * vy || 1;
      const t = Math.max(0, Math.min(1, ((px - x0) * vx + (py - y0) * vy) / len2));
      const dx = px - (x0 + t * vx);
      const dy = py - (y0 + t * vy);
      return dx * dx + dy * dy <= r * r;
    },
    boxOf(Math.min(x0, x1) - r, Math.min(y0, y1) - r, Math.max(x0, x1) + r, Math.max(y0, y1) + r),
  );
}

export function union(...ms: Mask[]): Mask {
  const box: Box | undefined = ms.every((m) => m.box)
    ? [Math.min(...ms.map((m) => m.box![0])), Math.min(...ms.map((m) => m.box![1])), Math.max(...ms.map((m) => m.box![2])), Math.max(...ms.map((m) => m.box![3]))]
    : undefined;
  return bounded((x, y) => ms.some((m) => m(x, y)), box);
}

export function minus(a: Mask, b: Mask): Mask {
  return bounded((x, y) => a(x, y) && !b(x, y), a.box);
}

export function intersect(a: Mask, b: Mask): Mask {
  const box: Box | undefined =
    a.box && b.box ? [Math.max(a.box[0], b.box[0]), Math.max(a.box[1], b.box[1]), Math.min(a.box[2], b.box[2]), Math.min(a.box[3], b.box[3])] : (a.box ?? b.box);
  return bounded((x, y) => a(x, y) && b(x, y), box);
}

const LIGHT = (() => {
  const v = [-0.55, -0.62, 0.56];
  const n = Math.hypot(...v);
  return v.map((c) => c / n);
})();

/// Light on a dome (a sphere squashed to the box): the shading of round things.
export function dome(cx: number, cy: number, rx: number, ry: number): Light {
  return (x, y) => {
    const nx = (x + 0.5 - cx) / rx;
    const ny = (y + 0.5 - cy) / ry;
    const nz = Math.sqrt(Math.max(0, 1 - nx * nx - ny * ny));
    return clamp01(0.32 + 0.78 * (nx * LIGHT[0] + ny * LIGHT[1] + nz * LIGHT[2]));
  };
}

/// Light falling from the top (and a little from the left) of a box.
export function slope(x0: number, y0: number, w: number, h: number): Light {
  return (x, y) => clamp01(0.95 - 0.55 * ((y - y0) / Math.max(1, h)) - 0.25 * ((x - x0) / Math.max(1, w)));
}

export const even =
  (v: number): Light =>
  () =>
    v;

/// 4x4 Bayer thresholds: a light dither only where two tones meet.
const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

export function tone(r: Ramp, light: number, x: number, y: number, dither = 0.35): Color {
  const v = light * 4 + (BAYER[(y & 3) * 4 + (x & 3)] / 16 - 0.5) * dither;
  return r.tones[Math.max(0, Math.min(4, Math.round(v)))];
}

export interface PaintOptions {
  light?: Light;
  /// Edge the part with the ramp's outline (default on).
  outline?: boolean;
  dither?: number;
  /// Only edges facing outward from these sides get the outline (all by
  /// default); a part sitting on another can leave its base open.
  clip?: Mask;
}

/// Paint `mask` with `r`: shaded by `light`, edged with the ramp's outline.
export function paint(out: Raster, mask: Mask, r: Ramp, opts: PaintOptions = {}): Raster {
  const light = opts.light ?? even(0.6);
  const outline = opts.outline ?? true;
  const dither = opts.dither ?? 0.35;
  const [x0, y0, x1, y1] = mask.box ?? [0, 0, out.w - 1, out.h - 1];
  for (let y = Math.max(0, y0); y <= Math.min(out.h - 1, y1); y++) {
    for (let x = Math.max(0, x0); x <= Math.min(out.w - 1, x1); x++) {
      if (!mask(x, y) || (opts.clip && !opts.clip(x, y))) continue;
      const edge = outline && (!mask(x - 1, y) || !mask(x + 1, y) || !mask(x, y - 1) || !mask(x, y + 1));
      out.set(x, y, edge ? r.outline : tone(r, light(x, y), x, y, dither));
    }
  }
  return out;
}

/// Set single pixels: `[x, y]` pairs in colour `c`.
export function dots(out: Raster, c: Color, pts: readonly (readonly [number, number])[], dx = 0, dy = 0): Raster {
  for (const [x, y] of pts) out.set(x + dx, y + dy, c);
  return out;
}
