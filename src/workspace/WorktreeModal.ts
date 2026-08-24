// Branch-name input for the "new worktree" flow.
//
// Backed by the in-app dialog rather than `window.prompt`, which WKWebView
// silently ignores — see `src/ui/Dialog.ts`. Async as a result.
import { askText } from "../ui/Dialog";
import { t } from "../i18n/i18n";

/// Prompt the user for a new worktree's branch name, pre-filled with
/// `suggest`. Returns the trimmed branch name, or `null` if the user
/// cancelled or entered only whitespace.
export async function promptWorktreeBranch(
  suggest: string,
): Promise<string | null> {
  const v = await askText(t("worktree.branchPrompt"), suggest);
  const trimmed = (v ?? "").trim();
  return trimmed.length ? trimmed : null;
}
