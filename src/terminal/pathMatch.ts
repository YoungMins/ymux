// Extracts filesystem-path *candidates* from one line of terminal text.
//
// Pure: no DOM, no IPC, no filesystem. It answers "which substrings of this
// line are shaped like a path, and where do they start and end" — nothing
// more. Whether a candidate is a real file is decided later, by an existence
// probe (`pathProbe.ts`), because only the backend can answer that and only
// the pane knows the cwd a relative candidate resolves against.
//
// The split matters: a matcher that also guessed at existence would have to
// be conservative enough to avoid false links, and would then miss the
// awkward-but-real paths (spaces, `:line:col`, punctuation glued on by the
// surrounding prose) that this one is written to catch. Here we over-produce
// on purpose and let the filesystem veto.

/// Upper bound on candidates returned for a single row. xterm asks for links
/// per hovered row, so this bounds the IPC fan-out of one hover. A row of
/// prose full of slashes ("and/or", "24/09/2026", "n/a") is exactly the case
/// this stops from turning into a burst of stat calls.
export const MAX_CANDIDATES_PER_ROW = 24;

export interface PathCandidate {
  /// The path text alone: quotes, wrapping punctuation and any `:line:col`
  /// suffix removed. This is what gets probed and opened.
  text: string;
  /// Index of `text` within the source line, inclusive.
  start: number;
  /// Index one past the end of `text` within the source line.
  end: number;
  /// 1-based line number from a `path:line` / `path:line:col` suffix.
  /// Recorded so the UI can show it; it is *not* passed to the opener.
  line?: number;
  /// 1-based column from a `path:line:col` suffix.
  col?: number;
}

/// Characters that may be glued to the front of a path by the surrounding
/// text and are never part of one in practice. Backtick and pipe are here
/// because coding agents wrap paths in them constantly.
const LEAD_STRIP = "([{<\"'`|*";
/// Same idea for the tail. `:` is included because `file.rs:` is a common
/// prefix form in compiler output; a real `:line:col` suffix is recovered
/// afterwards, from the already-trimmed token.
const TRAIL_STRIP = ")]}>,.;:!?\"'`|*";

/// A scheme-qualified URL. Used twice: to blank out regions of the line that
/// belong to the URL link provider (which is registered first and wins
/// anyway), and to reject a candidate that is itself a URL.
const URL_RE = /[A-Za-z][A-Za-z0-9+.-]*:\/\/\S+/g;

/// C0 controls and DEL. A candidate carrying one of these is not a path we
/// are willing to hand to the OS opener, whatever the filesystem says.
const CONTROL_RE = /[\u0000-\u001f\u007f]/;

/// `:line` or `:line:col`, but only where the run of digits is followed by
/// end-of-token or another colon. The lookahead is what keeps a directory
/// that genuinely contains a colon-digit segment (`/tmp/a:1/b`) intact while
/// still cutting grep's `src/main.ts:12:const x` down to the file.
const LINE_COL_RE = /:(\d+)(?::(\d+))?(?=$|:)/;

/// Quote characters that can wrap a path containing spaces.
const QUOTES = "\"'`";

function isSpace(ch: string): boolean {
  return ch === " " || ch === "\t";
}

/// Is `ch` a plausible character *before* an opening quote? Requiring one
/// stops an apostrophe in prose ("don't use src/main.ts") from opening a
/// quoted region that swallows the real path that follows it.
function opensQuote(prev: string | undefined): boolean {
  return prev === undefined || isSpace(prev) || "([{<=:,".includes(prev);
}

/// Every character that is *only* structure, never content. A candidate made
/// entirely of these (`/`, `//`, `./`, `..\`) names a directory, but linking
/// it is pure noise, so it is rejected.
const STRUCTURAL = new Set(["/", "\\", "."]);

/// Turn one whitespace-delimited token into a candidate, or `null` when it
/// is not path-shaped. `offset` is the token's index in the source line.
function refineToken(token: string, offset: number): PathCandidate | null {
  let start = 0;
  let end = token.length;
  while (start < end && LEAD_STRIP.includes(token[start]!)) start++;
  while (end > start && TRAIL_STRIP.includes(token[end - 1]!)) end--;
  if (end <= start) return null;

  let text = token.slice(start, end);
  let line: number | undefined;
  let col: number | undefined;

  const m = LINE_COL_RE.exec(text);
  if (m && m.index > 0) {
    line = Number(m[1]);
    if (m[2] !== undefined) col = Number(m[2]);
    end = start + m.index;
    text = token.slice(start, end);
    // The cut can expose punctuation that was hiding behind the suffix, as
    // in `see src/main.ts:12,` once the `,` and then `:12` have gone.
    while (end > start && TRAIL_STRIP.includes(token[end - 1]!)) {
      end--;
      text = token.slice(start, end);
    }
  }

  if (!text) return null;
  if (CONTROL_RE.test(text)) return null;
  // A path has to have a separator. This is the rule that keeps bare words
  // ("README", "main.ts", "build") out, whether or not a file of that name
  // happens to sit in the cwd.
  if (!text.includes("/") && !text.includes("\\")) return null;
  if (![...text].some((ch) => !STRUCTURAL.has(ch))) return null;
  // Leave URLs to the web-links provider; it is registered first, so xterm
  // would drop ours on overlap anyway, but a bare `ftp://host/x` that the
  // URL provider skips should not become a path link either.
  if (/^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(text)) return null;

  return { text, start: offset + start, end: offset + end, line, col };
}

/// Find every path-shaped substring of `line`.
///
/// Candidates may overlap: a quoted region yields both the whole quoted
/// string (the path-with-spaces reading) and the tokens inside it (the
/// path-in-prose reading). The caller probes both and keeps whichever turns
/// out to exist — see `pathProbe.ts`'s `resolveOverlaps`.
export function findPathCandidates(
  line: string,
  limit: number = MAX_CANDIDATES_PER_ROW,
): PathCandidate[] {
  const urlRanges: Array<[number, number]> = [];
  URL_RE.lastIndex = 0;
  for (let m = URL_RE.exec(line); m; m = URL_RE.exec(line)) {
    urlRanges.push([m.index, m.index + m[0].length]);
  }
  const inUrl = (c: PathCandidate): boolean =>
    urlRanges.some(([s, e]) => c.start < e && c.end > s);

  const out: PathCandidate[] = [];
  const seen = new Set<string>();
  const push = (c: PathCandidate | null): void => {
    if (!c || inUrl(c)) return;
    const key = `${c.start}:${c.end}`;
    if (seen.has(key)) return;
    seen.add(key);
    out.push(c);
  };

  // Tokens of a region of the line, pushed with a base offset. Used for the
  // line itself and again for the inside of each quoted region.
  const scanTokens = (text: string, base: number): void => {
    let i = 0;
    while (i < text.length) {
      if (isSpace(text[i]!)) {
        i++;
        continue;
      }
      let j = i;
      while (j < text.length && !isSpace(text[j]!)) j++;
      push(refineToken(text.slice(i, j), base + i));
      i = j;
    }
  };

  let i = 0;
  while (i < line.length && out.length < limit) {
    const ch = line[i]!;
    if (isSpace(ch)) {
      i++;
      continue;
    }
    if (QUOTES.includes(ch) && opensQuote(line[i - 1])) {
      const close = line.indexOf(ch, i + 1);
      if (close > i + 1) {
        const inner = line.slice(i + 1, close);
        // The whole quoted string as one path (this is how a path with
        // spaces survives), plus its tokens (this is how a path mentioned
        // inside a quoted sentence survives).
        push(refineToken(inner, i + 1));
        if (inner.includes(" ")) scanTokens(inner, i + 1);
        i = close + 1;
        continue;
      }
    }
    let j = i;
    while (j < line.length && !isSpace(line[j]!)) j++;
    push(refineToken(line.slice(i, j), i));
    i = j;
  }

  return out.slice(0, limit);
}
