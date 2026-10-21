// Add an account without asking for any folder: ask only for a name, let the
// backend pick `<config_dir>/accounts/<id>/{claude,codex}`, and save the
// profile. Claude Code / Codex find no login there, so the first agent
// started under it asks the user to sign in, the same way a website's
// "add another account" does.

import type { AccountProfile } from "../types";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import { api, describeError } from "../ipc/bridge";
import { askText } from "../ui/Dialog";
import { t } from "../i18n/i18n";
import { fill } from "../git/worktreeFlow";
import { newAccountId, nextAccountColor, nextAccountName } from "./accountModel";

/// Build a managed profile named `name` (the backend creates its folders).
export async function managedAccount(
  existing: readonly AccountProfile[],
  name: string,
): Promise<AccountProfile> {
  const id = newAccountId();
  const dirs = await api.prepareAccountDirs(id);
  return {
    id,
    name,
    claude_config_dir: dirs.claude_config_dir,
    codex_home: dirs.codex_home,
    color: nextAccountColor(existing),
  };
}

/// Ask for a name, create the account and save it, resolving once the save
/// reached the backend. `null` when the user cancelled or the folders could
/// not be created (the user is told).
export async function promptNewAccount(manager: WorkspaceManager): Promise<AccountProfile | null> {
  const fallback = nextAccountName(manager.accounts, t("settings.accounts.defaultName"));
  const typed = await askText(t("account.newPrompt"), fallback);
  if (typed === null) return null;
  const name = typed.trim() || fallback;
  let profile: AccountProfile;
  try {
    profile = await managedAccount(manager.accounts, name);
  } catch (e) {
    console.error("create account failed:", e);
    manager.notify(fill(t("account.createFailed"), { error: describeError(e) }));
    return null;
  }
  manager.setAccounts([...manager.accounts, profile]);
  // `setAccounts` only arms the debounced save, and `spawn_pane` resolves
  // the account id against the *saved* config: a tab spawned before the
  // save lands would run the agent under the default login. A failed save
  // is reported by the manager itself.
  await manager.flushPendingSave();
  return profile;
}
