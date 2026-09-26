import { describe, expect, it } from "vitest";

import {
  MAX_CANDIDATES_PER_ROW,
  findPathCandidates,
  type PathCandidate,
} from "./pathMatch";
import { resolveOverlaps } from "./pathProbe";

/// The candidate texts, in the order they were found.
function texts(line: string): string[] {
  return findPathCandidates(line).map((c) => c.text);
}

/// The single candidate for a line that should produce exactly one.
function only(line: string): PathCandidate {
  const found = findPathCandidates(line);
  expect(found, `expected one candidate in ${JSON.stringify(line)}`).toHaveLength(1);
  return found[0]!;
}

describe("findPathCandidates — absolute paths", () => {
  it("matches a Windows drive path with backslashes", () => {
    expect(only("see C:\\Users\\nanyo\\ymux").text).toBe("C:\\Users\\nanyo\\ymux");
  });

  it("matches a Windows drive path with forward slashes", () => {
    expect(only("D:/Git/ymux/src/main.ts").text).toBe("D:/Git/ymux/src/main.ts");
  });

  it("matches a UNC path", () => {
    expect(only("copy from \\\\server\\share\\build.log").text).toBe(
      "\\\\server\\share\\build.log",
    );
  });

  it("matches a POSIX absolute path", () => {
    expect(only("loading /usr/lib/x86_64/libc.so").text).toBe("/usr/lib/x86_64/libc.so");
  });

  it("matches a tilde path", () => {
    expect(only("edit ~/.claude/settings.json").text).toBe("~/.claude/settings.json");
  });
});

describe("findPathCandidates — relative paths", () => {
  it("matches a bare relative path with a separator", () => {
    expect(only("src/main.ts").text).toBe("src/main.ts");
  });

  it("matches a dot-slash relative path", () => {
    expect(only("run ./scripts/test.sh now").text).toBe("./scripts/test.sh");
  });

  it("matches a parent-relative path", () => {
    expect(only("../crates/ypath/src/lib.rs").text).toBe("../crates/ypath/src/lib.rs");
  });

  it("records the offsets of the matched text", () => {
    const c = only("run ./scripts/test.sh now");
    expect("run ./scripts/test.sh now".slice(c.start, c.end)).toBe(c.text);
    expect(c.start).toBe(4);
  });
});

describe("findPathCandidates — path:line:col", () => {
  it("splits a path:line suffix off", () => {
    const c = only("error at src/main.ts:42");
    expect(c.text).toBe("src/main.ts");
    expect(c.line).toBe(42);
    expect(c.col).toBeUndefined();
  });

  it("splits a path:line:col suffix off", () => {
    const c = only("src-tauri/src/commands.rs:271:8: warning");
    expect(c.text).toBe("src-tauri/src/commands.rs");
    expect(c.line).toBe(271);
    expect(c.col).toBe(8);
  });

  it("handles grep output where the match text follows the line number", () => {
    // rg prints `path:line:<matched text>` with no space before the text.
    const c = findPathCandidates("src/main.ts:12:const x = 1;")[0]!;
    expect(c.text).toBe("src/main.ts");
    expect(c.line).toBe(12);
    expect(c.col).toBeUndefined();
  });

  it("keeps a Windows drive letter out of the line-number cut", () => {
    const c = only("C:\\Git\\ymux\\src\\main.ts:10:5");
    expect(c.text).toBe("C:\\Git\\ymux\\src\\main.ts");
    expect(c.line).toBe(10);
    expect(c.col).toBe(5);
  });

  it("does not cut a colon-digit segment in the middle of a path", () => {
    // The digits are followed by `/`, not end-of-token or another colon.
    const c = only("/tmp/a:1/b");
    expect(c.text).toBe("/tmp/a:1/b");
    expect(c.line).toBeUndefined();
  });

  it("drops a trailing colon left by compiler output", () => {
    const c = only("src/main.ts:12:");
    expect(c.text).toBe("src/main.ts");
    expect(c.line).toBe(12);
  });
});

describe("findPathCandidates — path(line,col)", () => {
  it("splits the tsc / MSVC position form off", () => {
    // This is what `npx tsc --noEmit` prints, so it matters here in
    // particular.
    const c = only("src/terminal/TerminalPane.ts(30,3): error TS6133");
    expect(c.text).toBe("src/terminal/TerminalPane.ts");
    expect(c.line).toBe(30);
    expect(c.col).toBe(3);
  });

  it("accepts a line with no column", () => {
    const c = only("src/main.ts(42): warning");
    expect(c.text).toBe("src/main.ts");
    expect(c.line).toBe(42);
    expect(c.col).toBeUndefined();
  });

  it("peels the position form out of wrapping parentheses", () => {
    const c = only("at foo (src/main.ts(12,5))");
    expect(c.text).toBe("src/main.ts");
    expect(c.line).toBe(12);
    expect(c.col).toBe(5);
  });

  it("strips punctuation that follows the position form", () => {
    const c = only("see src/main.ts(12,5).");
    expect(c.text).toBe("src/main.ts");
    expect(c.line).toBe(12);
  });

  it("leaves a parenthesised directory name alone", () => {
    expect(only("src/foo(old)/a.ts").text).toBe("src/foo(old)/a.ts");
  });
});

describe("findPathCandidates — surrounding punctuation", () => {
  it("strips a trailing comma", () => {
    expect(only("edit src/main.ts, then build").text).toBe("src/main.ts");
  });

  it("strips a trailing period", () => {
    expect(only("it lives in src/types.ts.").text).toBe("src/types.ts");
  });

  it("strips wrapping parentheses", () => {
    expect(only("(see ./docs/readme.md)").text).toBe("./docs/readme.md");
  });

  it("strips parentheses around a path:line:col", () => {
    const c = only("at foo (src/main.ts:12:3)");
    expect(c.text).toBe("src/main.ts");
    expect(c.line).toBe(12);
    expect(c.col).toBe(3);
  });

  it("strips backticks, which agent output wraps paths in", () => {
    expect(only("open `src/terminal/ime.ts` next").text).toBe("src/terminal/ime.ts");
  });

  it("strips a trailing semicolon and bang", () => {
    expect(texts("check src/a.ts; then src/b.ts!")).toEqual(["src/a.ts", "src/b.ts"]);
  });

  it("strips bullet markers", () => {
    expect(only("* src/style.css").text).toBe("src/style.css");
  });
});

describe("findPathCandidates — quoted paths", () => {
  it("matches a double-quoted path containing spaces", () => {
    const found = findPathCandidates('open "C:\\Program Files\\Git\\bin" now');
    expect(found.map((c) => c.text)).toContain("C:\\Program Files\\Git\\bin");
  });

  it("matches a single-quoted path containing spaces", () => {
    const found = findPathCandidates("ls '/Users/me/My Documents/notes'");
    expect(found.map((c) => c.text)).toContain("/Users/me/My Documents/notes");
  });

  it("also yields the tokens inside a quoted region", () => {
    // Whichever of the two readings exists on disk wins later.
    const found = findPathCandidates('"src/a.ts and src/b.ts"');
    expect(found.map((c) => c.text)).toContain("src/a.ts");
    expect(found.map((c) => c.text)).toContain("src/b.ts");
  });

  it("does not let an apostrophe in prose swallow the path after it", () => {
    // A naive quote scan would treat `'t use src/main.ts, it'` as quoted and
    // never look inside it as bare tokens.
    expect(texts("don't use src/main.ts, it's broken")).toContain("src/main.ts");
  });

  it("keeps a quoted path with no spaces as a single candidate", () => {
    expect(texts('"src/main.ts"')).toEqual(["src/main.ts"]);
  });
});

describe("findPathCandidates — rejections", () => {
  it("rejects bare words with no separator", () => {
    expect(texts("README main.ts Cargo.toml build")).toEqual([]);
  });

  it("rejects a lone separator", () => {
    expect(texts("cd / and then \\ and // and ./ and ../")).toEqual([]);
  });

  it("rejects an http URL, leaving it to the web-links provider", () => {
    expect(texts("open https://example.com/a/b now")).toEqual([]);
  });

  it("rejects a path-shaped fragment inside a URL", () => {
    expect(texts("https://github.com/youngmins/ymux/blob/main/src/main.ts")).toEqual([]);
  });

  it("rejects a non-http scheme URL too", () => {
    expect(texts("ftp://host/pub/file.txt")).toEqual([]);
  });

  it("rejects text carrying a control character", () => {
    expect(texts("src/\u0007main.ts")).toEqual([]);
  });

  it("rejects an empty line", () => {
    expect(texts("")).toEqual([]);
  });

  it("still emits prose fragments with slashes — the probe vetoes them", () => {
    // These are path-shaped; only the filesystem can say they are not paths.
    expect(texts("and/or n/a 24/09/2026")).toEqual(["and/or", "n/a", "24/09/2026"]);
  });
});

describe("findPathCandidates — bounds", () => {
  it("caps the number of candidates per row", () => {
    const line = Array.from({ length: 200 }, (_, i) => `d${i}/f${i}.txt`).join(" ");
    expect(findPathCandidates(line)).toHaveLength(MAX_CANDIDATES_PER_ROW);
  });

  it("honours an explicit lower limit", () => {
    expect(findPathCandidates("a/1 b/2 c/3", 2)).toHaveLength(2);
  });

  it("never returns overlapping duplicates of the same span", () => {
    const found = findPathCandidates('"src/main.ts"');
    const spans = found.map((c) => `${c.start}:${c.end}`);
    expect(new Set(spans).size).toBe(spans.length);
  });
});

describe("findPathCandidates — non-ASCII", () => {
  it("matches a path with Korean components", () => {
    expect(only("열기 /srv/한글/문서.txt").text).toBe("/srv/한글/문서.txt");
  });

  it("matches a quoted path with Korean components and spaces", () => {
    const found = findPathCandidates('"C:\\사용자\\내 문서\\a.txt"');
    expect(found.map((c) => c.text)).toContain("C:\\사용자\\내 문서\\a.txt");
  });
});

/// What a row ends up linking, given which texts exist on disk: the matcher
/// plus the same overlap resolution `PathLinks` applies. Returns each link
/// as the exact slice of `line` it underlines, so a test also pins the span.
function linked(line: string, existing: readonly string[]): string[] {
  const onDisk = new Set(existing);
  return resolveOverlaps(findPathCandidates(line), (c) => onDisk.has(c.text)).map((c) =>
    line.slice(c.start, c.end),
  );
}

describe("findPathCandidates — parentheses and brackets", () => {
  it("links a relative path wrapped in parentheses", () => {
    expect(linked("(src/foo/bar.md)", ["src/foo/bar.md"])).toEqual(["src/foo/bar.md"]);
  });

  it("links a Windows path wrapped in parentheses", () => {
    const line = "(D:\\Git\\ymux\\README.md)";
    expect(linked(line, ["D:\\Git\\ymux\\README.md"])).toEqual(["D:\\Git\\ymux\\README.md"]);
  });

  it("links a path:line in a parenthesised aside", () => {
    const c = findPathCandidates("(see docs/a.md:12)").find((x) => x.text === "docs/a.md");
    expect(c?.line).toBe(12);
  });

  it("links the target of a markdown link, not the label", () => {
    expect(linked("[text](docs/a.md)", ["docs/a.md"])).toEqual(["docs/a.md"]);
  });

  it("links a markdown link target whose label is itself a path", () => {
    // Claude Code writes `[src/a.md](src/a.md)` constantly.
    expect(linked("see [src/a.md](src/a.md).", ["src/a.md"])).toEqual(["src/a.md"]);
  });

  it("links a markdown link target with a line suffix", () => {
    const c = findPathCandidates("[x](docs/a.md:12)").find((x) => x.text === "docs/a.md");
    expect(c?.line).toBe(12);
  });

  it("links a bare file name with a line:col suffix", () => {
    const c = only("(file.ts:10:5)");
    expect(c.text).toBe("file.ts");
    expect(c.line).toBe(10);
    expect(c.col).toBe(5);
  });

  it("links a bare file name in the tsc position form", () => {
    const c = only("main.rs(42,7): error");
    expect(c.text).toBe("main.rs");
    expect(c.line).toBe(42);
  });

  it("still rejects separator-less words with a colon-number suffix but no extension", () => {
    expect(texts("localhost:8080 12:30:45 127.0.0.1:80 v1.2:3")).toEqual([]);
  });

  it("links the argument of a Claude Code tool call", () => {
    expect(linked("● Read(src/a.ts)", ["src/a.ts"])).toEqual(["src/a.ts"]);
    const win = "● Update(D:\\Git\\ymux\\README.md)";
    expect(linked(win, ["D:\\Git\\ymux\\README.md"])).toEqual(["D:\\Git\\ymux\\README.md"]);
  });

  it("links a bare file name that is a tool-call argument or link target", () => {
    // Claude Code prints cwd-relative paths, so a root-level file has no
    // separator; the call / link context is the path signal instead.
    expect(linked("● Update(README.md)", ["README.md"])).toEqual(["README.md"]);
    expect(linked("[CLAUDE.md](CLAUDE.md)", ["CLAUDE.md"])).toEqual(["CLAUDE.md"]);
  });

  it("does not take a call argument without an extension as a file", () => {
    expect(texts("console.log(x) foo(bar) Read(12)")).toEqual([]);
  });

  it("links a tool-call argument that itself contains parentheses", () => {
    expect(linked("Read(src/foo(old)/a.ts)", ["src/foo(old)/a.ts"])).toEqual([
      "src/foo(old)/a.ts",
    ]);
  });

  it("does not split a parenthesised directory into a junk reading", () => {
    expect(texts("src/foo(old)/a.ts")).toEqual(["src/foo(old)/a.ts"]);
  });

  it("keeps a parenthesised last segment", () => {
    // Balanced, so the `)` is part of the name, not wrapping punctuation.
    expect(only("ls src/foo(old)").text).toBe("src/foo(old)");
  });

  it("keeps a Next.js route group", () => {
    expect(only("app/(group)/page.tsx").text).toBe("app/(group)/page.tsx");
    expect(only("(group)/page.tsx").text).toBe("(group)/page.tsx");
    expect(only("edit (app/(group)/page.tsx)").text).toBe("app/(group)/page.tsx");
  });

  it("strips nested wrapping parentheses", () => {
    expect(only("((src/a.md))").text).toBe("src/a.md");
    expect(only("(src/foo(old))").text).toBe("src/foo(old)");
  });

  it("keeps Program Files (x86) intact when the path is quoted", () => {
    const p = "C:\\Program Files (x86)\\foo\\bar.txt";
    expect(linked(`"${p}"`, [p])).toEqual([p]);
    expect(linked(`\`${p}\``, [p])).toEqual([p]);
  });
});

describe("findPathCandidates — agent output wrappers", () => {
  it("strips backticks, quotes and angle brackets", () => {
    expect(only("`src/a.ts`").text).toBe("src/a.ts");
    expect(only('"src/a.ts"').text).toBe("src/a.ts");
    expect(only("'src/a.ts'").text).toBe("src/a.ts");
    expect(only("<src/a.ts>").text).toBe("src/a.ts");
    expect(only("(`src/a.md`)").text).toBe("src/a.md");
    expect(only("**src/a.md**").text).toBe("src/a.md");
  });

  it("strips trailing prose punctuation", () => {
    expect(texts("src/a.ts. src/b.ts, src/c.ts: src/d.ts;")).toEqual([
      "src/a.ts",
      "src/b.ts",
      "src/c.ts",
      "src/d.ts",
    ]);
    const c = only("(src/a.md:12:3).");
    expect([c.text, c.line, c.col]).toEqual(["src/a.md", 12, 3]);
  });

  it("strips Claude Code's result and bullet glyphs", () => {
    expect(only("⎿ src/a.ts").text).toBe("src/a.ts");
    expect(only("⎿src/a.ts").text).toBe("src/a.ts");
    expect(only("●src/a.ts").text).toBe("src/a.ts");
    expect(only("  ⎿  Read src/a.md (12 lines)").text).toBe("src/a.md");
  });
});
