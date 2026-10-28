// The office games' memory: `FunStats` in localStorage (a per-machine
// record, like the theme and the heroes' classes), fed by WorkspaceManager
// (turns, tools, agents at once, whether or not the office is open) and by
// the office view (reviews, answers, pets, …). What an event unlocks is kept
// until the office shows it.

import { applyEvent, buy, settle, emptyStats, normalizeStats, type Achievement, type FunEvent, type FunStats } from "./funModel";

const KEY = "ymux.office.fun";
const SETTINGS_KEY = "ymux.office.funSettings";

/// What the office has yet to celebrate.
export interface FunNews {
  unlocked: Achievement[];
  quests: string[];
  bosses: number;
}

/// The games' on/off switches.
export interface FunSettings {
  /// Random events, chats between agents, breaks.
  life: boolean;
  /// Night, evening and the seasons.
  ambience: boolean;
  /// Banners for achievements and quests.
  banners: boolean;
}

export const DEFAULT_FUN: FunSettings = { life: true, ambience: true, banners: true };

export class FunStore {
  private stats: FunStats;
  private pending: FunNews = { unlocked: [], quests: [], bosses: 0 };
  private listeners = new Set<() => void>();
  private saveTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly now: () => number = () => Date.now()) {
    this.stats = load();
  }

  get(): Readonly<FunStats> {
    return this.stats;
  }

  onChange(cb: () => void): () => void {
    this.listeners.add(cb);
    return () => {
      this.listeners.delete(cb);
    };
  }

  record(ev: FunEvent): void {
    const r = applyEvent(this.stats, ev, this.now());
    this.pending.unlocked.push(...r.unlocked);
    this.pending.quests.push(...r.quests);
    if (r.boss) this.pending.bosses++;
    this.changed();
  }

  /// Take what has happened since the last call, for the office to show.
  takeNews(): FunNews {
    const n = this.pending;
    this.pending = { unlocked: [], quests: [], bosses: 0 };
    return n;
  }

  buy(id: string): boolean {
    if (!buy(this.stats, id)) return false;
    // Buying can unlock the collector's achievements.
    const r = settle(this.stats, this.now());
    this.pending.unlocked.push(...r.unlocked);
    this.changed();
    return true;
  }

  setTitle(id: string): void {
    this.stats.title = this.stats.unlocked[id] !== undefined ? id : "";
    this.changed();
  }

  markReport(week: string): void {
    this.stats.reportWeek = week;
    this.changed();
  }

  private changed(): void {
    if (this.saveTimer === null) {
      this.saveTimer = setTimeout(() => {
        this.saveTimer = null;
        save(this.stats);
      }, 500);
    }
    for (const cb of this.listeners) {
      try {
        cb();
      } catch (e) {
        console.warn("fun listener failed:", e);
      }
    }
  }
}

function load(): FunStats {
  try {
    return normalizeStats(JSON.parse(localStorage.getItem(KEY) ?? "null"));
  } catch {
    return emptyStats();
  }
}

function save(s: FunStats): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    /* localStorage unavailable */
  }
}

export function loadFunSettings(): FunSettings {
  try {
    const v = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}") as Partial<FunSettings>;
    return {
      life: typeof v.life === "boolean" ? v.life : true,
      ambience: typeof v.ambience === "boolean" ? v.ambience : true,
      banners: typeof v.banners === "boolean" ? v.banners : true,
    };
  } catch {
    return { ...DEFAULT_FUN };
  }
}

export function saveFunSettings(s: FunSettings): void {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
  } catch {
    /* localStorage unavailable */
  }
}

/// The app's one store.
export const funStore = new FunStore();
