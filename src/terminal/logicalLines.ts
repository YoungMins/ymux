// Terminal rows -> logical lines. A row that only continues the one above (the
// terminal soft-wrapped a long line) is joined to it, so a secret or a path the
// wrap cut in two is seen whole by whatever reads the text next (the remote
// server redacts per line). Only xterm's own soft wraps are joined: text that
// an agent's TUI wraps itself (it prints real line breaks) stays split, and
// redaction there is as best effort as it is anywhere. Pure: TerminalPane
// feeds it xterm's rows.

export interface ScreenRow {
  /// The row's text untrimmed (`translateToString(false)`): a space at the
  /// wrap point belongs to the line and must survive the join.
  raw: string;
  /// xterm's `isWrapped`: this row continues the previous one.
  wrapped: boolean;
}

/// One string per logical line, trailing blanks trimmed from each, and no
/// blank lines at the end.
export function logicalLines(rows: readonly ScreenRow[]): string[] {
  const out: string[] = [];
  let cur: string | null = null;
  for (const r of rows) {
    if (cur !== null && r.wrapped) {
      cur += r.raw;
      continue;
    }
    if (cur !== null) out.push(cur.trimEnd());
    cur = r.raw;
  }
  if (cur !== null) out.push(cur.trimEnd());
  while (out.length > 0 && out[out.length - 1].trim() === "") out.pop();
  return out;
}
