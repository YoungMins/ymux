import { describe, expect, it } from "vitest";

import {
  MAX_CANDIDATES_PER_ROW,
  findPathCandidates,
  type PathCandidate,
} from "./pathMatch";

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
