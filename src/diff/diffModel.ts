import type { DiffLine, DiffSummary, FileDiff, Hunk } from "./types";
import type { CommentSide, ReviewComment } from "./model-types";

export interface SideRow {
  left?: DiffLine;
  right?: DiffLine;
}

/// Lay a hunk out as two columns. A run of deletions is paired row by row with
/// the run of additions that directly follows it; context spans both columns.
/// A "no newline" marker stays on the side of the line it annotates.
export function toSideBySide(hunk: Hunk): SideRow[] {
  const rows: SideRow[] = [];
  let dels: DiffLine[] = [];
  let adds: DiffLine[] = [];
  let last: DiffLine["kind"] | null = null;
  const flush = () => {
    const n = Math.max(dels.length, adds.length);
    for (let i = 0; i < n; i++) {
      const row: SideRow = {};
      if (i < dels.length) row.left = dels[i];
      if (i < adds.length) row.right = adds[i];
      rows.push(row);
    }
    dels = [];
    adds = [];
  };
  for (const l of hunk.lines) {
    if (l.kind === "del") {
      if (adds.length) flush();
      dels.push(l);
    } else if (l.kind === "add") {
      adds.push(l);
    } else {
      flush();
      if (l.kind === "ctx") rows.push({ left: l, right: l });
      else if (last === "del") rows.push({ left: l });
      else if (last === "add") rows.push({ right: l });
      else rows.push({ left: l, right: l });
    }
    last = l.kind;
  }
  flush();
  return rows;
}

export function numberOn(l: DiffLine, side: CommentSide): number | null {
  if (l.kind === "noeol") return null;
  if (side === "old") return l.kind === "add" ? null : l.old_no;
  return l.kind === "del" ? null : l.new_no;
}

/// The lines around `line` (through `opts.endLine`) on `opts.side` (default
/// "new"), `ctx` lines of padding either way, capped at `max` lines from the
/// start. Empty when the line is not in the hunk.
export function excerpt(
  hunk: Hunk,
  line: number,
  ctx = 3,
  max = 40,
  opts: { side?: CommentSide; endLine?: number } = {},
): DiffLine[] {
  const side = opts.side ?? "new";
  const end = opts.endLine ?? line;
  let first = -1;
  let last = -1;
  hunk.lines.forEach((l, i) => {
    const n = numberOn(l, side);
    if (n !== null && n >= line && n <= end) {
      if (first < 0) first = i;
      last = i;
    }
  });
  if (first < 0) return [];
  const from = Math.max(0, first - ctx);
  const to = Math.min(hunk.lines.length, last + ctx + 1);
  return hunk.lines.slice(from, Math.min(to, from + max));
}

export const REANCHOR_RANGE = 50;

/// Re-find a comment's line in a file's fresh hunks: same side, same text, the
/// nearest within +-50 lines of where it was. The caller passes the hunks of
/// the comment's own file. No match leaves the comment where it is, marked stale.
export function reanchor(comment: ReviewComment, newHunks: Hunk[]): ReviewComment {
  let best: number | null = null;
  for (const h of newHunks) {
    for (const l of h.lines) {
      const n = numberOn(l, comment.side);
      if (n === null || l.text !== comment.lineText) continue;
      const d = Math.abs(n - comment.line);
      if (d > REANCHOR_RANGE) continue;
      if (best === null || d < Math.abs(best - comment.line)) best = n;
    }
  }
  if (best === null) return { ...comment, stale: true };
  const { stale: _stale, ...rest } = comment;
  const out: ReviewComment = { ...rest, line: best };
  if (comment.endLine !== undefined) out.endLine = comment.endLine + (best - comment.line);
  return out;
}

/// Cache key of a file's diff: the base commit and the whitespace mode it was
/// read with, plus the path.
export function fileCacheKey(sum: Pick<DiffSummary, "base_rev">, ignoreCr: boolean, path: string): string {
  return `${sum.base_rev}|${ignoreCr ? 1 : 0}|${path}`;
}

/// The cached diff of `path` for the current summary, or undefined.
export function cachedFile(
  cache: ReadonlyMap<string, FileDiff>,
  sum: Pick<DiffSummary, "base_rev"> | null,
  ignoreCr: boolean,
  path: string | null,
): FileDiff | undefined {
  return sum && path ? cache.get(fileCacheKey(sum, ignoreCr, path)) : undefined;
}
