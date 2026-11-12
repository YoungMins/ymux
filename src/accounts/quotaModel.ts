// Quota-aware account switching, pure. Which accounts are near or at their
// usage limit for an agent CLI, and which other accounts could take over.
// The numbers are the ones the Usage view already shows
// (`ProviderUsage.quotas`, per owning account and CLI).

import { ACCOUNT_NONE, agentUsesAccounts } from "./accountModel";
import { remainingQuota, type ProviderUsage } from "../tokenmonitor/model";

/// Used share (percent) from which an account counts as near / at its limit.
export const NEAR_PERCENT = 90;
export const LIMIT_PERCENT = 100;

export type QuotaLevel = "ok" | "near" | "limit";

/// `used` is the highest used share over the account's live windows, or null
/// when no window is known (nothing recorded, or every window has reset).
export function quotaLevel(used: number | null): QuotaLevel {
  if (used === null || !Number.isFinite(used)) return "ok";
  if (used >= LIMIT_PERCENT) return "limit";
  return used >= NEAR_PERCENT ? "near" : "ok";
}

/// The cards of one CLI that carry `account`'s quota (its own, or the one of
/// the account whose folder it shares).
function cards(providers: readonly ProviderUsage[], account: string, agent: string): ProviderUsage[] {
  return providers.filter((p) => p.id === agent && (p.account === account || p.also.includes(account)));
}

/// Highest used percent across the 5-hour and weekly windows of `agent` under
/// `account`, windows that already reset left out. Null when unknown.
export function usedPercent(
  providers: readonly ProviderUsage[],
  account: string,
  agent: string,
  now: number,
): number | null {
  let used: number | null = null;
  for (const p of cards(providers, account, agent)) {
    for (const q of p.quotas) {
      const left = remainingQuota(q, now);
      if (left === null) continue;
      used = Math.max(used ?? 0, 100 - left);
    }
  }
  return used;
}

/// Accounts sharing one folder share one quota; this names the folder's owner.
function ownerKey(providers: readonly ProviderUsage[], account: string, agent: string): string {
  return cards(providers, account, agent)[0]?.account ?? account;
}

export interface SwitchOption {
  account: string;
  used: number | null;
  /// 100 - used, or null when the quota is unknown.
  headroom: number | null;
  level: QuotaLevel;
}

/// The accounts to offer instead of `current`: not `current`, and not an
/// account that shares its folder (same quota), best headroom first, accounts
/// with an unknown quota after the known ones. `candidates` are ids a pane can
/// run under (`ACCOUNT_NONE` for the default login, plus the profiles).
export function rankSwitchTargets(
  providers: readonly ProviderUsage[],
  candidates: readonly string[],
  current: string,
  agent: string,
  now: number,
): SwitchOption[] {
  const seen = new Set<string>([ownerKey(providers, current, agent)]);
  const out: SwitchOption[] = [];
  for (const account of candidates) {
    if (account === current) continue;
    const owner = ownerKey(providers, account, agent);
    if (seen.has(owner)) continue;
    seen.add(owner);
    const used = usedPercent(providers, account, agent, now);
    out.push({ account, used, headroom: used === null ? null : Math.max(0, 100 - used), level: quotaLevel(used) });
  }
  return out.sort((a, b) => {
    if ((a.headroom === null) !== (b.headroom === null)) return a.headroom === null ? 1 : -1;
    return (b.headroom ?? 0) - (a.headroom ?? 0);
  });
}

export interface QuotaAlert {
  level: "near" | "limit";
  used: number;
  /// Other accounts, best first.
  options: SwitchOption[];
}

/// An alert only when `current` is near/at its limit AND some other account is
/// known to have headroom (below the "near" mark): with one account, or none
/// better, there is nothing to offer and nothing to say.
export function quotaAlert(
  providers: readonly ProviderUsage[],
  candidates: readonly string[],
  current: string,
  agent: string,
  now: number,
): QuotaAlert | null {
  const used = usedPercent(providers, current, agent, now);
  const level = quotaLevel(used);
  if (level === "ok" || used === null) return null;
  const options = rankSwitchTargets(providers, candidates, current, agent, now);
  if (!options.some((o) => o.headroom !== null && o.level === "ok")) return null;
  return { level, used, options };
}

/// Account ids a pane can be moved to: the default login and every profile.
export function switchCandidates(profileIds: readonly string[]): string[] {
  return [ACCOUNT_NONE, ...profileIds.filter((id) => id !== "" && id !== ACCOUNT_NONE)];
}

/// Whether the 60 s quota poll is worth running: there is a profile to move
/// to (the default login plus at least one profile) and at least one live
/// pane whose agent runs under an account at all. `leadKinds`: the kind of
/// each live lead in a terminal pane.
export function quotaPollWanted(profileCount: number, leadKinds: readonly string[]): boolean {
  return profileCount > 0 && leadKinds.some((k) => agentUsesAccounts(k));
}

/// The prompt offered after a switch. English on purpose: it is read by the
/// agent, and the user can edit it before it is sent.
export function handoffNote(info: { cwd: string | null; branch: string | null }): string {
  const lines = [
    "Continue the work from the previous session. It ran on another account that hit its usage limit, so its conversation is not available here.",
  ];
  if (info.cwd) lines.push(`Working directory: ${info.cwd}`);
  if (info.branch) lines.push(`Branch: ${info.branch}`);
  lines.push("First run `git status` and `git diff` to see what was in progress, then carry on.");
  return lines.join("\n");
}
