// The git pane's decisions, as pure functions (spec §4): the branch list,
// what a checkout would do and what it would cost, whether a worktree may be
// removed and what removing it destroys, decorations, branch-name checks.
// `GitPane.ts` is the DOM and IPC around these.
//
// The confirmations are built from what git actually does, which is not
// what it sounds like:
//
//  - `git checkout <branch>` never discards work. Local changes are carried
//    to the new branch, or git refuses if they would be overwritten. What a
//    checkout *can* lose is a detached HEAD's commits that no ref reaches —
//    they stay only in the reflog.
//  - `git worktree remove` deletes the worktree's folder, and with it every
//    ignored file (build output, `node_modules/`) even without `--force`.
//    `--force` additionally deletes uncommitted and untracked changes. A
//    locked worktree needs a double `--force`, which the pane never passes.

import type { BranchList, CommitInfo, StatusEntry, WorkStatus, WorktreeEntry } from "../ipc/bridge";

// ── Branches ──────────────────────────────────────────────────────────────

export interface BranchItem {
  kind: "local" | "remote";
  /// `main`, or `origin/main` for a remote-tracking branch.
  name: string;
  /// Checked out in this worktree.
  current: boolean;
  /// Path of the other worktree that has it checked out, or null.
  heldBy: string | null;
}

/// Local branches in git's order, then remote-tracking ones.
export function branchItems(list: BranchList): BranchItem[] {
  return [
    ...list.local.map((name) => ({
      kind: "local" as const,
      name,
      current: name === list.current,
      heldBy: list.held[name] ?? null,
    })),
    ...list.remote.map((name) => ({
      kind: "remote" as const,
      name,
      current: false,
      heldBy: null,
    })),
  ];
}

/// `origin/feature/x` → `feature/x`: the local branch `git checkout --track`
/// would create. The remote's name is the first path segment.
export function localNameOf(remote: string): string {
  const slash = remote.indexOf("/");
  return slash === -1 ? remote : remote.slice(slash + 1);
}

export type CheckoutPlan =
  /// Already on it.
  | { kind: "noop"; branch: string }
  /// Another worktree has it: git would refuse. Offer to view that one.
  | { kind: "held"; branch: string; path: string }
  /// `git checkout <branch> --`. `fromRemote` when the user picked the
  /// remote-tracking branch and a local one of that name already exists.
  | { kind: "local"; branch: string; fromRemote?: string }
  /// `git checkout --track <remote> --`, creating local `branch`.
  | { kind: "track"; remote: string; branch: string };

export function checkoutPlan(item: BranchItem, list: BranchList): CheckoutPlan {
  if (item.kind === "local") {
    if (item.name === list.current) return { kind: "noop", branch: item.name };
    const held = list.held[item.name];
    if (held !== undefined) return { kind: "held", branch: item.name, path: held };
    return { kind: "local", branch: item.name };
  }
  const local = localNameOf(item.name);
  if (!list.local.includes(local)) return { kind: "track", remote: item.name, branch: local };
  if (local === list.current) return { kind: "noop", branch: local };
  const held = list.held[local];
  if (held !== undefined) return { kind: "held", branch: local, path: held };
  return { kind: "local", branch: local, fromRemote: item.name };
}

export interface CheckoutRisk {
  /// Local changes that move to the new branch with the user.
  carried: StatusEntry[];
  /// Detached-HEAD commits nothing will reach after the checkout.
  stranded: CommitInfo[];
  moreStranded: boolean;
  /// True when something becomes unreachable — the only real loss.
  loses: boolean;
}

export function checkoutRisk(status: WorkStatus): CheckoutRisk {
  const stranded = status.detached ? status.orphans : [];
  return {
    carried: status.changes,
    stranded,
    moreStranded: status.detached && status.more_orphans,
    loses: stranded.length > 0,
  };
}

// ── Worktrees ─────────────────────────────────────────────────────────────

export type RemoveBlock = "main" | "locked" | "current" | "missing";

export type RemovePlan =
  | { kind: "blocked"; reason: RemoveBlock }
  /// The worktree may go; fetch its `WorkStatus` (with ignored files) to
  /// say what goes with it.
  | { kind: "needStatus" }
  | {
      kind: "remove";
      /// Needed exactly when there are changes — and then they are `lost`.
      force: boolean;
      /// Empty on a detached worktree.
      branch: string;
      lost: StatusEntry[];
      /// Deleted with the folder whether or not `force` is set.
      ignored: string[];
      /// A detached worktree's commits no ref reaches.
      stranded: CommitInfo[];
      moreStranded: boolean;
      /// Anything at all is destroyed — changes, stranded commits *or*
      /// ignored files (`.env`, a local database, which git deletes even
      /// without `--force`). Then Cancel is the dialog's default.
      loses: boolean;
    };

/// Decide a removal. Called twice: without a status to find out whether the
/// removal is possible at all, then with the worktree's own status.
///
/// The worktree the pane is showing is refused: the pane (and any terminal
/// whose cwd is in there — on Windows a process's cwd cannot be deleted, so
/// git would fail half-way) should move off it first.
export function removePlan(entry: WorktreeEntry, status: WorkStatus | null): RemovePlan {
  if (entry.main) return { kind: "blocked", reason: "main" };
  if (entry.locked) return { kind: "blocked", reason: "locked" };
  if (entry.current) return { kind: "blocked", reason: "current" };
  if (entry.prunable) return { kind: "blocked", reason: "missing" };
  if (status === null) return { kind: "needStatus" };
  const stranded = status.detached ? status.orphans : [];
  return {
    kind: "remove",
    force: status.changes.length > 0,
    branch: entry.branch,
    lost: status.changes,
    ignored: status.ignored,
    stranded,
    moreStranded: status.detached && status.more_orphans,
    loses: status.changes.length > 0 || stranded.length > 0 || status.ignored.length > 0,
  };
}

// ── Status codes ──────────────────────────────────────────────────────────

export type ChangeKind =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "copied"
  | "untracked"
  | "conflict";

/// git's two-column `XY` status code, as one word.
export function changeKind(code: string): ChangeKind {
  if (code === "??") return "untracked";
  const [x, y] = [code[0] ?? " ", code[1] ?? " "];
  // Unmerged: either side `U`, or both added / both deleted.
  if (x === "U" || y === "U" || code === "AA" || code === "DD") return "conflict";
  if (x === "R" || y === "R") return "renamed";
  if (x === "C" || y === "C") return "copied";
  if (x === "A") return "added";
  if (x === "D" || y === "D") return "deleted";
  return "modified";
}

/// The first `max` items and how many were left out — a dialog lists a
/// bounded number of files and says "and N more".
export function clip<T>(items: readonly T[], max: number): { shown: T[]; more: number } {
  return { shown: items.slice(0, max), more: Math.max(0, items.length - max) };
}

// ── Decorations ───────────────────────────────────────────────────────────

export interface RefChip {
  /// `head`: the branch HEAD is on. `detached`: HEAD on no branch.
  kind: "head" | "detached" | "branch" | "remote" | "tag";
  name: string;
}

/// `%D`'s decorations as chips. `remotes` is the remote-tracking branch
/// list (from `git_branches`), which is the only reliable way to tell
/// `origin/main` from a local branch literally named that.
export function refChips(refs: readonly string[], remotes: ReadonlySet<string>): RefChip[] {
  const out: RefChip[] = [];
  for (const r of refs) {
    if (r.startsWith("HEAD -> ")) out.push({ kind: "head", name: r.slice(8) });
    else if (r === "HEAD") out.push({ kind: "detached", name: "HEAD" });
    else if (r.startsWith("tag: ")) out.push({ kind: "tag", name: r.slice(5) });
    // `origin/HEAD` (and its `-> origin/main` form) is an alias.
    else if (r.endsWith("/HEAD") || r.includes("/HEAD -> ")) continue;
    else if (remotes.has(r)) out.push({ kind: "remote", name: r });
    else out.push({ kind: "branch", name: r });
  }
  return out;
}

// ── Branch names ──────────────────────────────────────────────────────────

/// Why `name` is not a branch name git accepts, or null when it is. A
/// subset of `git check-ref-format`, enough to answer in the dialog instead
/// of in a git error; git remains the authority, and the backend separately
/// refuses anything that starts with `-` or holds a control character.
export function validateBranchName(name: string): string | null {
  if (name.length === 0) return "empty";
  if (name.startsWith("-")) return "dash";
  // eslint-disable-next-line no-control-regex
  if (/[\x00-\x20\x7f~^:?*[\\]/.test(name)) return "char";
  if (name.includes("..") || name.includes("@{") || name === "@") return "sequence";
  if (name.startsWith("/") || name.endsWith("/") || name.includes("//")) return "slash";
  if (name.endsWith(".") || name.endsWith(".lock")) return "ending";
  if (name.split("/").some((part) => part.startsWith(".") || part.endsWith(".lock"))) {
    return "component";
  }
  return null;
}

// ── Dates ─────────────────────────────────────────────────────────────────

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/// Today → `14:03`; otherwise `2026-09-24`. Numeric like the files pane's
/// dates: it reads the same in all 13 languages and lines up in a column.
export function formatCommitDate(ms: number, now = new Date()): string {
  if (!ms) return "";
  const d = new Date(ms);
  if (
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  ) {
    return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
  }
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}
