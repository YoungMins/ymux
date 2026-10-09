export interface TokenTotals {
  readonly input: number;
  readonly output: number;
  readonly cache_read: number;
  readonly cache_write: number;
  readonly total: number;
}
export type UsageWindow = "five_hour" | "seven_day" | "all_time";
export function remainingQuota(quota: TokenUsage["providers"][number]["quotas"][number] | undefined, now: number): number | null {
  if (!quota || !Number.isFinite(quota.used_percent) || (quota.resets_at !== null && quota.resets_at <= now)) return null;
  return 100 - Math.min(100, Math.max(0, quota.used_percent));
}
export interface UsageRow {
  readonly provider: string;
  readonly model: string;
  readonly project: string;
  readonly five_hour: TokenTotals;
  readonly seven_day: TokenTotals;
  readonly all_time: TokenTotals;
}
export interface TokenUsage {
  readonly collected_at: number;
  readonly providers: readonly {
    readonly id: string;
    readonly auth_status: "credentials_present" | "not_detected" | "unknown";
    readonly five_hour: TokenTotals;
    readonly seven_day: TokenTotals;
    readonly all_time: TokenTotals;
    readonly quotas: readonly { readonly window_minutes: number; readonly used_percent: number; readonly resets_at: number | null; readonly observed_at: number }[];
    readonly files: number;
    readonly skipped: number;
  }[];
  readonly rows: readonly UsageRow[];
  readonly warnings: readonly string[];
}
export function aggregateUsage(rows: readonly UsageRow[], group: "provider" | "model" | "project", window: UsageWindow): readonly { readonly label: string; readonly tokens: TokenTotals }[] {
  const result = new Map<string, TokenTotals>();
  for (const row of rows) {
    const label = group === "model" ? `${row.provider} / ${row.model}` : row[group];
    const prev = result.get(label) ?? { input: 0, output: 0, cache_read: 0, cache_write: 0, total: 0 };
    const next = row[window];
    result.set(label, { input: prev.input + next.input, output: prev.output + next.output, cache_read: prev.cache_read + next.cache_read, cache_write: prev.cache_write + next.cache_write, total: prev.total + next.total });
  }
  return [...result].map(([label, tokens]) => ({ label, tokens })).sort((a, b) => b.tokens.total - a.tokens.total);
}
