// Pure model behind "Keep this one": what the confirm dialog says will
// happen, the backend request it turns into, and the report it comes back as.
// No DOM and no IPC here; FanoutCompare.ts does the talking.

import type { KeepReport, KeepRequest } from "../ipc/bridge";
import type { LaneRef } from "./compareModel";

export type KeepMode = "cleanup" | "merge-only";

export interface KeepChoice {
  mode: KeepMode;
  /// Also remove the kept lane's worktree after merging.
  removeKept: boolean;
  /// Force-remove other lanes that have uncommitted changes.
  discardDirty: boolean;
}

/// The dialog's defaults: remove the kept worktree, never discard.
export const DEFAULT_CHOICE: KeepChoice = {
  mode: "cleanup",
  removeKept: true,
  discardDirty: false,
};

/// The lanes the fan-out run lost to `keep`: same run, other worktree.
export function losingLanes(lanes: readonly LaneRef[], keep: LaneRef): LaneRef[] {
  return lanes.filter((l) => l.runId === keep.runId && l.path !== keep.path);
}

/// What the dialog lists. `removals` is empty in merge-only mode.
export interface KeepPlan {
  branch: string;
  base: string;
  keptPath: string;
  removals: { branch: string; path: string }[];
}

export function keepPlan(
  lanes: readonly LaneRef[],
  keep: LaneRef,
  baseBranch: string,
  keptPath: string,
  choice: KeepChoice,
): KeepPlan {
  const removals =
    choice.mode === "cleanup"
      ? losingLanes(lanes, keep).map((l) => ({ branch: l.branch, path: l.path }))
      : [];
  return { branch: keep.branch, base: baseBranch, keptPath, removals };
}

export function keepRequest(
  repo: string,
  baseBranch: string,
  lanes: readonly LaneRef[],
  keep: LaneRef,
  choice: KeepChoice,
  dryRun: boolean,
): KeepRequest {
  const cleanup = choice.mode === "cleanup";
  return {
    repo,
    baseBranch,
    runId: keep.runId,
    keepBranch: keep.branch,
    others: cleanup ? losingLanes(lanes, keep).map((l) => l.branch) : [],
    removeKeptWorktree: cleanup && choice.removeKept,
    discardDirtyOthers: cleanup && choice.discardDirty,
    dryRun,
    cleanupOnly: false,
  };
}

/// The real run's first step: the merge alone, nothing removed.
export function mergeRequest(
  repo: string,
  baseBranch: string,
  lanes: readonly LaneRef[],
  keep: LaneRef,
  choice: KeepChoice,
): KeepRequest {
  return keepRequest(repo, baseBranch, lanes, keep, { ...choice, mode: "merge-only" }, false);
}

/// The second step, after the merge: remove what `choice` asks for, except
/// the lanes in `open` (worktree paths whose terminal would not close).
export function cleanupRequest(
  repo: string,
  baseBranch: string,
  lanes: readonly LaneRef[],
  keep: LaneRef,
  choice: KeepChoice,
  open: readonly string[],
): KeepRequest {
  const q = keepRequest(repo, baseBranch, lanes, keep, choice, false);
  return {
    ...q,
    others: losingLanes(lanes, keep)
      .filter((l) => choice.mode === "cleanup" && !open.includes(l.path))
      .map((l) => l.branch),
    removeKeptWorktree: q.removeKeptWorktree && !open.includes(keep.path),
    cleanupOnly: true,
  };
}

/// Does the cleanup step have anything left to do?
export function hasCleanup(q: KeepRequest): boolean {
  return q.others.length > 0 || q.removeKeptWorktree;
}

/// Lanes the cleanup left alone because a terminal in them would not close,
/// as report entries.
export function openSkips(
  lanes: readonly LaneRef[],
  keep: LaneRef,
  choice: KeepChoice,
  open: readonly string[],
): KeepReport["skipped"] {
  if (choice.mode !== "cleanup") return [];
  const out: KeepReport["skipped"] = [];
  for (const l of losingLanes(lanes, keep)) {
    if (open.includes(l.path)) out.push({ branch: l.branch, path: l.path, reason: "pane-open", files: [] });
  }
  if (choice.removeKept && open.includes(keep.path)) {
    out.push({ branch: keep.branch, path: keep.path, reason: "pane-open", files: [] });
  }
  return out;
}

/// The merge report and (when it ran) the cleanup report as one: the merge
/// stays the outcome, the cleanup adds its removals, skips and errors.
export function combineReports(
  merge: KeepReport,
  cleanup: KeepReport | null,
  extraSkipped: KeepReport["skipped"],
  conflictCheckUnavailable: boolean,
): KeepReport {
  const out: KeepReport = { ...merge, conflict_check_unavailable: conflictCheckUnavailable };
  if (cleanup) {
    if (cleanup.outcome === "cleaned") {
      out.removed_worktrees = cleanup.removed_worktrees;
      out.deleted_branches = cleanup.deleted_branches;
      out.kept_worktree_removed = cleanup.kept_worktree_removed;
      out.skipped = [...cleanup.skipped];
      out.errors = [...cleanup.errors];
    } else {
      // Refused or failed after the merge: the merge stands, the cleanup did not run.
      out.errors = [{ step: "cleanup", branch: "", detail: cleanup.detail || cleanup.reason || cleanup.outcome }];
    }
  }
  out.skipped = [...out.skipped, ...extraSkipped];
  return out;
}

/// Paths whose terminals must be closed before the real run: the losing
/// lanes' worktrees, and the kept one when it is going too.
export function pathsToVacate(
  lanes: readonly LaneRef[],
  keep: LaneRef,
  choice: KeepChoice,
): string[] {
  if (choice.mode !== "cleanup") return [];
  const out = losingLanes(lanes, keep).map((l) => l.path);
  if (choice.removeKept) out.push(keep.path);
  return out;
}

/// One line of a report: an i18n key, its variables, and how to read it.
export interface ReportLine {
  key: string;
  vars: Record<string, string | number>;
  tone: "ok" | "warn" | "bad";
}

/// Files shown per list before "…".
const FILE_MAX = 10;

export function fileList(files: readonly string[]): string {
  const shown = files.slice(0, FILE_MAX).join(", ");
  return files.length > FILE_MAX ? `${shown}, … (+${files.length - FILE_MAX})` : shown;
}

/// A refusal the front end reaches without asking the backend.
export function refusal(reason: string): KeepReport {
  return {
    outcome: "refused",
    reason,
    files: [],
    detail: "",
    left_mid_merge: false,
    merged_commit: "",
    removed_worktrees: [],
    deleted_branches: [],
    kept_worktree_removed: false,
    skipped: [],
    errors: [],
    conflict_check_unavailable: false,
  };
}

/// True when the backend changed the base branch.
export function didMerge(r: KeepReport): boolean {
  return r.outcome === "merged";
}

/// The report as translatable lines, in the order things happened.
export function reportLines(r: KeepReport, base: string): ReportLine[] {
  const line = (
    key: string,
    vars: Record<string, string | number>,
    tone: ReportLine["tone"],
  ): ReportLine => ({ key, vars, tone });
  switch (r.outcome) {
    case "refused":
      return [line(`fanoutKeep.refused.${r.reason}`, { files: fileList(r.files), base }, "bad")];
    case "conflict": {
      const out = [line("fanoutKeep.conflict", { files: fileList(r.files), base }, "bad")];
      if (r.left_mid_merge) out.push(line("fanoutKeep.midMerge", { detail: r.detail, base }, "bad"));
      return out;
    }
    case "failed":
      return [line("fanoutKeep.failed", { detail: r.detail, base }, "bad")];
    case "would-conflict":
      return [line("fanoutKeep.wouldConflict", { files: fileList(r.files), base }, "bad")];
    case "ok":
      return [];
    default:
      break;
  }
  const out: ReportLine[] = [];
  if (r.outcome === "merged") {
    out.push(line("fanoutKeep.rep.merged", { commit: r.merged_commit.slice(0, 8), base }, "ok"));
  }
  if (r.conflict_check_unavailable) out.push(line("fanoutKeep.rep.noConflictCheck", {}, "warn"));
  for (const w of r.removed_worktrees) {
    out.push(line("fanoutKeep.rep.removed", { branch: w.branch, path: w.path }, "ok"));
  }
  for (const b of r.deleted_branches) out.push(line("fanoutKeep.rep.deleted", { branch: b }, "ok"));
  if (r.kept_worktree_removed) out.push(line("fanoutKeep.rep.keptRemoved", {}, "ok"));
  for (const s of r.skipped) {
    out.push(
      line(
        `fanoutKeep.rep.skipped.${s.reason}`,
        { branch: s.branch, path: s.path, files: fileList(s.files) },
        "warn",
      ),
    );
  }
  for (const e of r.errors) {
    out.push(line(`fanoutKeep.rep.error.${e.step}`, { branch: e.branch, detail: e.detail }, "bad"));
  }
  return out;
}
