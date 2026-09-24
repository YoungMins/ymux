// Flattens an xterm buffer row (and the rows it wraps onto) into a plain
// string, keeping a per-code-unit map back to buffer cells.
//
// The matcher in `pathMatch.ts` works on strings; xterm's `ILink.range` is
// expressed in buffer cells. Something has to bridge the two, and the naive
// bridge — "string index == column" — is wrong twice over:
//
//  - **Wide characters.** A Korean or CJK glyph occupies two cells but one
//    code unit, and the second cell reports width 0. Counting columns as
//    characters shifts every link on the row right of the first such glyph.
//  - **Wrapping.** A long path printed into a narrow pane is split across
//    several buffer rows with `isWrapped` set. Matching each row on its own
//    would find neither half.
//
// So this walks cells, not columns, and joins a whole wrap group before
// handing the text to the matcher. Pure: the buffer is reached through the
// minimal duck-typed interfaces below, so it is testable without xterm.

/// The slice of xterm's `IBufferCell` this needs.
export interface RowCell {
  getChars(): string;
  getWidth(): number;
}

/// The slice of xterm's `IBufferLine` this needs.
export interface RowLine {
  readonly length: number;
  readonly isWrapped: boolean;
  getCell(x: number): RowCell | undefined;
}

/// The slice of xterm's `IBuffer` this needs.
export interface RowBuffer {
  getLine(y: number): RowLine | undefined;
}

/// Where one code unit of the flattened text lives in the buffer.
export interface CellPos {
  /// 0-based buffer row.
  y: number;
  /// 0-based column of the cell's first (or only) column.
  x: number;
  /// How many columns the cell occupies — 2 for a wide glyph, else 1.
  w: number;
}

export interface FlatRow {
  /// The row group's text, trailing blanks removed.
  text: string;
  /// `pos[i]` is the buffer cell holding `text`'s code unit `i`.
  /// `pos.length === text.length`.
  pos: CellPos[];
}

/// Stop expanding a wrap group past this many characters. Mirrors the
/// web-links addon's own 2048-per-direction guard: a pane that has been
/// `cat`ing a minified bundle can wrap a single logical line over hundreds
/// of rows, and hovering it must not turn into an O(screen) walk.
export const MAX_FLAT_LENGTH = 4096;

/// Flatten the wrap group containing buffer row `y` (0-based).
export function flattenWrappedRow(
  buffer: RowBuffer,
  y: number,
  maxLength: number = MAX_FLAT_LENGTH,
): FlatRow {
  if (!buffer.getLine(y)) return { text: "", pos: [] };

  // Walk up to the row that started the wrap group.
  let top = y;
  for (let guard = 0; guard < maxLength; guard++) {
    const line = buffer.getLine(top);
    if (!line?.isWrapped) break;
    if (!buffer.getLine(top - 1)) break;
    top--;
  }

  let text = "";
  const pos: CellPos[] = [];
  for (let row = top; text.length < maxLength; row++) {
    const line = buffer.getLine(row);
    if (!line) break;
    if (row !== top && !line.isWrapped) break;
    for (let x = 0; x < line.length; x++) {
      const cell = line.getCell(x);
      if (!cell) continue;
      const w = cell.getWidth();
      // Width 0 is the right half of a wide glyph — no content of its own.
      if (w === 0) continue;
      // An untouched cell reports "" and stands for a blank column.
      const chars = cell.getChars() || " ";
      for (let k = 0; k < chars.length; k++) pos.push({ y: row, x, w });
      text += chars;
    }
  }

  // Only the group's last row can carry blank padding, and it is never part
  // of a path, so trimming the assembled text is safe and keeps the matcher
  // from seeing a run of hundreds of spaces.
  let end = text.length;
  while (end > 0 && text[end - 1] === " ") end--;
  return { text: text.slice(0, end), pos: pos.slice(0, end) };
}

/// xterm's `IBufferRange`, 1-based with an inclusive end.
export interface LinkRange {
  start: { x: number; y: number };
  end: { x: number; y: number };
}

/// Convert a `[start, end)` span of a `FlatRow`'s text into the buffer range
/// xterm wants. Returns `null` when the span is empty or out of bounds.
export function spanToRange(flat: FlatRow, start: number, end: number): LinkRange | null {
  if (start < 0 || end <= start || end > flat.pos.length) return null;
  const first = flat.pos[start]!;
  const last = flat.pos[end - 1]!;
  return {
    start: { x: first.x + 1, y: first.y + 1 },
    // Inclusive, and a wide glyph's link has to cover both of its columns.
    end: { x: last.x + last.w, y: last.y + 1 },
  };
}
