// The office's games: what the user and their agents did, counted, and
// turned into achievements (150-odd, in families with rising thresholds),
// titles, coins, a shop, daily quests, a weekly report, the building's size
// and the season. Pure: `funStore` keeps the stats in localStorage and feeds
// events in; the view reads the results.

/// Something worth counting.
export type FunEvent =
  /// An agent finished a turn (`ms`: how long it worked, if known).
  | { t: "turn"; kind: string; ms: number | null; at: number }
  /// What that turn did, from the activity log.
  | { t: "facts"; pane: string; files: number | null; add: number | null; del: number | null; test: boolean | null }
  | { t: "tool"; tool: string }
  /// Agents working at once, and helpers working for one lead (`started`: new ones).
  | { t: "concurrent"; n: number }
  | { t: "subs"; n: number; started: number }
  | { t: "approve"; n: number }
  /// A waiting agent answered, `waitMs` after it started waiting.
  | { t: "answer"; waitMs: number | null }
  | { t: "theme"; id: string }
  | { t: "floor"; id: string }
  | { t: "level"; level: number }
  | { t: "class"; id: string }
  | { t: "form"; id: string }
  | { t: "quota"; left: number }
  | { t: "open"; at: number }
  | { t: "event"; kind: string }
  | {
      t: "count";
      what: "review" | "handoff" | "pet" | "mini" | "launch" | "queue" | "report" | "chat";
    };

export interface DayStats {
  /// Counters for the day (turns, add, del, reviews, …).
  c: Record<string, number>;
  /// Turns per agent kind.
  kinds: Record<string, number>;
  /// Achievements unlocked that day.
  unlocked: string[];
}

export interface Quests {
  day: string;
  ids: string[];
  done: string[];
}

export interface FunStats {
  v: 1;
  counters: Record<string, number>;
  maxes: Record<string, number>;
  sets: Record<string, string[]>;
  days: Record<string, DayStats>;
  /// Achievement id → when it was unlocked.
  unlocked: Record<string, number>;
  coins: number;
  /// Shop items bought.
  owned: string[];
  /// The achievement worn as a title ("" none).
  title: string;
  quests: Quests;
  /// Last test result per pane (a failing test made to pass is a bug hunted).
  lastTest: Record<string, boolean>;
  /// The week whose report was last shown by itself.
  reportWeek: string;
  /// Where the user put bought furniture: a room's id (null: the lobby) and
  /// the spot in it, as shares of its width and height.
  places: Record<string, Place>;
  /// The activity log is on (quests that need it are only offered then).
  activity: boolean;
}

export interface Place {
  room: string | null;
  x: number;
  y: number;
}

export function emptyStats(): FunStats {
  return {
    v: 1,
    counters: {},
    maxes: {},
    sets: {},
    days: {},
    unlocked: {},
    coins: 0,
    owned: [],
    title: "",
    quests: { day: "", ids: [], done: [] },
    lastTest: {},
    reportWeek: "",
    places: {},
    activity: true,
  };
}

/// Whatever came out of storage, made safe to use.
export function normalizeStats(v: unknown): FunStats {
  const s = emptyStats();
  if (!v || typeof v !== "object") return s;
  const o = v as Partial<FunStats>;
  const rec = <T>(x: unknown, ok: (y: unknown) => y is T): Record<string, T> => {
    const out: Record<string, T> = {};
    if (x && typeof x === "object" && !Array.isArray(x)) for (const [k, y] of Object.entries(x)) if (ok(y)) out[k] = y;
    return out;
  };
  const num = (y: unknown): y is number => typeof y === "number" && Number.isFinite(y);
  const strs = (y: unknown): y is string[] => Array.isArray(y) && y.every((z) => typeof z === "string");
  s.counters = rec(o.counters, num);
  s.maxes = rec(o.maxes, num);
  s.sets = rec(o.sets, strs);
  s.unlocked = rec(o.unlocked, num);
  s.lastTest = rec(o.lastTest, (y): y is boolean => typeof y === "boolean");
  s.days = rec(o.days, (y): y is DayStats => !!y && typeof y === "object");
  for (const d of Object.values(s.days)) {
    d.c = rec(d.c, num);
    d.kinds = rec(d.kinds, num);
    d.unlocked = strs(d.unlocked) ? d.unlocked : [];
  }
  s.coins = num(o.coins) ? Math.max(0, o.coins) : 0;
  s.owned = strs(o.owned) ? o.owned : [];
  s.title = typeof o.title === "string" ? o.title : "";
  const q = o.quests;
  if (q && typeof q.day === "string" && strs(q.ids) && strs(q.done)) s.quests = { day: q.day, ids: q.ids, done: q.done };
  s.reportWeek = typeof o.reportWeek === "string" ? o.reportWeek : "";
  s.activity = typeof o.activity === "boolean" ? o.activity : true;
  s.places = rec(o.places, (y): y is Place => {
    const p = y as Place;
    return !!p && typeof p === "object" && (p.room === null || typeof p.room === "string") && num(p.x) && num(p.y);
  });
  return s;
}

// ── Time ────────────────────────────────────────────────────────────────

/// Local calendar day, "2026-10-28".
export function dayKey(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/// The Monday starting the week of `ms` (local), as a day key.
export function weekKey(ms: number): string {
  const d = new Date(ms);
  const back = (d.getDay() + 6) % 7;
  return dayKey(new Date(d.getFullYear(), d.getMonth(), d.getDate() - back).getTime());
}

/// Consecutive days with a finished turn, ending today or yesterday.
export function streak(days: Readonly<Record<string, DayStats>>, now: number): number {
  const active = (ms: number) => (days[dayKey(ms)]?.c.turns ?? 0) > 0;
  const d = new Date(now);
  let cursor = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  if (!active(cursor)) cursor -= 86_400_000;
  let n = 0;
  while (active(cursor) && n < 10_000) {
    n++;
    // Step back a calendar day (noon avoids DST edges).
    const c = new Date(cursor);
    cursor = new Date(c.getFullYear(), c.getMonth(), c.getDate() - 1, 12).getTime();
    const c2 = new Date(cursor);
    cursor = new Date(c2.getFullYear(), c2.getMonth(), c2.getDate()).getTime();
  }
  return n;
}

export type Season = "xmas" | "halloween" | "newyear" | "spring" | null;

/// The season the office dresses for on `ms`'s date.
export function seasonOf(ms: number): Season {
  const d = new Date(ms);
  const m = d.getMonth() + 1;
  const day = d.getDate();
  if (m === 12 && day >= 1) return "xmas";
  if (m === 1 && day <= 7) return "newyear";
  if (m === 10 && day >= 20) return "halloween";
  if ((m === 3 && day >= 20) || (m === 4 && day <= 20)) return "spring";
  return null;
}

export type DayPart = "day" | "evening" | "night";

export function dayPart(ms: number): DayPart {
  const h = new Date(ms).getHours();
  if (h >= 20 || h < 6) return "night";
  if (h >= 17) return "evening";
  return "day";
}

/// Friday afternoon: pizza in the lobby.
export function pizzaTime(ms: number): boolean {
  const d = new Date(ms);
  return d.getDay() === 5 && d.getHours() >= 15;
}

// ── Achievements ────────────────────────────────────────────────────────

export interface Family {
  id: string;
  icon: string;
  /// The number the thresholds are compared with.
  metric: (s: FunStats, now: number) => number;
  steps: readonly number[];
}

const c = (k: string) => (s: FunStats) => s.counters[k] ?? 0;
const mx = (k: string) => (s: FunStats) => s.maxes[k] ?? 0;
const set = (k: string) => (s: FunStats) => s.sets[k]?.length ?? 0;

export const FAMILIES: readonly Family[] = [
  { id: "turns", icon: "✔", metric: c("turns"), steps: [1, 10, 50, 100, 250, 500, 1000, 2500, 5000] },
  { id: "add", icon: "➕", metric: c("add"), steps: [100, 1000, 10000, 50000, 100000] },
  { id: "del", icon: "➖", metric: c("del"), steps: [100, 1000, 10000, 50000] },
  { id: "files", icon: "📄", metric: c("files"), steps: [10, 100, 1000, 5000] },
  { id: "reviews", icon: "🔍", metric: c("review"), steps: [1, 10, 50, 100] },
  { id: "approvals", icon: "👍", metric: c("approvals"), steps: [1, 10, 100, 500] },
  { id: "approveAll", icon: "✅", metric: c("approveAll"), steps: [1, 10, 50] },
  { id: "handoffs", icon: "📨", metric: c("handoff"), steps: [1, 10, 50] },
  { id: "quick", icon: "⚡", metric: c("quick"), steps: [1, 25, 100, 500] },
  { id: "answers", icon: "💬", metric: c("answers"), steps: [10, 100, 1000] },
  { id: "night", icon: "🌙", metric: c("night"), steps: [1, 10, 50] },
  { id: "early", icon: "🌅", metric: c("early"), steps: [1, 10, 50] },
  { id: "weekend", icon: "🏖", metric: c("weekend"), steps: [1, 10, 50] },
  { id: "midnight", icon: "🕛", metric: c("midnight"), steps: [1] },
  { id: "streak", icon: "🔥", metric: mx("streak"), steps: [3, 7, 14, 30, 100] },
  { id: "days", icon: "📅", metric: (s) => Object.values(s.days).filter((d) => (d.c.turns ?? 0) > 0).length + (s.counters.oldDays ?? 0), steps: [10, 50, 100, 365] },
  { id: "tests", icon: "🧪", metric: c("testPass"), steps: [1, 10, 100, 500] },
  { id: "bughunt", icon: "🐛", metric: c("bughunt"), steps: [1, 10, 50] },
  { id: "cleanup", icon: "🧹", metric: c("cleanup"), steps: [1, 10, 25] },
  { id: "bigdiff", icon: "🏗", metric: c("bigdiff"), steps: [1, 10] },
  { id: "tiny", icon: "🔧", metric: c("tiny"), steps: [1, 25, 100] },
  { id: "marathon", icon: "🏃", metric: c("marathon"), steps: [1, 10, 50] },
  { id: "speed", icon: "💨", metric: c("speed"), steps: [1, 25, 100] },
  { id: "bosses", icon: "👹", metric: c("bosses"), steps: [1, 10, 50] },
  { id: "concurrent", icon: "👥", metric: mx("concurrent"), steps: [2, 4, 8] },
  { id: "subsMax", icon: "🌱", metric: mx("subs"), steps: [3, 5, 10] },
  { id: "subsTotal", icon: "🌳", metric: c("subagents"), steps: [10, 100, 1000] },
  { id: "toolRead", icon: "📖", metric: c("tool:read"), steps: [100, 1000, 10000] },
  { id: "toolBash", icon: "⚙", metric: c("tool:bash"), steps: [100, 1000, 10000] },
  { id: "toolEdit", icon: "✏", metric: c("tool:edit"), steps: [100, 1000, 10000] },
  { id: "toolWeb", icon: "🌐", metric: c("tool:web"), steps: [10, 100, 1000] },
  { id: "themes", icon: "🎨", metric: set("themes"), steps: [2, 4, 8] },
  { id: "pets", icon: "🐾", metric: c("pet"), steps: [1, 50, 500] },
  { id: "coins", icon: "🪙", metric: c("coinsEarned"), steps: [100, 1000, 10000, 50000] },
  { id: "items", icon: "🛍", metric: (s) => s.owned.length, steps: [1, 5, 10, 20] },
  { id: "quests", icon: "📜", metric: c("quests"), steps: [1, 10, 50, 100] },
  { id: "reports", icon: "📰", metric: c("report"), steps: [1, 10] },
  { id: "level", icon: "⭐", metric: mx("level"), steps: [1, 5, 10, 25, 50, 75, 100] },
  { id: "classes", icon: "🎭", metric: set("classes"), steps: [5] },
  { id: "forms", icon: "🧍", metric: set("forms"), steps: [2] },
  { id: "mini", icon: "▣", metric: c("mini"), steps: [1, 50] },
  { id: "floors", icon: "🏢", metric: set("floors"), steps: [2, 5] },
  { id: "launch", icon: "🚀", metric: c("launch"), steps: [1, 10] },
  { id: "kinds", icon: "🤖", metric: set("kinds"), steps: [2, 3, 5] },
  { id: "chats", icon: "🗨", metric: c("chat"), steps: [1, 25, 100] },
  { id: "seasons", icon: "🎄", metric: set("seasons"), steps: [1, 4] },
  { id: "events", icon: "🎲", metric: set("events"), steps: [1, 4] },
  { id: "fumes", icon: "🪫", metric: c("fumes"), steps: [1] },
  { id: "friday", icon: "🍕", metric: c("friday"), steps: [1, 10] },
  { id: "queue", icon: "🚦", metric: c("queue"), steps: [10, 100] },
];

export interface Achievement {
  id: string;
  family: Family;
  /// 0-based rank within its family.
  rank: number;
  target: number;
  /// Coins it pays.
  reward: number;
}

export const ACHIEVEMENTS: readonly Achievement[] = FAMILIES.flatMap((family) =>
  family.steps.map((target, rank) => ({ id: `${family.id}:${target}`, family, rank, target, reward: 20 + rank * 15 })),
);

export function achievementById(id: string): Achievement | undefined {
  return ACHIEVEMENTS.find((a) => a.id === id);
}

/// Roman numeral for a rank (I, II, …).
export function roman(n: number): string {
  const table: [number, string][] = [[10, "X"], [9, "IX"], [5, "V"], [4, "IV"], [1, "I"]];
  let out = "";
  let v = n;
  for (const [k, r] of table) while (v >= k) {
    out += r;
    v -= k;
  }
  return out;
}

/// Achievements reached by `s` and not yet unlocked.
export function newlyReached(s: FunStats, now: number): Achievement[] {
  const metrics = new Map<string, number>();
  return ACHIEVEMENTS.filter((a) => {
    if (s.unlocked[a.id] !== undefined) return false;
    let m = metrics.get(a.family.id);
    if (m === undefined) metrics.set(a.family.id, (m = a.family.metric(s, now)));
    return m >= a.target;
  });
}

/// How far `s` is towards `a` (0..1) and the number shown.
export function progressOf(s: FunStats, a: Achievement, now: number): { value: number; share: number } {
  const value = a.family.metric(s, now);
  return { value, share: Math.max(0, Math.min(1, value / a.target)) };
}

// ── Coins, shop ─────────────────────────────────────────────────────────

/// What a finished turn pays.
export const TURN_COINS = 5;
export const QUEST_COINS = 30;
export const BOSS_COINS = 40;
/// A turn this long brought a boss into the room.
export const BOSS_MS = 10 * 60 * 1000;

/// Where a bought item goes; "use" items are spent at once (a party,
/// snacks, fireworks) and can be bought again.
export type ItemPlace = "lobby" | "room" | "wall" | "pet" | "use";

export interface ShopItem {
  id: string;
  price: number;
  place: ItemPlace;
}

export const SHOP: readonly ShopItem[] = [
  { id: "coffee", price: 60, place: "lobby" },
  { id: "aquarium", price: 150, place: "lobby" },
  { id: "vending", price: 120, place: "lobby" },
  { id: "arcade", price: 300, place: "lobby" },
  { id: "piano", price: 400, place: "lobby" },
  { id: "fountain", price: 600, place: "lobby" },
  { id: "pingpong", price: 250, place: "room" },
  { id: "trophy", price: 200, place: "room" },
  { id: "bonsai", price: 80, place: "room" },
  { id: "lamp", price: 70, place: "room" },
  { id: "beanbag", price: 90, place: "room" },
  { id: "statue", price: 800, place: "room" },
  { id: "poster", price: 40, place: "wall" },
  { id: "clock", price: 50, place: "wall" },
  { id: "neon", price: 180, place: "wall" },
  { id: "map", price: 110, place: "wall" },
  { id: "penguin", price: 350, place: "pet" },
  { id: "turtle", price: 300, place: "pet" },
  { id: "blackcat", price: 250, place: "pet" },
  { id: "snacks", price: 40, place: "use" },
  { id: "party", price: 150, place: "use" },
  { id: "fireworks", price: 220, place: "use" },
];

/// This week's special: one furniture piece or pet at 30% off (seeded by
/// the week, so it is the same all week).
export function weeklySpecial(now: number): string {
  const pool = SHOP.filter((i) => i.place !== "use");
  let h = 2166136261;
  for (const ch of weekKey(now)) h = Math.imul(h ^ ch.charCodeAt(0), 16777619) >>> 0;
  return pool[h % pool.length].id;
}

/// What `id` costs at `now` (the weekly special is cheaper).
export function priceOf(item: ShopItem, now: number): number {
  return item.id === weeklySpecial(now) ? Math.round(item.price * 0.7) : item.price;
}

/// Achievement families counted from the activity log (diffs and tests):
/// they stand still while the log is off.
export const ACTIVITY_FAMILIES: ReadonlySet<string> = new Set(["add", "del", "files", "tests", "bughunt", "cleanup", "bigdiff", "tiny"]);
/// Quest counters that need the activity log.
const ACTIVITY_KEYS: ReadonlySet<string> = new Set(["add", "testPass"]);

export function shopItem(id: string): ShopItem | undefined {
  return SHOP.find((i) => i.id === id);
}

// ── Building ────────────────────────────────────────────────────────────

/// The building grows with finished turns: garage to empire.
export const BUILDING_STEPS = [0, 10, 50, 200, 500, 1000, 2500] as const;
export const BUILDING_NAMES = ["garage", "startup", "office", "company", "hq", "tower", "empire"] as const;

export function buildingOf(turns: number): { step: number; name: (typeof BUILDING_NAMES)[number]; next: number | null } {
  let step = 0;
  BUILDING_STEPS.forEach((min, i) => {
    if (turns >= min) step = i;
  });
  return { step, name: BUILDING_NAMES[step], next: BUILDING_STEPS[step + 1] ?? null };
}

// ── Daily quests ────────────────────────────────────────────────────────

export interface QuestDef {
  id: string;
  /// The day counter it watches, and how much of it.
  key: string;
  target: number;
}

export const QUEST_POOL: readonly QuestDef[] = [
  { id: "turns3", key: "turns", target: 3 },
  { id: "turns10", key: "turns", target: 10 },
  { id: "review1", key: "review", target: 1 },
  { id: "review3", key: "review", target: 3 },
  { id: "quick3", key: "quick", target: 3 },
  { id: "answers5", key: "answers", target: 5 },
  { id: "approve5", key: "approvals", target: 5 },
  { id: "handoff1", key: "handoff", target: 1 },
  { id: "pet3", key: "pet", target: 3 },
  { id: "lines200", key: "add", target: 200 },
  { id: "tests2", key: "testPass", target: 2 },
  { id: "queue3", key: "queue", target: 3 },
  { id: "mini1", key: "mini", target: 1 },
  { id: "launch1", key: "launch", target: 1 },
];

/// Three quests for `day`, the same all day (seeded by the date).
export function questsFor(day: string, activity = true): string[] {
  let h = 2166136261;
  for (const ch of day) h = Math.imul(h ^ ch.charCodeAt(0), 16777619) >>> 0;
  // Without the activity log, no quest that only it could count.
  const pool = QUEST_POOL.filter((q) => activity || !ACTIVITY_KEYS.has(q.key));
  const out: string[] = [];
  const keys = new Set<string>();
  while (out.length < 3 && pool.length > 0) {
    h = Math.imul(h ^ (h >>> 13), 0x5bd1e995) >>> 0;
    const q = pool.splice(h % pool.length, 1)[0];
    // One quest per counter a day.
    if (keys.has(q.key)) continue;
    keys.add(q.key);
    out.push(q.id);
  }
  return out;
}

export function questDef(id: string): QuestDef | undefined {
  return QUEST_POOL.find((q) => q.id === id);
}

// ── Applying events ─────────────────────────────────────────────────────

/// What applying an event led to, for the view to celebrate.
export interface FunResult {
  unlocked: Achievement[];
  quests: string[];
  coins: number;
  boss: boolean;
}

function day(s: FunStats, at: number): DayStats {
  const k = dayKey(at);
  let d = s.days[k];
  if (!d) {
    d = s.days[k] = { c: {}, kinds: {}, unlocked: [] };
    // Keep two months; older days only count towards "days worked".
    const keys = Object.keys(s.days).sort();
    while (keys.length > 62) {
      const old = keys.shift()!;
      if ((s.days[old].c.turns ?? 0) > 0) s.counters.oldDays = (s.counters.oldDays ?? 0) + 1;
      delete s.days[old];
    }
  }
  return d;
}

function add(s: FunStats, d: DayStats, key: string, n = 1): void {
  if (!n) return;
  s.counters[key] = (s.counters[key] ?? 0) + n;
  d.c[key] = (d.c[key] ?? 0) + n;
}

function addTo(s: FunStats, key: string, v: string): void {
  const list = s.sets[key] ?? (s.sets[key] = []);
  if (!list.includes(v)) list.push(v);
}

function earn(s: FunStats, d: DayStats, n: number): void {
  s.coins += n;
  add(s, d, "coinsEarned", n);
}

/// Which tool family a tool name counts towards.
export function toolKind(tool: string): "read" | "bash" | "edit" | "web" | null {
  const t = tool.toLowerCase();
  if (/web|fetch|browse/.test(t)) return "web";
  if (/read|grep|glob|^ls$|search|find|view/.test(t)) return "read";
  if (/bash|shell|exec|command/.test(t)) return "bash";
  if (/edit|write|patch|apply/.test(t)) return "edit";
  return null;
}

/// Apply `ev` to `s` (in place) at time `now`: counters, then quests and
/// achievements reached, with their coins.
export function applyEvent(s: FunStats, ev: FunEvent, now: number): FunResult {
  const d = day(s, now);
  let boss = false;
  let coins = 0;
  switch (ev.t) {
    case "turn": {
      add(s, d, "turns");
      d.kinds[ev.kind] = (d.kinds[ev.kind] ?? 0) + 1;
      addTo(s, "kinds", ev.kind);
      const at = new Date(ev.at);
      const h = at.getHours();
      if (h < 5) add(s, d, "night");
      else if (h < 8) add(s, d, "early");
      if (h === 0) add(s, d, "midnight");
      if (at.getDay() === 0 || at.getDay() === 6) add(s, d, "weekend");
      if (at.getDay() === 5 && h >= 17) add(s, d, "friday");
      if (ev.ms !== null) {
        add(s, d, "workMs", ev.ms);
        if (ev.ms >= 30 * 60_000) add(s, d, "marathon");
        if (ev.ms < 30_000) add(s, d, "speed");
        if (ev.ms >= BOSS_MS) {
          add(s, d, "bosses");
          boss = true;
          earn(s, d, BOSS_COINS);
          coins += BOSS_COINS;
        }
      }
      earn(s, d, TURN_COINS);
      coins += TURN_COINS;
      s.maxes.streak = Math.max(s.maxes.streak ?? 0, streak(s.days, now));
      break;
    }
    case "facts": {
      const a = ev.add ?? 0;
      const r = ev.del ?? 0;
      add(s, d, "files", ev.files ?? 0);
      add(s, d, "add", a);
      add(s, d, "del", r);
      if (a + r >= 1000) add(s, d, "bigdiff");
      if (a + r > 0 && a + r <= 3) add(s, d, "tiny");
      if (r >= 30 && r >= 3 * a) add(s, d, "cleanup");
      if (ev.test !== null) {
        if (ev.test) add(s, d, "testPass");
        if (ev.test && s.lastTest[ev.pane] === false) add(s, d, "bughunt");
        s.lastTest[ev.pane] = ev.test;
      }
      const bonus = Math.min(20, Math.floor((a + r) / 100));
      earn(s, d, bonus);
      coins += bonus;
      break;
    }
    case "tool": {
      const k = toolKind(ev.tool);
      if (k) add(s, d, `tool:${k}`);
      break;
    }
    case "concurrent":
      s.maxes.concurrent = Math.max(s.maxes.concurrent ?? 0, ev.n);
      break;
    case "subs":
      s.maxes.subs = Math.max(s.maxes.subs ?? 0, ev.n);
      add(s, d, "subagents", ev.started);
      break;
    case "approve":
      add(s, d, "approvals", ev.n);
      add(s, d, "approveAll");
      break;
    case "answer":
      add(s, d, "answers");
      add(s, d, "approvals");
      if (ev.waitMs !== null && ev.waitMs < 30_000) add(s, d, "quick");
      break;
    case "theme":
      addTo(s, "themes", ev.id);
      break;
    case "floor":
      addTo(s, "floors", ev.id);
      break;
    case "level":
      s.maxes.level = Math.max(s.maxes.level ?? 0, ev.level);
      break;
    case "class":
      addTo(s, "classes", ev.id);
      break;
    case "form":
      addTo(s, "forms", ev.id);
      break;
    case "quota":
      if (ev.left < 10) add(s, d, "fumes");
      break;
    case "open": {
      const season = seasonOf(ev.at);
      if (season) addTo(s, "seasons", season);
      break;
    }
    case "event":
      addTo(s, "events", ev.kind);
      break;
    case "count":
      add(s, d, ev.what);
      break;
  }

  const r = settle(s, now);
  return { ...r, coins: r.coins + coins, boss };
}

/// Today's quests and the achievements `s` has reached, paid out.
export function settle(s: FunStats, now: number): FunResult {
  const d = day(s, now);
  let coins = 0;
  // Today's quests.
  const today = dayKey(now);
  if (s.quests.day !== today) s.quests = { day: today, ids: questsFor(today, s.activity), done: [] };
  const quests: string[] = [];
  for (const id of s.quests.ids) {
    const q = questDef(id);
    if (!q || s.quests.done.includes(id)) continue;
    if ((d.c[q.key] ?? 0) >= q.target) {
      s.quests.done.push(id);
      quests.push(id);
      add(s, d, "quests");
      earn(s, d, QUEST_COINS);
      coins += QUEST_COINS;
    }
  }

  // Achievements, until nothing new (coins earned can unlock more).
  const unlocked: Achievement[] = [];
  for (let round = 0; round < 5; round++) {
    const fresh = newlyReached(s, now);
    if (fresh.length === 0) break;
    for (const a of fresh) {
      s.unlocked[a.id] = now;
      d.unlocked.push(a.id);
      earn(s, d, a.reward);
      coins += a.reward;
      unlocked.push(a);
    }
  }
  return { unlocked, quests, coins, boss: false };
}

/// Buy `id` at `now`'s price: false when it is unknown, owned already or
/// too dear. A "use" item is spent, not kept.
export function buy(s: FunStats, id: string, now = Date.now()): boolean {
  const item = shopItem(id);
  if (!item || s.owned.includes(id)) return false;
  const price = priceOf(item, now);
  if (s.coins < price) return false;
  s.coins -= price;
  if (item.place === "use") s.counters[`used:${id}`] = (s.counters[`used:${id}`] ?? 0) + 1;
  else s.owned.push(id);
  return true;
}

// ── Weekly report ───────────────────────────────────────────────────────

export interface WeekReport {
  week: string;
  turns: number;
  add: number;
  del: number;
  reviews: number;
  quick: number;
  workMs: number;
  coins: number;
  /// The agent kind with the most finished turns, and how many.
  mvp: { kind: string; turns: number } | null;
  /// The busiest day (day key) and its turns.
  busiest: { day: string; turns: number } | null;
  unlocked: string[];
}

/// The 7 days of the week containing `now` (Monday to Sunday).
export function weekReport(s: FunStats, now: number): WeekReport {
  const start = weekKey(now);
  const [y, m, dd] = start.split("-").map(Number);
  const r: WeekReport = { week: start, turns: 0, add: 0, del: 0, reviews: 0, quick: 0, workMs: 0, coins: 0, mvp: null, busiest: null, unlocked: [] };
  const kinds: Record<string, number> = {};
  for (let i = 0; i < 7; i++) {
    const k = dayKey(new Date(y, m - 1, dd + i, 12).getTime());
    const d = s.days[k];
    if (!d) continue;
    const turns = d.c.turns ?? 0;
    r.turns += turns;
    r.add += d.c.add ?? 0;
    r.del += d.c.del ?? 0;
    r.reviews += d.c.review ?? 0;
    r.quick += d.c.quick ?? 0;
    r.workMs += d.c.workMs ?? 0;
    r.coins += d.c.coinsEarned ?? 0;
    r.unlocked.push(...d.unlocked);
    for (const [kind, n] of Object.entries(d.kinds)) kinds[kind] = (kinds[kind] ?? 0) + n;
    if (turns > 0 && (!r.busiest || turns > r.busiest.turns)) r.busiest = { day: k, turns };
  }
  for (const [kind, turns] of Object.entries(kinds)) if (!r.mvp || turns > r.mvp.turns) r.mvp = { kind, turns };
  return r;
}

/// Show the report by itself: Friday from 4 pm, or Monday for last week,
/// once per week.
export function reportDue(s: FunStats, now: number): string | null {
  const d = new Date(now);
  if (d.getDay() === 5 && d.getHours() >= 16) return s.reportWeek === weekKey(now) ? null : weekKey(now);
  if (d.getDay() === 1) {
    const last = weekKey(now - 7 * 86_400_000);
    return s.reportWeek === last || s.reportWeek === weekKey(now) ? null : last;
  }
  return null;
}
