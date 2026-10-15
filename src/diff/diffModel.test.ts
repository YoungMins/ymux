import { describe, expect, it } from "vitest";
import { cachedFile, excerpt, fileCacheKey, reanchor, toSideBySide } from "./diffModel";
import type { DiffLine, FileDiff, Hunk } from "./types";
import type { ReviewComment } from "./model-types";

const ctx = (o: number, n: number, text = `c${n}`): DiffLine => ({ kind: "ctx", old_no: o, new_no: n, text, cr: false });
const add = (n: number, text = `a${n}`): DiffLine => ({ kind: "add", old_no: null, new_no: n, text, cr: false });
const del = (o: number, text = `d${o}`): DiffLine => ({ kind: "del", old_no: o, new_no: null, text, cr: false });
const noeol = (): DiffLine => ({ kind: "noeol", old_no: null, new_no: null, text: "\\ No newline at end of file", cr: false });
const hunk = (lines: DiffLine[]): Hunk => ({ old_start: 1, old_lines: 0, new_start: 1, new_lines: 0, header: "", lines });

describe("toSideBySide", () => {
  it("shows context on both sides", () => {
    const c = ctx(1, 1);
    expect(toSideBySide(hunk([c]))).toEqual([{ left: c, right: c }]);
  });

  it("pairs a del run with the add run that follows", () => {
    const d1 = del(1), d2 = del(2), a1 = add(1);
    expect(toSideBySide(hunk([d1, d2, a1]))).toEqual([{ left: d1, right: a1 }, { left: d2 }]);
  });

  it("leaves extra adds on the right alone", () => {
    const d1 = del(1), a1 = add(1), a2 = add(2);
    expect(toSideBySide(hunk([d1, a1, a2]))).toEqual([{ left: d1, right: a1 }, { right: a2 }]);
  });

  it("does not pair across context", () => {
    const d1 = del(1), c = ctx(2, 1), a1 = add(2);
    expect(toSideBySide(hunk([d1, c, a1]))).toEqual([{ left: d1 }, { left: c, right: c }, { right: a1 }]);
  });

  it("puts adds with no del on the right", () => {
    const a1 = add(1);
    expect(toSideBySide(hunk([a1]))).toEqual([{ right: a1 }]);
  });

  it("puts a noeol marker on the side of the line before it", () => {
    const n = noeol();
    const a1 = add(1);
    expect(toSideBySide(hunk([a1, n]))).toEqual([{ right: a1 }, { right: n }]);
    const d1 = del(1);
    expect(toSideBySide(hunk([d1, n]))).toEqual([{ left: d1 }, { left: n }]);
    const c = ctx(1, 1);
    expect(toSideBySide(hunk([c, n]))).toEqual([{ left: c, right: c }, { left: n, right: n }]);
  });

  it("handles an empty hunk", () => {
    expect(toSideBySide(hunk([]))).toEqual([]);
  });
});

describe("excerpt", () => {
  const lines = Array.from({ length: 20 }, (_, i) => ctx(i + 1, i + 1));

  it("returns the line with ctx lines around it", () => {
    expect(excerpt(hunk(lines), 10).map((l) => l.new_no)).toEqual([7, 8, 9, 10, 11, 12, 13]);
  });

  it("clips at the hunk edges", () => {
    expect(excerpt(hunk(lines), 1).map((l) => l.new_no)).toEqual([1, 2, 3, 4]);
  });

  it("covers an end line", () => {
    const out = excerpt(hunk(lines), 5, 1, 40, { endLine: 8 });
    expect(out.map((l) => l.new_no)).toEqual([4, 5, 6, 7, 8, 9]);
  });

  it("caps at max lines, keeping the start", () => {
    const out = excerpt(hunk(lines), 2, 3, 4, { endLine: 20 });
    expect(out).toHaveLength(4);
    expect(out[0].new_no).toBe(1);
  });

  it("addresses the old side by old_no", () => {
    const h = hunk([ctx(1, 1), del(2), add(2), ctx(3, 3)]);
    expect(excerpt(h, 2, 0, 40, { side: "old" }).map((l) => l.kind)).toEqual(["del"]);
  });

  it("returns nothing when the line is not in the hunk", () => {
    expect(excerpt(hunk(lines), 99)).toEqual([]);
  });
});

describe("reanchor", () => {
  const base: ReviewComment = { id: "1", path: "f.ts", side: "new", line: 10, text: "hm", lineText: "target" };
  const hunks = (ls: DiffLine[]) => [hunk(ls)];

  it("keeps a comment whose line is unchanged", () => {
    const out = reanchor(base, hunks([ctx(10, 10, "target")]));
    expect(out.line).toBe(10);
    expect(out.stale).toBeFalsy();
  });

  it("follows the text when the line moved", () => {
    const out = reanchor({ ...base, endLine: 12 }, hunks([add(15, "target")]));
    expect(out.line).toBe(15);
    expect(out.endLine).toBe(17);
    expect(out.stale).toBeFalsy();
  });

  it("picks the nearest of several matches", () => {
    expect(reanchor(base, hunks([add(5, "target"), add(12, "target")])).line).toBe(12);
  });

  it("is stale beyond 50 lines", () => {
    const out = reanchor(base, hunks([add(61, "target")]));
    expect(out.stale).toBe(true);
    expect(out.line).toBe(10);
  });

  it("is stale when the text is gone", () => {
    expect(reanchor(base, hunks([add(10, "other")])).stale).toBe(true);
  });

  it("matches the old side on del and ctx lines only", () => {
    const c: ReviewComment = { ...base, side: "old", line: 4, lineText: "x" };
    expect(reanchor(c, hunks([del(6, "x")])).line).toBe(6);
    expect(reanchor(c, hunks([add(4, "x")])).stale).toBe(true);
  });

  it("clears an earlier stale flag on a match", () => {
    expect(reanchor({ ...base, stale: true }, hunks([ctx(10, 10, "target")])).stale).toBeFalsy();
  });
});

describe("cachedFile", () => {
  const sum = { base_rev: "abc" };
  const fd = { hunks: [] } as unknown as FileDiff;
  it("finds a file stored under the keyed form", () => {
    const cache = new Map([[fileCacheKey(sum, false, "a.ts"), fd]]);
    expect(cachedFile(cache, sum, false, "a.ts")).toBe(fd);
  });
  it("misses a bare-path key, another base or another whitespace mode", () => {
    const cache = new Map([["a.ts", fd]]);
    expect(cachedFile(cache, sum, false, "a.ts")).toBeUndefined();
    const keyed = new Map([[fileCacheKey(sum, false, "a.ts"), fd]]);
    expect(cachedFile(keyed, { base_rev: "def" }, false, "a.ts")).toBeUndefined();
    expect(cachedFile(keyed, sum, true, "a.ts")).toBeUndefined();
  });
  it("is undefined without a summary or path", () => {
    const cache = new Map([[fileCacheKey(sum, false, "a.ts"), fd]]);
    expect(cachedFile(cache, null, false, "a.ts")).toBeUndefined();
    expect(cachedFile(cache, sum, false, null)).toBeUndefined();
  });
});
