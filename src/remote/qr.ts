// QR code for the pairing URL, drawn as an inline <svg> (the app's CSP forbids
// data: images). The matrix comes from `qrcode-generator` (byte mode, error
// correction M); everything else here is ours and pure apart from `qrSvg`.

import qrcode from "qrcode-generator";

/// Quiet zone around the symbol, in modules (the QR spec asks for 4).
export const QUIET = 4;
/// Fewest screen pixels per module that a phone camera reads reliably.
export const MIN_MODULE_PX = 4;
/// The box a small code is scaled up to (a bigger code keeps its own size).
const TARGET_PX = 240;

/// Whole screen pixels per module for a symbol of `modules` modules: enough to
/// fill about [`TARGET_PX`], never fewer than [`MIN_MODULE_PX`]. The pairing
/// link (`<server>/#pair=<id>.<code>.<dk>`, about 180 to 230 characters) needs
/// a version 10 to 11 symbol at level M, 57 to 61 modules (65 to 69 with the
/// quiet zone): a fixed 220 px box would be only 3.2 px per module.
export function qrScale(modules: number): number {
  return Math.max(MIN_MODULE_PX, Math.ceil(TARGET_PX / (modules + QUIET * 2)));
}

/// The module matrix of `text`: true = dark. Square, version chosen to fit.
export function qrMatrix(text: string): boolean[][] {
  const qr = qrcode(0, "M");
  qr.addData(text, "Byte");
  qr.make();
  const n = qr.getModuleCount();
  const rows: boolean[][] = [];
  for (let r = 0; r < n; r++) {
    const row: boolean[] = [];
    for (let c = 0; c < n; c++) row.push(qr.isDark(r, c));
    rows.push(row);
  }
  return rows;
}

/// One SVG path for every dark module: a horizontal run per `M x y h len v1 h-len z`,
/// offset by the quiet zone.
export function qrPath(matrix: readonly (readonly boolean[])[], quiet = QUIET): string {
  const parts: string[] = [];
  matrix.forEach((row, r) => {
    let c = 0;
    while (c < row.length) {
      if (!row[c]) {
        c++;
        continue;
      }
      let end = c;
      while (end < row.length && row[end]) end++;
      parts.push(`M${c + quiet} ${r + quiet}h${end - c}v1h-${end - c}z`);
      c = end;
    }
  });
  return parts.join("");
}

const NS = "http://www.w3.org/2000/svg";

/// An <svg> of the code: white background, black modules, scalable by CSS.
export function qrSvg(text: string): SVGSVGElement {
  const matrix = qrMatrix(text);
  const size = matrix.length + QUIET * 2;
  const px = size * qrScale(matrix.length);
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", `0 0 ${size} ${size}`);
  // Attributes, not CSS: the CSP forbids inline styles, and the box must be a
  // whole number of pixels per module to stay crisp.
  svg.setAttribute("width", String(px));
  svg.setAttribute("height", String(px));
  svg.setAttribute("shape-rendering", "crispEdges");
  svg.setAttribute("role", "img");
  svg.setAttribute("class", "remote-qr");
  const bg = document.createElementNS(NS, "rect");
  bg.setAttribute("width", String(size));
  bg.setAttribute("height", String(size));
  bg.setAttribute("fill", "#ffffff");
  const dark = document.createElementNS(NS, "path");
  dark.setAttribute("d", qrPath(matrix));
  dark.setAttribute("fill", "#000000");
  svg.append(bg, dark);
  return svg;
}
