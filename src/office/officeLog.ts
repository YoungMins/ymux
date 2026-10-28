// What the office remembers between glances: the lobby board's notes
// (agents finishing, with what the turn did; work handed from one place to
// another), each pane's mailbox, and since when each agent has been in its
// status and on its tool (for wait times and stuck agents). A letter arrives sealed, is opened when the pane's agent starts
// working, and leaves the box when that turn ends. Kept for
// the app's lifetime only, never written to disk; fed by WorkspaceManager
// whether or not the office is open, and read by OfficeView.

import type { Uuid } from "../types";
import type { OfficeMessageKind } from "./officeEvents";

export type LogKind = "done" | OfficeMessageKind;

export interface LogEntry {
  id: number;
  /// Epoch ms.
  at: number;
  kind: LogKind;
  /// The pane it happened in (done) or was delivered to (a message).
  paneId: Uuid;
  /// Its label when it happened (the pane may be gone since).
  label: string;
  /// The agent kind there ("claude", …), if any.
  agent: string | null;
  /// A message's sender pane and label; null when the user sent it.
  from: Uuid | null;
  fromLabel: string | null;
  /// A message's first line ("" for a finished turn).
  snippet: string;
  /// A finished turn: how long it took, and what the activity log says it
  /// did (when the log is on).
  turn?: TurnFacts;
}

export interface TurnFacts {
  durationMs: number | null;
  headline: string;
  files: number | null;
  add: number | null;
  del: number | null;
  /// The turn's test run passed (true), failed (false), or none ran.
  test?: boolean | null;
}

export interface Mail {
  id: number;
  at: number;
  kind: OfficeMessageKind;
  from: Uuid | null;
  fromLabel: string | null;
  /// The first line of what was sent, clipped.
  snippet: string;
  /// The agent has started working since it arrived: an opened letter.
  opened: boolean;
}

/// Notes kept on the board; older ones fall off.
export const LOG_LIMIT = 40;
/// Letters kept per mailbox.
export const MAIL_LIMIT = 9;
const SNIPPET = 60;

/// First non-empty line of `text`, whitespace collapsed, clipped.
export function snippetOf(text: string): string {
  const line = text.split(/\r?\n/).map((l) => l.trim()).find((l) => l.length > 0) ?? "";
  const flat = line.replace(/\s+/g, " ");
  return flat.length > SNIPPET ? `${flat.slice(0, SNIPPET - 1)}…` : flat;
}

export class OfficeLog {
  private entries: LogEntry[] = [];
  private mail = new Map<Uuid, Mail[]>();
  private statuses = new Map<Uuid, { status: string; at: number }>();
  private tools = new Map<Uuid, { tool: string | null; at: number }>();
  /// Tool calls seen in each pane's current working turn.
  private calls = new Map<Uuid, number>();
  private listeners = new Set<() => void>();
  private nextId = 1;

  constructor(private readonly now: () => number = () => Date.now()) {}

  onChange(cb: () => void): () => void {
    this.listeners.add(cb);
    return () => {
      this.listeners.delete(cb);
    };
  }

  private changed(): void {
    for (const cb of this.listeners) {
      try {
        cb();
      } catch (e) {
        console.warn("office log listener failed:", e);
      }
    }
  }

  /// Newest first.
  list(): readonly LogEntry[] {
    return this.entries;
  }

  mailOf(paneId: Uuid): readonly Mail[] {
    return this.mail.get(paneId) ?? [];
  }

  /// An agent finished in `paneId`, after `durationMs` of work if known.
  done(paneId: Uuid, label: string, agent: string | null, durationMs: number | null = null): void {
    this.push({
      kind: "done",
      paneId,
      label,
      agent,
      from: null,
      fromLabel: null,
      snippet: "",
      turn: { durationMs, headline: "", files: null, add: null, del: null },
    });
    this.changed();
  }

  /// What the activity log says `paneId`'s last turn did, onto its newest
  /// "done" note.
  describeTurn(paneId: Uuid, facts: Partial<TurnFacts>): void {
    const i = this.entries.findIndex((e) => e.kind === "done" && e.paneId === paneId);
    if (i < 0) return;
    const e = this.entries[i];
    const turn = { ...(e.turn ?? { durationMs: null, headline: "", files: null, add: null, del: null }) };
    for (const [k, v] of Object.entries(facts)) if (v !== null && v !== undefined && v !== "") (turn as Record<string, unknown>)[k] = v;
    this.entries = this.entries.map((x, j) => (j === i ? { ...x, turn } : x));
    this.changed();
  }

  /// The agent in `paneId` is in `status` with `tool`: remembered with the
  /// time each last changed. Null status: no agent there any more.
  track(paneId: Uuid, status: string | null, tool: string | null): void {
    if (status === null) {
      this.statuses.delete(paneId);
      this.tools.delete(paneId);
      this.calls.delete(paneId);
      return;
    }
    if (this.statuses.get(paneId)?.status !== status) {
      this.statuses.set(paneId, { status, at: this.now() });
      if (status === "working") this.calls.set(paneId, 0);
    }
    const t = this.tools.get(paneId);
    if (!t || t.tool !== tool) {
      this.tools.set(paneId, { tool, at: this.now() });
      if (tool) this.calls.set(paneId, (this.calls.get(paneId) ?? 0) + 1);
    }
  }

  /// Tool calls (changes of tool) in `paneId`'s current working turn.
  toolCalls(paneId: Uuid): number {
    return this.calls.get(paneId) ?? 0;
  }

  /// When the agent in `paneId` entered its current status, or null.
  statusSince(paneId: Uuid): number | null {
    return this.statuses.get(paneId)?.at ?? null;
  }

  /// When the agent in `paneId` started on its current tool, or null.
  toolSince(paneId: Uuid): number | null {
    return this.tools.get(paneId)?.at ?? null;
  }

  /// Work was handed to `paneId`: a note on the board and a sealed letter
  /// in its mailbox.
  message(
    m: { kind: OfficeMessageKind; to: Uuid; label: string; agent: string | null; from: Uuid | null; fromLabel: string | null },
    text: string,
  ): void {
    this.push({ kind: m.kind, paneId: m.to, label: m.label, agent: m.agent, from: m.from, fromLabel: m.fromLabel, snippet: snippetOf(text) });
    const box = [
      { id: this.nextId++, at: this.now(), kind: m.kind, from: m.from, fromLabel: m.fromLabel, snippet: snippetOf(text), opened: false },
      ...(this.mail.get(m.to) ?? []),
    ].slice(0, MAIL_LIMIT);
    this.mail.set(m.to, box);
    this.changed();
  }

  /// The agent in `paneId` started working: it has opened its letters.
  agentWorking(paneId: Uuid): void {
    const box = this.mail.get(paneId);
    if (!box || box.every((m) => m.opened)) return;
    this.mail.set(paneId, box.map((m) => ({ ...m, opened: true })));
    this.changed();
  }

  /// The agent in `paneId` stopped working: the letters it opened are dealt
  /// with; ones that came in since stay sealed.
  agentStopped(paneId: Uuid): void {
    const box = this.mail.get(paneId);
    if (!box || !box.some((m) => m.opened)) return;
    const left = box.filter((m) => !m.opened);
    if (left.length > 0) this.mail.set(paneId, left);
    else this.mail.delete(paneId);
    this.changed();
  }

  /// The pane went away.
  clearMail(paneId: Uuid): void {
    if (this.mail.delete(paneId)) this.changed();
  }

  private push(e: Omit<LogEntry, "id" | "at">): void {
    this.entries = [{ ...e, id: this.nextId++, at: this.now() }, ...this.entries].slice(0, LOG_LIMIT);
  }
}

/// The app's one log.
export const officeLog = new OfficeLog();
