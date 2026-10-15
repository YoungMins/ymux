// Per-pane auto-continue state mirrored from the backend (runtime only, never
// persisted): which panes have it on, and a small log of what it answered.
// The backend decides (`src-tauri/src/auto_continue.rs`); this is display.

import type { Uuid } from "../types";

/// One `auto-continue:answered` event from the backend.
export interface AutoAnswerEvent {
  paneId: Uuid;
  /// No option was recommended: the question was sent back to the model.
  denied: boolean;
  /// `[question, answer]` pairs; empty when denied.
  answers: [string, string][];
}

export interface AutoLogEntry {
  at: number;
  denied: boolean;
  answers: [string, string][];
}

/// Entries kept per pane.
export const LOG_LIMIT = 10;

const on = new Set<Uuid>();
const logs = new Map<Uuid, AutoLogEntry[]>();
const listeners = new Set<() => void>();

function notify(): void {
  for (const l of listeners) l();
}

export function isAutoContinue(id: Uuid): boolean {
  return on.has(id);
}

export function onAutoContinueChange(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

/// Mirror the backend's switch. Turning it off keeps the log, so the last
/// answers stay readable.
export function applyAutoContinue(id: Uuid, enabled: boolean): void {
  if (enabled === on.has(id)) return;
  if (enabled) on.add(id);
  else on.delete(id);
  notify();
}

/// The pane's PTY is gone (backend cleared its switch): drop everything.
export function forgetPane(id: Uuid): void {
  const had = on.delete(id);
  logs.delete(id);
  if (had) notify();
}

export function recordAutoAnswer(ev: AutoAnswerEvent, now = Date.now()): void {
  const list = logs.get(ev.paneId) ?? [];
  list.push({ at: now, denied: ev.denied, answers: ev.answers });
  if (list.length > LOG_LIMIT) list.splice(0, list.length - LOG_LIMIT);
  logs.set(ev.paneId, list);
  notify();
}

export function autoLog(id: Uuid): readonly AutoLogEntry[] {
  return logs.get(id) ?? [];
}

/// One log line: `Which db? → Postgres (Recommended)`, several joined by `; `.
export function formatEntry(e: AutoLogEntry, deniedText: string): string {
  const time = new Date(e.at).toLocaleTimeString();
  const body = e.denied ? deniedText : e.answers.map(([q, a]) => `${q} → ${a}`).join("; ");
  return `${time}  ${body}`;
}

/// Test hook: forget everything.
export function resetAutoContinue(): void {
  on.clear();
  logs.clear();
}
