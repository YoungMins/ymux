// The git pane's dialogs that other code needs too: the new-worktree branch
// prompt (the palette's "Open pane in new git worktree" command used to take
// it from src/workspace/WorktreeModal.ts) and the worktree removal flow
// (also run when a pane that opened a worktree is closed).
//
// Every decision is `gitModel.ts`'s; this file turns a plan into a dialog
// and, on a yes, into exactly one argv-built git call through `gitApi`.

import { gitApi, type CommitInfo, type StatusEntry, type WorktreeEntry } from "../ipc/bridge";
import { askChoice, askText } from "../ui/Dialog";
import { t } from "../i18n/i18n";
import {
  changeKind,
  clip,
  confirmationStillHolds,
  removePlan,
  validateBranchName,
  type RemovePlan,
} from "./gitModel";

/// Files and commits a dialog lists before "…and N more".
const LIST_MAX = 12;

export function fill(template: string, vars: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m));
}

/// A backend error's text, without the `command: ` prefix nobody needs.
export function gitErrorText(e: unknown): string {
  const msg = e instanceof Error ? e.message : String(e);
  return msg.replace(/^git_[a-z_]+:\s*/, "");
}

/// `modified  src/main.rs`, one per line, clipped.
export function changeLines(changes: readonly StatusEntry[]): string {
  const { shown, more } = clip(changes, LIST_MAX);
  const lines = shown.map((e) => {
    const kind = t(`git.change.${changeKind(e.code)}`);
    return e.orig ? `${kind}  ${e.orig} → ${e.path}` : `${kind}  ${e.path}`;
  });
  if (more) lines.push(fill(t("git.more"), { n: more }));
  return lines.join("\n");
}

/// `abc1234  subject`, one per line, clipped.
export function commitLines(commits: readonly CommitInfo[], moreExist: boolean): string {
  const { shown, more } = clip(commits, LIST_MAX);
  const lines = shown.map((c) => `${c.short}  ${c.subject}`);
  // The backend stops counting at MAX_ORPHANS, so past that there is no
  // number to give.
  if (moreExist) lines.push(t("git.andMore"));
  else if (more) lines.push(fill(t("git.more"), { n: more }));
  return lines.join("\n");
}

/// Show a git failure as a dialog, not a status line nobody reads.
export async function showGitError(title: string, e: unknown): Promise<void> {
  await askChoice(title, gitErrorText(e), [{ id: "ok", label: t("dialog.ok"), primary: true }]);
}

/// Ask for a new worktree's branch name, pre-filled with `suggest`, until
/// it is one git accepts or the user cancels (null).
export async function promptWorktreeBranch(suggest: string): Promise<string | null> {
  let value = suggest;
  let message = t("worktree.branchPrompt");
  for (;;) {
    const v = await askText(message, value);
    if (v === null) return null;
    const name = v.trim();
    if (!name) return null;
    if (validateBranchName(name) === null) return name;
    value = name;
    message = `${fill(t("git.invalidBranch"), { name })}\n${t("worktree.branchPrompt")}`;
  }
}

export type RemoveOutcome = "removed" | "cancelled" | "blocked" | "failed";

/// Remove `entry` after a confirmation that says exactly what goes with it.
///
/// The status is read first, so `--force` is decided before git runs and
/// only ever passed after a dialog listing the changes it deletes. A git
/// failure is shown and never escalated to a forced retry: the old pane-close
/// flow did that on *any* error, which on Windows includes "a terminal's cwd
/// is in there" — and a forced retry of that one deletes what it can.
///
/// The status is read again after the user confirms, and git only runs if it
/// still matches what the dialog listed (`confirmationStillHolds`); if not,
/// the new list is shown and the question asked again.
export async function removeWorktreeFlow(entry: WorktreeEntry): Promise<RemoveOutcome> {
  const first = removePlan(entry, null);
  if (first.kind === "blocked") {
    await askChoice(t(`git.blocked.${first.reason}`), entry.path, [
      { id: "ok", label: t("dialog.ok"), primary: true },
    ]);
    return "blocked";
  }
  let plan = await readPlan(entry);
  let changed = false;
  for (;;) {
    if (plan === null) return "failed";
    if (plan.kind !== "remove") return "blocked";
    const answer = await askRemoval(entry, plan, changed);
    if (!answer) return "cancelled";
    // The dialog may have sat open while an agent kept writing in there:
    // re-read, and only act on a confirmation that still describes what
    // will be deleted. Otherwise show the new list and ask again.
    const fresh = await readPlan(entry);
    if (fresh !== null && confirmationStillHolds(plan, fresh)) break;
    plan = fresh;
    changed = true;
  }
  try {
    await gitApi.worktreeRemove(entry.path, plan.force);
    return "removed";
  } catch (e) {
    await showGitError(t("git.removeFailed"), e);
    return "failed";
  }
}

/// The worktree's current removal plan, or null (error already shown).
async function readPlan(entry: WorktreeEntry): Promise<RemovePlan | null> {
  try {
    return removePlan(entry, await gitApi.workStatus(entry.path, true));
  } catch (e) {
    await showGitError(t("git.removeFailed"), e);
    return null;
  }
}

/// The confirmation for one plan. True on "remove".
async function askRemoval(
  entry: WorktreeEntry,
  plan: Extract<RemovePlan, { kind: "remove" }>,
  changed: boolean,
): Promise<boolean> {
  const parts: string[] = changed ? [t("git.removeChanged"), entry.path] : [entry.path];
  if (plan.branch) parts.push(fill(t("git.removeKeepsBranch"), { branch: plan.branch }));
  if (plan.lost.length) {
    parts.push(`${fill(t("git.removeLost"), { n: plan.lost.length })}\n${changeLines(plan.lost)}`);
  }
  if (plan.stranded.length) {
    parts.push(
      `${fill(t("git.removeStranded"), { n: plan.stranded.length })}\n${commitLines(plan.stranded, plan.moreStranded)}`,
    );
  }
  if (plan.ignored.length) {
    const { shown, more } = clip(plan.ignored, LIST_MAX);
    const names = shown.join("\n") + (more ? `\n${fill(t("git.more"), { n: more })}` : "");
    parts.push(`${t("git.removeIgnored")}\n${names}`);
  }
  const loses = plan.loses;
  const answer = await askChoice(t("git.removeTitle"), parts.join("\n\n"), [
    // Something is destroyed — ignored files included: the focused default
    // is to keep it.
    { id: "cancel", label: t("dialog.cancel"), primary: loses },
    {
      id: "remove",
      label: t(plan.force ? "git.removeForceBtn" : "git.removeBtn"),
      primary: !loses,
    },
  ]);
  return answer?.id === "remove";
}
