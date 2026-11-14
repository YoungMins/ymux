// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { MIN_MODULE_PX, QUIET, qrMatrix, qrPath, qrScale, qrSvg } from "./qr";

describe("qr", () => {
  const url = "https://ymux.example.com/pair#0123456789abcdef";

  it("builds a square matrix with the three finder patterns", () => {
    const m = qrMatrix(url);
    const n = m.length;
    // Version n = 17 + 4v, so a valid size is 21, 25, 29, ...
    expect((n - 17) % 4).toBe(0);
    expect(n).toBeGreaterThanOrEqual(21);
    expect(m.every((row) => row.length === n)).toBe(true);
    // Each finder pattern: 7x7 dark ring, light ring, 3x3 dark core.
    for (const [r0, c0] of [
      [0, 0],
      [0, n - 7],
      [n - 7, 0],
    ]) {
      for (let r = 0; r < 7; r++) {
        for (let c = 0; c < 7; c++) {
          const edge = r === 0 || r === 6 || c === 0 || c === 6;
          const core = r >= 2 && r <= 4 && c >= 2 && c <= 4;
          expect(m[r0 + r][c0 + c]).toBe(edge || core);
        }
      }
    }
  });

  it("is deterministic and differs per text", () => {
    expect(qrMatrix(url)).toEqual(qrMatrix(url));
    expect(qrMatrix(url)).not.toEqual(qrMatrix(`${url}x`));
  });

  it("turns runs of dark modules into path commands inside the quiet zone", () => {
    const d = qrPath(
      [
        [true, true, false, true],
        [false, false, false, false],
      ],
      1,
    );
    expect(d).toBe("M1 1h2v1h-2zM4 1h1v1h-1z");
  });

  it("fits the pairing link (about 230 characters) and stays crisp at a scannable size", () => {
    // `<server>/#pair=<desktop id>.<code>.<dk>`: dk alone is 87 characters.
    const link = `https://ymux.example.com/#pair=${"d".repeat(36)}.${"c".repeat(22)}.${"k".repeat(87)}`;
    const long = `${link}${"/x".repeat(30)}`.slice(0, 230);
    expect(link.length).toBeGreaterThan(170);
    expect(long).toHaveLength(230);
    for (const text of [link, long]) {
      const n = qrMatrix(text).length;
      // Version 10 or 11 at level M: 57 or 61 modules.
      expect(n).toBeLessThanOrEqual(61);
      const svg = qrSvg(text);
      const px = Number(svg.getAttribute("width"));
      const modules = n + QUIET * 2;
      expect(Number(svg.getAttribute("height"))).toBe(px);
      expect(Number.isInteger(px / modules)).toBe(true);
      expect(px / modules).toBeGreaterThanOrEqual(MIN_MODULE_PX);
      expect(px / modules).toBe(qrScale(n));
    }
  });

  it("scales a small code up and never below 4 px per module", () => {
    expect(qrScale(21)).toBeGreaterThanOrEqual(8);
    expect(qrScale(61)).toBe(MIN_MODULE_PX);
    expect(qrScale(177)).toBe(MIN_MODULE_PX);
    for (let n = 21; n <= 177; n += 4) {
      expect(qrScale(n)).toBeGreaterThanOrEqual(MIN_MODULE_PX);
    }
  });

  it("draws an svg without data: urls or inline styles", () => {
    const svg = qrSvg(url);
    const n = qrMatrix(url).length + QUIET * 2;
    expect(svg.getAttribute("viewBox")).toBe(`0 0 ${n} ${n}`);
    expect(svg.getAttribute("style")).toBeNull();
    expect(svg.outerHTML).not.toContain("data:");
    expect(svg.querySelector("path")?.getAttribute("d")?.startsWith("M")).toBe(true);
  });
});
