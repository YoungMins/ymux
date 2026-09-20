/// Bottom-anchored prompt.
///
/// A terminal whose content is shorter than its pane is drawn against the
/// pane's bottom edge, so the prompt sits on the last row and output grows
/// upward. It is purely presentational: the PTY, xterm's buffer and its row
/// count never change. `TerminalPane` translates xterm's `.xterm-screen` down
/// by the offset computed here (see the plan
/// docs/superpowers/plans/2026-09-18-bottom-anchor.md for why that element
/// and not `.xterm`).
///
/// Off (offset 0) in the alternate buffer (vim, less, full-screen TUIs) and
/// while the user has scrolled up. Those programs and that view own the whole
/// screen.

/// Everything is in viewport rows: `0` is the top row on screen, `rows - 1` is
/// the bottom one.
export interface AnchorInputs {
  rows: number;
  /// `buffer.active.cursorY`, which xterm already reports viewport-relative.
  cursorY: number;
  /// Lowest viewport row holding any text, or `-1` for none.
  lastContentRow: number;
  altBuffer: boolean;
  /// `viewportY === baseY`, i.e. the user has not scrolled up.
  atBottom: boolean;
}

/// How many rows to push the screen down. Never negative.
export function anchorOffset(i: AnchorInputs): number {
  if (i.altBuffer || !i.atBottom) return 0;
  return Math.max(0, i.rows - 1 - Math.max(i.cursorY, i.lastContentRow));
}

/// The slice of xterm's `IBufferLine` / `IBuffer` this module reads. Kept
/// structural so both `@xterm/xterm` and `@xterm/headless` buffers satisfy
/// it, and a test can count calls.
export interface AnchorLine {
  translateToString(trimRight?: boolean): string;
}

export interface AnchorBuffer {
  readonly type: "normal" | "alternate";
  readonly baseY: number;
  readonly viewportY: number;
  readonly cursorY: number;
  getLine(y: number): AnchorLine | undefined;
}

/// Lowest viewport row strictly below `floor` that holds any text, or `-1`.
///
/// Scans bottom-up and stops at the first hit, so a full screen costs one
/// `getLine`. Rows at or above `floor` are never read. A row of
/// background-coloured spaces counts as blank (`trimRight`).
export function lastContentRow(buf: AnchorBuffer, rows: number, floor: number): number {
  for (let y = rows - 1; y > floor; y--) {
    const line = buf.getLine(buf.baseY + y);
    if (line !== undefined && line.translateToString(true).length > 0) return y;
  }
  return -1;
}

/// `anchorOffset` for a live buffer. The cheap checks go first so the scan is
/// unreachable in the alternate buffer and while scrolled up. The scan is
/// bounded to the `rows - 1 - cursorY` rows below the cursor: nothing at or
/// above the cursor can change `max(cursorY, lastContentRow)`.
export function bufferAnchorOffset(buf: AnchorBuffer, rows: number): number {
  const altBuffer = buf.type === "alternate";
  const atBottom = buf.viewportY === buf.baseY;
  if (altBuffer || !atBottom) return 0;
  const cursorY = buf.cursorY;
  return anchorOffset({
    rows,
    cursorY,
    lastContentRow: lastContentRow(buf, rows, cursorY),
    altBuffer,
    atBottom,
  });
}

/// CSS `transform` for an offset of `offset` rows. `screenHeightPx` is
/// `.xterm-screen`'s inline height, which the renderer sets to exactly
/// `rows x cell height`, so dividing gives the cell height without touching
/// xterm internals. Empty string (no transform) for a zero offset or before
/// the renderer has sized the screen.
export function anchorTransform(offset: number, screenHeightPx: number, rows: number): string {
  if (offset <= 0 || rows <= 0 || !(screenHeightPx > 0)) return "";
  return `translateY(${(offset * screenHeightPx) / rows}px)`;
}
