// Pure decisions behind the fan-out compare view.

import type { DiffFile } from "../diff/types";
import { quotePathForShell, type ShellFamily } from "../terminal/shellQuote";

export interface LaneRef {
  path: string;
  branch: string;
  base: string;
  runId: string;
}

/// Run ids newest first. They start `yyyymmdd-hhmm`, so text order is time order.
export function runIds(lanes: readonly LaneRef[]): string[] {
  return [...new Set(lanes.map((l) => l.runId))].sort().reverse();
}

export function lanesOfRun(lanes: readonly LaneRef[], runId: string): LaneRef[] {
  return lanes.filter((l) => l.runId === runId);
}

export interface DiffStat {
  files: number;
  added: number;
  deleted: number;
  /// Some file's line counts are unknown (binary / uncounted).
  partial: boolean;
}

export function diffStat(files: readonly DiffFile[]): DiffStat {
  let added = 0;
  let deleted = 0;
  let partial = false;
  for (const f of files) {
    if (f.binary || f.counts_unknown) partial = true;
    added += f.added;
    deleted += f.deleted;
  }
  return { files: files.length, added, deleted, partial };
}

/// Commits of `log` (newest first, from the lane's HEAD) strictly above
/// `base`. `more`: `base` was not reached within the log, so the count is a
/// lower bound.
/// `count` is the backend's exact `rev-list --count base..HEAD` (null when
/// that failed); it wins over the log-based estimate.
export function commitsAhead(
  log: readonly { hash: string }[],
  base: string,
  count: number | null = null,
): { n: number; more: boolean } {
  if (count !== null && Number.isFinite(count) && count >= 0) return { n: count, more: false };
  const i = log.findIndex((c) => c.hash === base);
  return i >= 0 ? { n: i, more: false } : { n: log.length, more: true };
}

/// `git merge --no-ff <branch>`, the branch quoted only when its spelling
/// needs it. `null` when it cannot be typed safely into `family`.
export function mergeCommand(branch: string, family: ShellFamily): string | null {
  if (/^[A-Za-z0-9._/-]+$/.test(branch) && !branch.startsWith("-")) {
    return `git merge --no-ff ${branch}`;
  }
  const q = quotePathForShell(branch, family);
  return q === null || branch.startsWith("-") ? null : `git merge --no-ff ${q}`;
}

/// The lanes "Clean up other lanes" removes: every lane of the run except
/// the kept one. The main worktree is never a lane.
export function cleanupTargets(lanes: readonly LaneRef[], keepPath: string): LaneRef[] {
  return lanes.filter((l) => l.path !== keepPath);
}
