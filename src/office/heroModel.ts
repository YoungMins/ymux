// Accounts as RPG heroes: an account's all-time token total (the Usage
// view's number, Claude Code and Codex together) is its experience, one
// level per 100M tokens. The hero's class and form (slime or human) are the
// user's picks; its gear grows with the level (`tierOf`). Pure, apart from
// the localStorage helpers at the bottom (a per-machine view preference,
// like the office theme).

import type { UsageRow } from "../tokenmonitor/model";

export const TOKENS_PER_LEVEL = 100_000_000;

export const HERO_CLASSES = ["warrior", "mage", "rogue", "cleric", "cat"] as const;
export type HeroClass = (typeof HERO_CLASSES)[number];

/// Levels where the hero's look changes: plain, starter gear, full gear,
/// jewelled gear, golden gear, golden gear with an aura.
export const TIER_LEVELS = [0, 1, 5, 10, 25, 50] as const;
export type Tier = 0 | 1 | 2 | 3 | 4 | 5;

export interface Level {
  level: number;
  /// Tokens into the current level, and the share of it done (0..1).
  xp: number;
  progress: number;
}

export function levelOf(tokens: number): Level {
  const t = Number.isFinite(tokens) && tokens > 0 ? tokens : 0;
  const level = Math.floor(t / TOKENS_PER_LEVEL);
  const xp = t - level * TOKENS_PER_LEVEL;
  return { level, xp, progress: xp / TOKENS_PER_LEVEL };
}

export function tierOf(level: number): Tier {
  let tier = 0;
  TIER_LEVELS.forEach((min, i) => {
    if (level >= min) tier = i;
  });
  return tier as Tier;
}

/// The level where the look changes next, or null at the last tier.
export function nextTierLevel(level: number): number | null {
  return TIER_LEVELS.find((min) => min > level) ?? null;
}

/// All-time tokens per account id ("" default, "none" home, else a profile).
export function accountXp(rows: readonly UsageRow[]): Map<string, number> {
  const out = new Map<string, number>();
  for (const r of rows) out.set(r.account, (out.get(r.account) ?? 0) + r.all_time.total);
  return out;
}

/// A hero is drawn as a slime or as a human adventurer.
export const HERO_FORMS = ["slime", "human"] as const;
export type HeroForm = (typeof HERO_FORMS)[number];

export function isHeroForm(v: unknown): v is HeroForm {
  return typeof v === "string" && (HERO_FORMS as readonly string[]).includes(v);
}

export function isHeroClass(v: unknown): v is HeroClass {
  return typeof v === "string" && (HERO_CLASSES as readonly string[]).includes(v);
}

/// Accounts whose level went up since `seen` (the levels last shown). An
/// account seen for the first time is not a level-up.
export function levelUps(seen: Readonly<Record<string, number>>, now: ReadonlyMap<string, number>): { account: string; level: number }[] {
  const out: { account: string; level: number }[] = [];
  for (const [account, level] of now) {
    const before = seen[account];
    if (before !== undefined && level > before) out.push({ account, level });
  }
  return out;
}

// ── Saved picks and last-seen levels ─────────────────────────────────────

const CLASS_KEY = "ymux.office.heroClass";
const SEEN_KEY = "ymux.office.heroLevels";
const FORM_KEY = "ymux.office.heroForm";

function readJson(key: string): Record<string, unknown> {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(key) ?? "{}");
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* localStorage unavailable */
  }
}

/// The class picked for `account`; warriors by default.
export function loadHeroClass(account: string): HeroClass {
  const v = readJson(CLASS_KEY)[account];
  return isHeroClass(v) ? v : "warrior";
}

export function saveHeroClass(account: string, cls: HeroClass): void {
  writeJson(CLASS_KEY, { ...readJson(CLASS_KEY), [account]: cls });
}

/// The form picked for `account`; slimes by default.
export function loadHeroForm(account: string): HeroForm {
  const v = readJson(FORM_KEY)[account];
  return isHeroForm(v) ? v : "slime";
}

export function saveHeroForm(account: string, form: HeroForm): void {
  writeJson(FORM_KEY, { ...readJson(FORM_KEY), [account]: form });
}

export function loadSeenLevels(): Record<string, number> {
  const out: Record<string, number> = {};
  for (const [k, v] of Object.entries(readJson(SEEN_KEY))) if (typeof v === "number" && Number.isFinite(v)) out[k] = v;
  return out;
}

export function saveSeenLevels(levels: ReadonlyMap<string, number>): void {
  writeJson(SEEN_KEY, { ...loadSeenLevels(), ...Object.fromEntries(levels) });
}
