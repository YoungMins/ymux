// What the office remembers between glances: the lobby board's notes
// (agents finishing, work handed from one place to another) and each pane's
// mailbox. A letter arrives sealed, is opened when the pane's agent starts
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

  /// An agent finished in `paneId`.
  done(paneId: Uuid, label: string, agent: string | null): void {
    this.push({ kind: "done", paneId, label, agent, from: null, fromLabel: null });
    this.changed();
  }

  /// Work was handed to `paneId`: a note on the board and a sealed letter
  /// in its mailbox.
  message(
    m: { kind: OfficeMessageKind; to: Uuid; label: string; agent: string | null; from: Uuid | null; fromLabel: string | null },
    text: string,
  ): void {
    this.push({ kind: m.kind, paneId: m.to, label: m.label, agent: m.agent, from: m.from, fromLabel: m.fromLabel });
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
