// The office games' memory: `FunStats` in localStorage (a per-machine
// record, like the theme and the heroes' classes), fed by WorkspaceManager
// (turns, tools, agents at once, whether or not the office is open) and by
// the office view (reviews, answers, pets, …). What an event unlocks is kept
// until the office shows it.

import { applyEvent, buy, isNoop, settle, shopItem, emptyStats, normalizeStats, type Achievement, type FunEvent, type FunStats, type Place } from "./funModel";

const KEY = "ymux.office.fun";
const SETTINGS_KEY = "ymux.office.funSettings";

/// What the office has yet to celebrate.
export interface FunNews {
  unlocked: Achievement[];
  quests: string[];
  bosses: number;
  /// Consumables just bought, to set off in the office.
  used: string[];
}

/// The games' on/off switches.
export interface FunSettings {
  /// Random events, chats between agents, breaks.
  life: boolean;
  /// Night, evening and the seasons.
  ambience: boolean;
  /// Banners for achievements and quests.
  banners: boolean;
  /// Little 8-bit sounds.
  sound: boolean;
}

export const DEFAULT_FUN: FunSettings = { life: true, ambience: true, banners: true, sound: true };

export class FunStore {
  private stats: FunStats;
  private pending: FunNews = { unlocked: [], quests: [], bosses: 0, used: [] };
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
    // Sent on every agent snapshot while agents work; most change nothing.
    if (isNoop(this.stats, ev)) return;
    const before = this.summary();
    const r = applyEvent(this.stats, ev, this.now());
    this.pending.unlocked.push(...r.unlocked);
    this.pending.quests.push(...r.quests);
    if (r.boss) this.pending.bosses++;
    // Most events only move a counter: saved, but nobody is woken for that.
    const news = r.unlocked.length > 0 || r.quests.length > 0 || r.boss;
    this.changed(news || this.summary() !== before);
  }

  /// What the office shows of the stats without opening the games window.
  private summary(): string {
    const s = this.stats;
    return `${s.coins}|${Object.keys(s.unlocked).length}|${s.quests.day}:${s.quests.done.length}|${s.owned.length}|${s.title}|${s.counters.turns ?? 0}`;
  }

  /// Take what has happened since the last call, for the office to show.
  takeNews(): FunNews {
    const n = this.pending;
    this.pending = { unlocked: [], quests: [], bosses: 0, used: [] };
    return n;
  }

  buy(id: string): boolean {
    if (!buy(this.stats, id, this.now())) return false;
    if (shopItem(id)?.place === "use") this.pending.used.push(id);
    // Buying can unlock the collector's achievements.
    const r = settle(this.stats, this.now());
    this.pending.unlocked.push(...r.unlocked);
    this.changed();
    return true;
  }

  /// Put bought `id` at `place` (null: back to where the office puts it).
  setPlace(id: string, place: Place | null): void {
    if (!this.stats.owned.includes(id)) return;
    if (place) this.stats.places[id] = place;
    else delete this.stats.places[id];
    this.changed();
  }

  /// The activity log was switched on or off.
  setActivity(on: boolean): void {
    if (this.stats.activity === on) return;
    this.stats.activity = on;
    this.changed(false);
  }

  /// Replace the whole record (an import), or start over with `null`.
  replace(next: FunStats | null): void {
    this.stats = next ?? emptyStats();
    this.pending = { unlocked: [], quests: [], bosses: 0, used: [] };
    this.changed();
  }

  resetPlaces(): void {
    this.stats.places = {};
    this.changed();
  }

  setTitle(id: string): void {
    this.stats.title = this.stats.unlocked[id] !== undefined ? id : "";
    this.changed();
  }

  markReport(week: string): void {
    this.stats.reportWeek = week;
    this.changed();
  }

  private changed(notify = true): void {
    if (this.saveTimer === null) {
      this.saveTimer = setTimeout(() => {
        this.saveTimer = null;
        save(this.stats);
      }, 500);
    }
    if (!notify) return;
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
      sound: typeof v.sound === "boolean" ? v.sound : true,
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
