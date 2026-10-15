import { describe, expect, it } from "vitest";
import { cleanupTargets, commitsAhead, diffStat, lanesOfRun, mergeCommand, runIds } from "./compareModel";

const lane = (path: string, runId: string) => ({ path, branch: `b-${path}`, base: "abc", runId });
const file = (over = {}) => ({
  path: "a", orig: "", added: 1, deleted: 2, binary: false, untracked: false, counts_unknown: false, ...over,
});

describe("compareModel", () => {
  it("orders runs newest first and filters lanes", () => {
    const ls = [lane("1", "20260101-0900-aaaa"), lane("2", "20260102-0800-bbbb"), lane("3", "20260102-0800-bbbb")];
    expect(runIds(ls)).toEqual(["20260102-0800-bbbb", "20260101-0900-aaaa"]);
    expect(lanesOfRun(ls, "20260102-0800-bbbb").map((l) => l.path)).toEqual(["2", "3"]);
  });
  it("sums a diff and flags uncounted files", () => {
    expect(diffStat([file(), file({ added: 4, deleted: 0 })])).toEqual({ files: 2, added: 5, deleted: 2, partial: false });
    expect(diffStat([file({ binary: true })]).partial).toBe(true);
    expect(diffStat([])).toEqual({ files: 0, added: 0, deleted: 0, partial: false });
  });
  it("counts commits above the base", () => {
    expect(commitsAhead([{ hash: "c" }, { hash: "b" }, { hash: "a" }], "a")).toEqual({ n: 2, more: false });
    expect(commitsAhead([{ hash: "a" }], "a")).toEqual({ n: 0, more: false });
    expect(commitsAhead([{ hash: "c" }, { hash: "b" }], "zzz")).toEqual({ n: 2, more: true });
  });
  it("prefers the backend's exact count over the log estimate", () => {
    expect(commitsAhead([{ hash: "c" }, { hash: "b" }], "zzz", 150)).toEqual({ n: 150, more: false });
    expect(commitsAhead([{ hash: "c" }, { hash: "b" }], "zzz", 0)).toEqual({ n: 0, more: false });
    expect(commitsAhead([{ hash: "c" }, { hash: "b" }], "zzz", null)).toEqual({ n: 2, more: true });
    expect(commitsAhead([{ hash: "c" }, { hash: "a" }], "a", -1)).toEqual({ n: 1, more: false });
  });
  it("builds a merge command and quotes odd branch names", () => {
    expect(mergeCommand("fanout/claude-1", "posix")).toBe("git merge --no-ff fanout/claude-1");
    expect(mergeCommand("a;rm -rf", "posix")).toBe("git merge --no-ff 'a;rm -rf'");
    expect(mergeCommand("a$b", "powershell")).toBe("git merge --no-ff 'a$b'");
    expect(mergeCommand("-x", "posix")).toBeNull();
  });
  it("keeps one lane out of the cleanup", () => {
    const ls = [lane("1", "r"), lane("2", "r"), lane("3", "r")];
    expect(cleanupTargets(ls, "2").map((l) => l.path)).toEqual(["1", "3"]);
  });
});
