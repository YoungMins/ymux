import { describe, expect, it } from "vitest";
import type { KeepReport } from "../ipc/bridge";
import { t, translationGaps } from "../i18n/i18n";
import type { LaneRef } from "./compareModel";
import {
  DEFAULT_CHOICE,
  cleanupRequest,
  combineReports,
  fileList,
  hasCleanup,
  keepPlan,
  keepRequest,
  losingLanes,
  mergeRequest,
  openSkips,
  pathsToVacate,
  refusal,
  reportLines,
} from "./keepModel";

const lane = (n: string, runId = "run-1"): LaneRef => ({
  path: `/wt/${n}`,
  branch: `fanout/${n}`,
  base: "a".repeat(40),
  runId,
});
const a = lane("a");
const b = lane("b");
const c = lane("c");
const other = lane("z", "run-0");
const all = [a, b, c, other];

describe("keep plan and request", () => {
  it("loses every other lane of the same run only", () => {
    expect(losingLanes(all, a).map((l) => l.branch)).toEqual(["fanout/b", "fanout/c"]);
  });

  it("lists the removals in cleanup mode and none in merge-only mode", () => {
    const p = keepPlan(all, a, "main", "/repo", DEFAULT_CHOICE);
    expect(p.removals).toEqual([
      { branch: "fanout/b", path: "/wt/b" },
      { branch: "fanout/c", path: "/wt/c" },
    ]);
    const m = keepPlan(all, a, "main", "/repo", { ...DEFAULT_CHOICE, mode: "merge-only" });
    expect(m.removals).toEqual([]);
  });

  it("defaults: kept worktree removed, nothing discarded", () => {
    const q = keepRequest("/repo", "main", all, a, DEFAULT_CHOICE, false);
    expect(q).toEqual({
      repo: "/repo",
      baseBranch: "main",
      runId: "run-1",
      keepBranch: "fanout/a",
      others: ["fanout/b", "fanout/c"],
      removeKeptWorktree: true,
      discardDirtyOthers: false,
      dryRun: false,
      cleanupOnly: false,
    });
  });

  it("the two real steps: merge alone, then cleanup-only for what is left", () => {
    const m = mergeRequest("/repo", "main", all, a, DEFAULT_CHOICE);
    expect(m).toMatchObject({ others: [], removeKeptWorktree: false, dryRun: false, cleanupOnly: false });
    const c = cleanupRequest("/repo", "main", all, a, DEFAULT_CHOICE, []);
    expect(c).toMatchObject({
      others: ["fanout/b", "fanout/c"],
      removeKeptWorktree: true,
      discardDirtyOthers: false,
      dryRun: false,
      cleanupOnly: true,
    });
    expect(hasCleanup(c)).toBe(true);
  });

  it("a lane whose terminal stayed open is left out of the cleanup and reported", () => {
    const open = ["/wt/b", "/wt/a"];
    const c = cleanupRequest("/repo", "main", all, a, DEFAULT_CHOICE, open);
    expect(c.others).toEqual(["fanout/c"]);
    expect(c.removeKeptWorktree).toBe(false);
    expect(openSkips(all, a, DEFAULT_CHOICE, open).map((s) => [s.branch, s.reason])).toEqual([
      ["fanout/b", "pane-open"],
      ["fanout/a", "pane-open"],
    ]);
    const none = cleanupRequest("/repo", "main", all, a, { ...DEFAULT_CHOICE, removeKept: false }, [
      "/wt/b",
      "/wt/c",
    ]);
    expect(hasCleanup(none)).toBe(false);
  });

  it("merge-only choice has no cleanup step at all", () => {
    const choice = { ...DEFAULT_CHOICE, mode: "merge-only" as const };
    const c = cleanupRequest("/repo", "main", all, a, choice, []);
    expect(hasCleanup(c)).toBe(false);
    expect(openSkips(all, a, choice, ["/wt/b"])).toEqual([]);
  });

  it("merge-only sends no cleanup at all, whatever the boxes say", () => {
    const q = keepRequest(
      "/repo",
      "main",
      all,
      a,
      { mode: "merge-only", removeKept: true, discardDirty: true },
      true,
    );
    expect(q.others).toEqual([]);
    expect(q.removeKeptWorktree).toBe(false);
    expect(q.discardDirtyOthers).toBe(false);
    expect(q.dryRun).toBe(true);
  });

  it("vacates the losers, and the kept lane only when it goes too", () => {
    expect(pathsToVacate(all, a, DEFAULT_CHOICE)).toEqual(["/wt/b", "/wt/c", "/wt/a"]);
    expect(pathsToVacate(all, a, { ...DEFAULT_CHOICE, removeKept: false })).toEqual([
      "/wt/b",
      "/wt/c",
    ]);
    expect(pathsToVacate(all, a, { ...DEFAULT_CHOICE, mode: "merge-only" })).toEqual([]);
  });
});

const merged = (over: Partial<KeepReport> = {}): KeepReport => ({
  ...refusal(""),
  outcome: "merged",
  merged_commit: "0123456789abcdef",
  ...over,
});

describe("report lines", () => {
  it("clips long file lists", () => {
    const files = Array.from({ length: 13 }, (_, i) => `f${i}`);
    expect(fileList(files)).toBe("f0, f1, f2, f3, f4, f5, f6, f7, f8, f9, … (+3)");
    expect(fileList(["x"])).toBe("x");
  });

  it("a refusal is one line with its reason's key and the files", () => {
    const r = { ...refusal("base-dirty"), files: ["a.txt", "b.txt"] };
    expect(reportLines(r, "main")).toEqual([
      {
        key: "fanoutKeep.refused.base-dirty",
        vars: { files: "a.txt, b.txt", base: "main" },
        tone: "bad",
      },
    ]);
  });

  it("a conflict names the files; a failed abort adds a second warning", () => {
    const r: KeepReport = { ...refusal(""), outcome: "conflict", files: ["f.txt"] };
    expect(reportLines(r, "main").map((l) => l.key)).toEqual(["fanoutKeep.conflict"]);
    const stuck = reportLines({ ...r, left_mid_merge: true, detail: "locked" }, "main");
    expect(stuck.map((l) => l.key)).toEqual(["fanoutKeep.conflict", "fanoutKeep.midMerge"]);
  });

  it("a merged report lists commit, removals, skips and errors in order", () => {
    const r = merged({
      removed_worktrees: [{ branch: "fanout/b", path: "/wt/b" }],
      deleted_branches: ["fanout/b"],
      kept_worktree_removed: true,
      skipped: [{ branch: "fanout/c", path: "/wt/c", reason: "dirty", files: ["w.txt"] }],
      errors: [{ step: "delete-branch", branch: "fanout/d", detail: "nope" }],
    });
    const lines = reportLines(r, "main");
    expect(lines.map((l) => l.key)).toEqual([
      "fanoutKeep.rep.merged",
      "fanoutKeep.rep.removed",
      "fanoutKeep.rep.deleted",
      "fanoutKeep.rep.keptRemoved",
      "fanoutKeep.rep.skipped.dirty",
      "fanoutKeep.rep.error.delete-branch",
    ]);
    expect(lines[0].vars).toEqual({ commit: "01234567", base: "main" });
    expect(lines.map((l) => l.tone)).toEqual(["ok", "ok", "ok", "ok", "warn", "bad"]);
  });

  it("a dry-run conflict names the files and stops there", () => {
    const r: KeepReport = { ...refusal(""), outcome: "would-conflict", files: ["f.txt"] };
    expect(reportLines(r, "main")).toEqual([
      { key: "fanoutKeep.wouldConflict", vars: { files: "f.txt", base: "main" }, tone: "bad" },
    ]);
  });

  it("the two steps read as one report: merge line, then the cleanup's, then the leftovers", () => {
    const cleaned: KeepReport = {
      ...refusal(""),
      outcome: "cleaned",
      removed_worktrees: [{ branch: "fanout/b", path: "/wt/b" }],
      deleted_branches: ["fanout/b"],
      skipped: [{ branch: "fanout/c", path: "/wt/c", reason: "dirty", files: ["w.txt"] }],
    };
    const pane = [{ branch: "fanout/d", path: "/wt/d", reason: "pane-open", files: [] }];
    const r = combineReports(merged(), cleaned, pane, true);
    expect(r.outcome).toBe("merged");
    expect(reportLines(r, "main").map((l) => l.key)).toEqual([
      "fanoutKeep.rep.merged",
      "fanoutKeep.rep.noConflictCheck",
      "fanoutKeep.rep.removed",
      "fanoutKeep.rep.deleted",
      "fanoutKeep.rep.skipped.dirty",
      "fanoutKeep.rep.skipped.pane-open",
    ]);
  });

  it("a cleanup that did not run leaves the merge standing, with one error line", () => {
    const refused = refusal("base-dirty");
    const r = combineReports(merged(), refused, [], false);
    expect(r.outcome).toBe("merged");
    expect(reportLines(r, "main").map((l) => l.key)).toEqual([
      "fanoutKeep.rep.merged",
      "fanoutKeep.rep.error.cleanup",
    ]);
    expect(combineReports(merged(), null, [], false)).toEqual(merged());
  });

  it("every key a report can produce is translated in all 13 languages", () => {
    expect(translationGaps("fanoutKeep.")).toEqual([]);
    const reasons = ["base-branch", "base-dirty", "lane-dirty", "no-commits", "not-fanout", "not-merged"];
    const keys = [
      ...reasons.map((x) => reportLines(refusal(x), "main")[0].key),
      "fanoutKeep.conflict",
      "fanoutKeep.midMerge",
      "fanoutKeep.failed",
      "fanoutKeep.wouldConflict",
      "fanoutKeep.rep.noConflictCheck",
      "fanoutKeep.cleaning",
      ...["dirty", "not-fanout", "protected", "pane-open"].map((x) => `fanoutKeep.rep.skipped.${x}`),
      ...["remove-worktree", "delete-branch", "remove-kept-worktree", "cleanup"].map(
        (x) => `fanoutKeep.rep.error.${x}`,
      ),
    ];
    expect(keys.filter((k) => t(k) === k)).toEqual([]);
  });
});
