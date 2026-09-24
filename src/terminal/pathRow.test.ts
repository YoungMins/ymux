import { describe, expect, it } from "vitest";

import {
  flattenWrappedRow,
  spanToRange,
  type RowBuffer,
  type RowCell,
  type RowLine,
} from "./pathRow";

/// Build a fake buffer from row strings. A character listed in `wide` counts
/// as two columns (its second column reports width 0, as xterm does).
function makeBuffer(
  rows: Array<{ text: string; wrapped?: boolean; cols?: number }>,
  wide: string = "",
): RowBuffer {
  const lines: RowLine[] = rows.map(({ text, wrapped = false, cols }) => {
    const cells: RowCell[] = [];
    for (const ch of text) {
      if (wide.includes(ch)) {
        cells.push({ getChars: () => ch, getWidth: () => 2 });
        cells.push({ getChars: () => "", getWidth: () => 0 });
      } else {
        cells.push({ getChars: () => ch, getWidth: () => 1 });
      }
    }
    // Pad out to the row width with untouched cells, which xterm reports as
    // empty-string width-1.
    while (cols !== undefined && cells.length < cols) {
      cells.push({ getChars: () => "", getWidth: () => 1 });
    }
    return {
      length: cells.length,
      isWrapped: wrapped,
      getCell: (x: number) => cells[x],
    };
  });
  return { getLine: (y: number) => lines[y] };
}

describe("flattenWrappedRow", () => {
  it("returns an empty row for a missing line", () => {
    expect(flattenWrappedRow(makeBuffer([]), 0)).toEqual({ text: "", pos: [] });
  });

  it("flattens a single unwrapped row", () => {
    const flat = flattenWrappedRow(makeBuffer([{ text: "src/main.ts" }]), 0);
    expect(flat.text).toBe("src/main.ts");
    expect(flat.pos).toHaveLength(flat.text.length);
    expect(flat.pos[0]).toEqual({ y: 0, x: 0, w: 1 });
    expect(flat.pos[4]).toEqual({ y: 0, x: 4, w: 1 });
  });

  it("trims the blank padding at the end of a row", () => {
    const flat = flattenWrappedRow(makeBuffer([{ text: "a/b", cols: 20 }]), 0);
    expect(flat.text).toBe("a/b");
    expect(flat.pos).toHaveLength(3);
  });

  it("joins a wrap group and maps each half to its own row", () => {
    const buf = makeBuffer([
      { text: "/very/long" },
      { text: "/path.ts", wrapped: true },
    ]);
    const flat = flattenWrappedRow(buf, 1);
    expect(flat.text).toBe("/very/long/path.ts");
    expect(flat.pos[0]).toEqual({ y: 0, x: 0, w: 1 });
    expect(flat.pos[10]).toEqual({ y: 1, x: 0, w: 1 });
  });

  it("starts from the top of the wrap group whichever row is asked for", () => {
    const buf = makeBuffer([
      { text: "/a" },
      { text: "/b", wrapped: true },
      { text: "/c", wrapped: true },
    ]);
    expect(flattenWrappedRow(buf, 0).text).toBe("/a/b/c");
    expect(flattenWrappedRow(buf, 2).text).toBe("/a/b/c");
  });

  it("stops at the first row that is not wrapped", () => {
    const buf = makeBuffer([{ text: "/a" }, { text: "/b" }]);
    expect(flattenWrappedRow(buf, 0).text).toBe("/a");
  });

  it("maps around a wide glyph without drifting", () => {
    // `한` takes columns 5 and 6, so `/` after it sits at column 7.
    const flat = flattenWrappedRow(makeBuffer([{ text: "/srv/한/x" }], "한"), 0);
    expect(flat.text).toBe("/srv/한/x");
    expect(flat.pos[5]).toEqual({ y: 0, x: 5, w: 2 });
    expect(flat.pos[6]).toEqual({ y: 0, x: 7, w: 1 });
    expect(flat.pos[7]).toEqual({ y: 0, x: 8, w: 1 });
  });

  it("honours the length cap", () => {
    const buf = makeBuffer([
      { text: "x".repeat(50) },
      { text: "y".repeat(50), wrapped: true },
    ]);
    expect(flattenWrappedRow(buf, 0, 40).text).toHaveLength(50);
    expect(flattenWrappedRow(buf, 0, 40).text).not.toContain("y");
  });
});

describe("spanToRange", () => {
  const flat = flattenWrappedRow(makeBuffer([{ text: "run src/a.ts" }]), 0);

  it("converts a span to a 1-based, end-inclusive buffer range", () => {
    // "src/a.ts" is text[4..12).
    expect(spanToRange(flat, 4, 12)).toEqual({
      start: { x: 5, y: 1 },
      end: { x: 12, y: 1 },
    });
  });

  it("covers both columns of a trailing wide glyph", () => {
    const wideFlat = flattenWrappedRow(makeBuffer([{ text: "/a/한" }], "한"), 0);
    expect(spanToRange(wideFlat, 0, 4)).toEqual({
      start: { x: 1, y: 1 },
      end: { x: 5, y: 1 },
    });
  });

  it("spans two rows of a wrap group", () => {
    const buf = makeBuffer([{ text: "/aa" }, { text: "bb", wrapped: true }]);
    const wrapped = flattenWrappedRow(buf, 0);
    expect(spanToRange(wrapped, 0, 5)).toEqual({
      start: { x: 1, y: 1 },
      end: { x: 2, y: 2 },
    });
  });

  it("rejects an empty or out-of-bounds span", () => {
    expect(spanToRange(flat, 3, 3)).toBeNull();
    expect(spanToRange(flat, -1, 2)).toBeNull();
    expect(spanToRange(flat, 0, 999)).toBeNull();
  });
});
