// The office as a control room: who is waiting for the user and for how
// long, which waiting agents only need a "yes", who looks stuck, how much of
// each provider's quota is left, and where a busy agent wanders while it
// works. Pure: the view and WorkspaceManager feed it numbers and screens.

import type { Uuid } from "../types";
import { remainingQuota, type ProviderUsage } from "../tokenmonitor/model";
import { menuOptions, type MenuOption } from "./peekKeys";
import type { Point, Rect } from "./officeModel";

/// A pane whose agent waits for the user.
export interface Waiting {
  paneId: Uuid;
  /// Its workspace.
  ws: number;
  label: string;
  /// Epoch ms it started waiting (null: unknown, sorted last).
  since: number | null;
}

/// Longest waiting first; unknown start times last, in the given order.
export function waitQueue(items: readonly Waiting[]): Waiting[] {
  return items
    .map((w, i) => ({ w, i }))
    .sort((a, b) => (a.w.since ?? Infinity) - (b.w.since ?? Infinity) || a.i - b.i)
    .map(({ w }) => w);
}

/// The pane after `current` in `queue`, wrapping round; the first when
/// `current` is not in it; null for an empty queue.
export function nextWaiting(queue: readonly Waiting[], current: Uuid | null): Waiting | null {
  if (queue.length === 0) return null;
  const i = current === null ? -1 : queue.findIndex((w) => w.paneId === current);
  return queue[(i + 1) % queue.length];
}

/// A short duration: "45s", "12m", "1h 5m" (units from Intl, so localised).
export function shortDuration(ms: number, lang = "en"): string {
  const unit = (n: number, u: "second" | "minute" | "hour") => {
    try {
      return new Intl.NumberFormat(lang, { style: "unit", unit: u, unitDisplay: "narrow" }).format(n);
    } catch {
      return `${n}${u[0]}`;
    }
  };
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return unit(s, "second");
  const m = Math.floor(s / 60);
  if (m < 60) return unit(m, "minute");
  const h = Math.floor(m / 60);
  return m % 60 ? `${unit(h, "hour")} ${unit(m % 60, "minute")}` : unit(h, "hour");
}

/// A first choice that means "go ahead", in the agents' own wording and the
/// translations people run them in.
const YES = /^(yes|y|allow|approve|proceed|accept|ok|continue|run)\b|^(예|네|허용|승인|はい|許可|是|允许|确认|sí|si|oui|sim|да|evet|ja|có)(\b|\s|,|$)/i;

/// The menu choice that approves what a waiting agent asks, if its screen
/// shows a menu whose first choice is a plain "yes" (not one that opens a
/// text field).
export function approveOption(screen: readonly string[] | null): MenuOption | null {
  if (!screen) return null;
  const options = menuOptions(screen);
  const first = options[0];
  return first && !first.input && YES.test(first.label.trim()) ? first : null;
}

/// After this long without output a working agent looks stuck.
export const SILENT_MS = 2 * 60 * 1000;
/// After this long on one tool call a working agent looks stuck.
export const LONG_TOOL_MS = 5 * 60 * 1000;

export type Stuck = "silent" | "tool";

/// Why a working agent looks stuck, or null: no terminal output for a while
/// (`outputAt`), or one tool call running long (`toolSince`, with a tool).
export function stuckReason(a: { working: boolean; tool: string | null; outputAt: number | null; toolSince: number | null; now: number }): Stuck | null {
  if (!a.working) return null;
  if (a.outputAt !== null && a.outputAt > 0 && a.now - a.outputAt >= SILENT_MS) return "silent";
  if (a.tool && a.toolSince !== null && a.now - a.toolSince >= LONG_TOOL_MS) return "tool";
  return null;
}

/// One provider's quota left, for the header's energy bars.
export interface Meter {
  provider: string;
  /// "5h", "7d", or the window in minutes.
  window: string;
  /// Percent left (0..100).
  left: number;
}

function windowName(minutes: number): string {
  if (minutes === 300) return "5h";
  if (minutes === 10080) return "7d";
  if (minutes % 1440 === 0) return `${minutes / 1440}d`;
  if (minutes % 60 === 0) return `${minutes / 60}h`;
  return `${minutes}m`;
}

/// The tightest quota per provider and window across accounts, shortest
/// window first per provider.
export function quotaMeters(providers: readonly ProviderUsage[], now: number): Meter[] {
  const best = new Map<string, Meter & { minutes: number }>();
  for (const p of providers) {
    for (const q of p.quotas) {
      const left = remainingQuota(q, now);
      if (left === null) continue;
      const key = `${p.id}:${q.window_minutes}`;
      const had = best.get(key);
      if (!had || left < had.left) best.set(key, { provider: p.id, window: windowName(q.window_minutes), left: Math.round(left), minutes: q.window_minutes });
    }
  }
  return [...best.values()]
    .sort((a, b) => a.provider.localeCompare(b.provider) || a.minutes - b.minutes)
    .map(({ provider, window, left }) => ({ provider, window, left }));
}

/// What a busy agent goes to look at for `tool`: the bookshelf for reading
/// and searching, the machines for commands, the window for the web, the
/// board for planning, a colleague for a subagent.
export type ErrandIcon = "book" | "gear" | "globe" | "paper" | "chat" | "pencil";

export function errandIcon(tool: string | null): ErrandIcon {
  const t = (tool ?? "").toLowerCase();
  if (/read|grep|glob|search_files|^ls$|list|find|view|cat/.test(t) && !/web/.test(t)) return "book";
  if (/bash|shell|exec|command|run|terminal|kill/.test(t)) return "gear";
  if (/web|fetch|browse|http|url/.test(t)) return "globe";
  if (/todo|plan|note/.test(t)) return "paper";
  if (/task|agent|delegate/.test(t)) return "chat";
  return "pencil";
}

/// Pixels between a room's top and where its floor starts: border and the
/// back wall (`.office-room__wall`, 18 art px of 3 CSS px, plus the border).
export const ROOM_WALL_PX = 60;

/// Where a busy agent walks for an errand: along the back wall of its
/// `room` (fractions of the floor; `floorW` x `floorH` CSS pixels), at a
/// spot picked by `rand` and kept clear of the room's edges.
export function errandSpot(room: Rect, floorW: number, floorH: number, rand: () => number): Point {
  const margin = Math.min(0.2, 40 / Math.max(1, room.w * floorW));
  const x = room.x + room.w * (margin + (1 - 2 * margin) * rand());
  // Feet a little in front of the wall, but never past the room's middle.
  const y = Math.min(room.y + (ROOM_WALL_PX + 36) / Math.max(1, floorH), room.y + room.h * 0.55);
  return { x, y };
}

/// A helper's short wander: a step or two away from its spot, inside its room.
export function wanderSpot(home: Point, room: Rect, floorW: number, floorH: number, rand: () => number): Point {
  const dx = ((rand() - 0.5) * 70) / Math.max(1, floorW);
  const dy = ((rand() - 0.5) * 24) / Math.max(1, floorH);
  const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
  return {
    x: clamp(home.x + dx, room.x + room.w * 0.08, room.x + room.w * 0.92),
    y: clamp(home.y + dy, room.y + room.h * 0.45, room.y + room.h * 0.94),
  };
}
