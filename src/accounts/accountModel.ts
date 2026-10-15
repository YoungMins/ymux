// Pure model for per-pane AI-CLI accounts (CLAUDE_CONFIG_DIR / CODEX_HOME
// profiles). The backend (`src-tauri/src/accounts.rs`) turns the effective
// account into env vars at spawn; this side decides which profile that is for
// the title badge, and keeps the profile list every UI reads.

import type { AccountProfile, Workspace } from "../types";
import { panes, findAndMutatePane } from "../layout/LayoutTree";
import { uuidv4 } from "../types";

/// `PaneSpec.account` value meaning "no account, even if the workspace has
/// one". Mirrors `accounts::ACCOUNT_NONE` in Rust.
export const ACCOUNT_NONE = "none";

export interface AccountBadge {
  id: string;
  name: string;
  /// "#rrggbb" or "" for the default dot colour.
  color: string;
}

/// Short stable id: 8 hex chars, so it can never equal `ACCOUNT_NONE`.
export function newAccountId(): string {
  return uuidv4().replace(/-/g, "").slice(0, 8);
}

/// Same resolution as the backend: the pane's own non-empty choice wins over
/// the workspace default, `ACCOUNT_NONE` opts out, an unknown id is nothing.
export function effectiveAccount(
  accounts: readonly AccountProfile[],
  paneAccount: string | undefined,
  workspaceAccount: string | undefined,
): AccountProfile | null {
  const id = paneAccount || workspaceAccount || "";
  if (id === "" || id === ACCOUNT_NONE) return null;
  return accounts.find((a) => a.id === id) ?? null;
}

/// The configured account (`badgeId`, null = none) differs from the one the
/// PTY was started under (`ranAccountId`, null = not spawned yet). A running
/// process keeps its environment, so a mismatch needs a respawn to apply.
export function isAccountPending(badgeId: string | null, ranAccountId: string | null): boolean {
  return badgeId !== null && ranAccountId !== null && ranAccountId !== badgeId;
}

export function toBadge(a: AccountProfile | null): AccountBadge | null {
  return a ? { id: a.id, name: a.name, color: normalizeColor(a.color) } : null;
}

/// "#rrggbb" (lower-cased) or "" when the text is not a valid colour.
export function normalizeColor(raw: string): string {
  const v = raw.trim().toLowerCase();
  return /^#[0-9a-f]{6}$/.test(v) ? v : "";
}

/// Remove every reference to account `id` from the workspaces: panes fall
/// back to "inherit" and workspaces to "no default". Returns whether anything
/// changed.
export function clearAccountRefs(workspaces: Workspace[], id: string): boolean {
  let changed = false;
  for (const ws of workspaces) {
    if (ws.account === id) {
      ws.account = "";
      changed = true;
    }
    for (const p of panes(ws.root)) {
      if (p.account === id) {
        findAndMutatePane(ws.root, p.id, (spec) => {
          spec.account = "";
        });
        changed = true;
      }
    }
  }
  return changed;
}

// ── Shared profile list ──────────────────────────────────────────────────
// The pane settings dialog, the workspace context menu and the title badges
// all need the current list; `WorkspaceManager` owns the config and pushes it
// here.

let current: readonly AccountProfile[] = [];

export function getAccounts(): readonly AccountProfile[] {
  return current;
}

export function setAccountsList(next: readonly AccountProfile[]): void {
  current = next;
}
