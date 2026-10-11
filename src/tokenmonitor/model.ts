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
  readonly hourly: readonly HourBucket[];
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
export interface HourBucket {
  readonly hour: number;
  readonly provider: string;
  readonly total: number;
}
export interface HeatmapCell {
  readonly date: string;
  readonly total: number;
  readonly level: 0 | 1 | 2 | 3 | 4;
}
const pad2 = (value: number): string => String(value).padStart(2, "0");
/** Local calendar key, "YYYY-MM-DD". */
export function dateKey(date: Date): string { return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`; }
/** Groups UTC hour buckets into local calendar days (DST and zone safe). */
export function dailyTotals(hourly: readonly HourBucket[], provider: string | null): Map<string, number> {
  const days = new Map<string, number>();
  for (const bucket of hourly) {
    if (provider !== null && bucket.provider !== provider) continue;
    const key = dateKey(new Date(bucket.hour * 3600_000));
    days.set(key, (days.get(key) ?? 0) + bucket.total);
  }
  return days;
}
/**
 * Columns of up to 7 days (Sunday first), oldest week first, ending with the
 * week containing `today`; days after `today` are omitted. Levels 1-4 are the
 * quartiles of the non-zero days shown, 0 is no usage.
 */
export function heatmapGrid(daily: ReadonlyMap<string, number>, today: Date, weeks: number): HeatmapCell[][] {
  const count = Math.max(1, Math.floor(weeks));
  const y = today.getFullYear(), m = today.getMonth(), d = today.getDate();
  const first = d - today.getDay() - (count - 1) * 7;
  const columns: { date: string; total: number }[][] = [];
  const nonzero: number[] = [];
  for (let week = 0; week < count; week++) {
    const column: { date: string; total: number }[] = [];
    for (let day = 0; day < 7; day++) {
      const date = new Date(y, m, first + week * 7 + day);
      if (date.getTime() > today.getTime() && dateKey(date) !== dateKey(today)) break;
      const key = dateKey(date); const total = daily.get(key) ?? 0;
      if (total > 0) nonzero.push(total);
      column.push({ date: key, total });
    }
    columns.push(column);
  }
  nonzero.sort((a, b) => a - b);
  const cut = (quarter: number): number => nonzero[Math.max(0, Math.ceil(nonzero.length * quarter / 4) - 1)] ?? 0;
  const [q1, q2, q3] = [cut(1), cut(2), cut(3)];
  const level = (total: number): HeatmapCell["level"] => total <= 0 ? 0 : total <= q1 ? 1 : total <= q2 ? 2 : total <= q3 ? 3 : 4;
  return columns.map(column => column.map(cell => ({ ...cell, level: level(cell.total) })));
}
