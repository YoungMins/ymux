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
/// because coding agents wrap paths in them constantly; `⎿`, `●` and `•`
/// are the result/bullet glyphs Claude Code prints right before a path.
/// An opening bracket stripped here is put back when the path turns out to
/// close it (`(group)/page.tsx`) — see the end of `refineToken`.
const LEAD_STRIP = "([{<\"'`|*⎿●•";
/// Same idea for the tail. `:` is included because `file.rs:` is a common
/// prefix form in compiler output; a real `:line:col` suffix is recovered
/// afterwards, from the already-trimmed token. A closing `)`, `]` or `}` is
/// only stripped while it is *unbalanced* in what remains, so `src/foo(old)`
/// keeps its `)` and `(src/a.md)` loses it — see `peelsAtTail`.
const TRAIL_STRIP = ")]}>,.;:!?\"'`|*";

/// Bracket pairs whose balance decides whether a bracket is part of the
/// path or wrapping punctuation. Paths do contain them — `Program Files
/// (x86)`, Next.js route groups `app/(group)/page.tsx`, `src/foo(old)` — but
/// balanced, so an unmatched one is taken to belong to the prose. `<>` is
/// not here: illegal in Windows names and never balanced inside a POSIX one
/// in practice, so it is always stripped.
const CLOSER_OF: Readonly<Record<string, string>> = { "(": ")", "[": "]", "{": "}" };
const OPENER_OF: Readonly<Record<string, string>> = { ")": "(", "]": "[", "}": "{" };

/// Does `s` close an `open`/`close` pair it never opened? True if, scanning
/// left to right, the depth ever goes negative.
function closesUnopened(s: string, open: string, close: string): boolean {
  let depth = 0;
  for (const ch of s) {
    if (ch === open) depth++;
    else if (ch === close && --depth < 0) return true;
  }
  return false;
}

/// Should the last character of `s` be peeled off as trailing punctuation?
/// A closing bracket only when `s` has more of it than of its opener.
function peelsAtTail(s: string): boolean {
  const ch = s[s.length - 1];
  if (ch === undefined || !TRAIL_STRIP.includes(ch)) return false;
  const open = OPENER_OF[ch];
  if (open === undefined) return true;
  let opens = 0;
  let closes = 0;
  for (const c of s) {
    if (c === open) opens++;
    else if (c === ch) closes++;
  }
  return closes > opens;
}

/// A file name whose extension has at least one letter: `main.rs`,
/// `file.ts`, `.env`. Consulted only for a separator-less token that came
/// with a `:line` / `(line)` suffix — the suffix is what makes `file.ts:10:5`
/// path-shaped where a bare `file.ts` is not — and it keeps `127.0.0.1:80`,
/// `v1.2:3` and `12:30:45` out.
const BARE_FILE_RE = /\.[A-Za-z0-9_-]*[A-Za-z][A-Za-z0-9_-]*$/;

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

/// `(line)` or `(line,col)` glued to the end of a path — how tsc, MSVC and
/// most .NET tooling report a position. Anchored at the end because a
/// parenthesised segment anywhere else is far more likely to be part of the
/// name (`src/foo(old)/a.ts`) than a position.
const PAREN_LINE_COL_RE = /\((\d+)(?:,(\d+))?\)$/;

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
/// `inPathContext` marks a token lifted out of a `Tool(…)` call or a
/// markdown link target, where a separator-less `README.md` is a path.
function refineToken(
  token: string,
  offset: number,
  inPathContext = false,
): PathCandidate | null {
  let start = 0;
  let end = token.length;
  while (start < end && LEAD_STRIP.includes(token[start]!)) start++;
  if (end <= start) return null;

  let line: number | undefined;
  let col: number | undefined;

  // Peel the tail one layer at a time. The two orders have to interleave:
  // `src/a.ts(12,5))` needs a `)` stripped before the position suffix is
  // visible, and `see src/a.ts(12,5).` needs the `.` stripped first.
  for (;;) {
    const paren = PAREN_LINE_COL_RE.exec(token.slice(start, end));
    if (paren && paren.index > 0) {
      line = Number(paren[1]);
      if (paren[2] !== undefined) col = Number(paren[2]);
      end = start + paren.index;
      continue;
    }
    if (end > start && peelsAtTail(token.slice(start, end))) {
      end--;
      continue;
    }
    break;
  }

  let text = token.slice(start, end);
  if (line === undefined) {
    const m = LINE_COL_RE.exec(text);
    if (m && m.index > 0) {
      line = Number(m[1]);
      if (m[2] !== undefined) col = Number(m[2]);
      end = start + m.index;
      // The cut can expose punctuation that was hiding behind the suffix,
      // as in `see src/main.ts:12,` once the `,` and then `:12` have gone.
      while (end > start && peelsAtTail(token.slice(start, end))) end--;
      text = token.slice(start, end);
    }
  }

  // Put back an opening bracket the lead strip took if the path closes it:
  // `(group)/page.tsx` is a Next.js route group, not a wrapped `group)/…`.
  while (start > 0) {
    const open = token[start - 1]!;
    const close = CLOSER_OF[open];
    if (close === undefined || !closesUnopened(text, open, close)) break;
    start--;
    text = token.slice(start, end);
  }

  if (!text) return null;
  if (CONTROL_RE.test(text)) return null;
  // A path has to have a separator. This is the rule that keeps bare words
  // ("README", "main.ts", "build") out, whether or not a file of that name
  // happens to sit in the cwd. The one exception is a file name that came
  // with a position suffix (`file.ts:10:5`, `main.rs(42,7)`): compilers and
  // agents print those relative to the cwd, and the suffix is what makes the
  // token path-shaped. The same goes for a `Tool(README.md)` argument or a
  // `[x](README.md)` target (`inPathContext`). Either still only links if it
  // exists in the cwd.
  const hasSeparator = text.includes("/") || text.includes("\\");
  const bareOk = (line !== undefined || inPathContext) && BARE_FILE_RE.test(text);
  if (!hasSeparator && !bareOk) return null;
  if (![...text].some((ch) => !STRUCTURAL.has(ch))) return null;
  // Leave URLs to the web-links provider; it is registered first, so xterm
  // would drop ours on overlap anyway, but a bare `ftp://host/x` that the
  // URL provider skips should not become a path link either.
  if (/^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(text)) return null;

  return { text, start: offset + start, end: offset + end, line, col };
}

/// Every reading of one token: the whole token, plus the path inside it
/// when the token is `label(path)` or a markdown link `[label](path)`.
///
/// - Markdown link: whatever follows the first `](` is the target, whatever
///   the label says (Claude Code writes `[src/a.md](src/a.md)` constantly).
/// - Call form: `Read(src/a.ts)`, `Update(D:\x\README.md)` — the first `(`
///   whose prefix is a non-empty word with no separator, and which is closed
///   by the token's final `)` (trailing prose punctuation aside). A prefix
///   with a separator is a directory name instead (`src/foo(old)/a.ts`), and
///   yields nothing extra.
///
/// Every reading is emitted and the existence probe picks: longest existing
/// wins (`resolveOverlaps`), so a real file named `foo(bar)` still beats a
/// `bar` inside it.
function refineAll(token: string, offset: number): Array<PathCandidate | null> {
  const out = [refineToken(token, offset)];
  const md = token.indexOf("](");
  if (md >= 0) out.push(refineToken(token.slice(md + 2), offset + md + 2, true));

  let lead = 0;
  while (lead < token.length && LEAD_STRIP.includes(token[lead]!)) lead++;
  const open = token.indexOf("(", lead);
  if (open > lead && !/[/\\\s]/.test(token.slice(lead, open))) {
    let tail = token.length;
    while (tail > open && token[tail - 1] !== ")" && TRAIL_STRIP.includes(token[tail - 1]!)) {
      tail--;
    }
    let depth = 0;
    let closeAt = -1;
    for (let k = open; k < tail; k++) {
      if (token[k] === "(") depth++;
      else if (token[k] === ")" && --depth === 0) {
        closeAt = k;
        break;
      }
    }
    if (closeAt === tail - 1 && closeAt > open + 1) {
      out.push(refineToken(token.slice(open + 1, closeAt), offset + open + 1, true));
    }
  }
  return out;
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
      for (const c of refineAll(text.slice(i, j), base + i)) push(c);
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
        for (const c of refineAll(inner, i + 1)) push(c);
        if (inner.includes(" ")) scanTokens(inner, i + 1);
        i = close + 1;
        continue;
      }
    }
    let j = i;
    while (j < line.length && !isSpace(line[j]!)) j++;
    for (const c of refineAll(line.slice(i, j), i)) push(c);
    i = j;
  }

  return out.slice(0, limit);
}
