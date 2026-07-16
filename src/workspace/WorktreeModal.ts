// Branch-name input for the "new worktree" flow. Reuses the existing
// `promptWithBlur` helper (see `src/browser/popupBlur.ts`) rather than
// building a bespoke modal — it already handles the native-webview z-order
// workaround shared by every other popup (Command Palette, Help, workspace
// rename, etc.). `promptWithBlur` is synchronous (wraps `window.prompt`), so
// this helper is synchronous too.
//
// NOTE: the i18n key `worktree.branchPrompt` referenced below is added in
// Task 12 alongside the rest of the worktree UI strings. `t()` falls back to
// returning the key itself when a translation is missing, so this compiles
// and runs cleanly before that key exists — it just shows the raw key text
// in the prompt dialog until Task 12 lands.
import { promptWithBlur } from "../browser/popupBlur";
import { t } from "../i18n/i18n";

/// Prompt the user for a new worktree's branch name, pre-filled with
/// `suggest`. Returns the trimmed branch name, or `null` if the user
/// cancelled or entered only whitespace.
export function promptWorktreeBranch(suggest: string): string | null {
  const v = promptWithBlur(t("worktree.branchPrompt"), suggest);
  const trimmed = (v ?? "").trim();
  return trimmed.length ? trimmed : null;
}
