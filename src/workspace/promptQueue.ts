// Per-pane prompt queue: text lined up for an agent that is still working.
// Pure state + decision; WorkspaceManager owns the wiring (paste, Enter,
// toasts). Runtime-only: nothing here is persisted.
//
// Delivery rule: exactly one prompt per turn end, into a pane whose lead
// agent is "ready" (its status is `done` or `idle`, i.e. the Stop hook has
// fired or the agent sits at its prompt), the screen shows no dialog, and no
// delivery is in flight. A pane with no agent never receives anything: the
// prompts wait until an agent shows up.

import type { AgentSource, AgentStatus, Uuid } from "../types";
import type { PaneStatus } from "../terminal/paneStatus";
import { effectiveLeadStatus } from "./agentTree";

/// What the pane's lead agent is doing, for the delivery decision.
export type AgentPhase = "none" | "working" | "waiting" | "ready";

/// A process-scan lead (Codex, or Claude with agent tracking off) carries no
/// real status from the backend (always "idle"), so its phase comes from the
/// pane's own status: only a pane that is quiet (`idle`) or finished (`done`)
/// is "ready"; `running` is "working", `attention` is "waiting".
export function phaseOf(
  lead: { status: AgentStatus; source: AgentSource } | null | undefined,
  paneStatus: PaneStatus,
): AgentPhase {
  if (!lead) return "none";
  const status = effectiveLeadStatus(lead, paneStatus);
  if (status === "working") return "working";
  if (status === "waiting") return "waiting";
  return "ready";
}

/// The user typed into the focused target pane this recently: a delivery
/// would land in the middle of their sentence.
export const TYPING_QUIET_MS = 4_000;

/// What the caller knows about the user's own input in the target pane.
export interface UserInput {
  /// The pane has keyboard focus.
  focused: boolean;
  /// When the user last typed into it (epoch ms), null = never.
  inputAt: number | null;
}

/// After a delivery the next one waits for the agent to be seen working again
/// (the turn the prompt started). If that never shows (a slash command that
/// finishes at once, a snapshot that missed a short turn) this long after the
/// delivery counts as the turn having ended.
export const REARM_MS = 20_000;
/// A refused delivery (no bracketed paste yet, a dialog) is retried no sooner.
export const RETRY_MS = 3_000;
/// Most prompts one pane holds.
export const MAX_QUEUED = 50;
/// How often WorkspaceManager re-checks a non-empty queue (a refused delivery
/// and the re-arm have no other trigger).
export const QUEUE_TICK_MS = 2_500;

interface Entry {
  items: string[];
  /// A delivery is being written; nothing else may start.
  pending: boolean;
  /// Delivered, and the agent has not been seen working since.
  awaitWork: boolean;
  deliveredAt: number;
  retryAt: number;
  /// A paste went in but its Enter was withheld (or the paste tore): the text
  /// sits unsent in the agent's input box. Nothing else is pasted on top
  /// until the user acts on the pane, the agent is seen working, or the
  /// agent is gone. Epoch ms of the paste, 0 = none.
  unsentAt: number;
}

export type SettleOutcome = "delivered" | "retry";

export class PromptQueues {
  private panes = new Map<Uuid, Entry>();

  /// Queue `text` (trailing whitespace and leading blank lines trimmed; blank
  /// is ignored) at the back. Returns the new length, or -1 when nothing was
  /// added (blank, or the queue is full).
  add(id: Uuid, text: string): number {
    const body = text.replace(/\s+$/, "").replace(/^\s*\n/, "");
    if (body.trim() === "") return -1;
    let e = this.panes.get(id);
    if (!e) {
      e = { items: [], pending: false, awaitWork: false, deliveredAt: 0, retryAt: 0, unsentAt: 0 };
      this.panes.set(id, e);
    }
    if (e.items.length >= MAX_QUEUED) return -1;
    e.items.push(body);
    return e.items.length;
  }

  list(id: Uuid): readonly string[] {
    return this.panes.get(id)?.items ?? [];
  }

  count(id: Uuid): number {
    return this.panes.get(id)?.items.length ?? 0;
  }

  /// Ids of the panes holding at least one prompt.
  queuedPanes(): Uuid[] {
    return [...this.panes].filter(([, e]) => e.items.length > 0).map(([id]) => id);
  }

  /// Drop the prompt at `index`. A prompt being delivered right now (index 0
  /// while pending) can no longer be taken back.
  remove(id: Uuid, index: number): boolean {
    const e = this.panes.get(id);
    if (!e || index < 0 || index >= e.items.length) return false;
    if (e.pending && index === 0) return false;
    e.items.splice(index, 1);
    return true;
  }

  /// Drop every prompt that is not mid-delivery.
  clear(id: Uuid): void {
    const e = this.panes.get(id);
    if (!e) return;
    e.items = e.pending ? e.items.slice(0, 1) : [];
  }

  /// The pane is gone: forget everything, delivery state included.
  forget(id: Uuid): void {
    this.panes.delete(id);
  }

  /// Note the agent's phase. Seeing it work is what re-arms delivery.
  observe(id: Uuid, phase: AgentPhase): void {
    const e = this.panes.get(id);
    if (!e) return;
    if (phase === "working") {
      e.awaitWork = false;
      e.unsentAt = 0;
    } else if (phase === "none") {
      e.unsentAt = 0;
    }
  }

  /// The next prompt to deliver to `id`, or null. A non-null result starts a
  /// delivery (`pending`): the caller must `settle` it. `blocked` is only
  /// asked when everything else allows a delivery.
  claim(
    id: Uuid,
    phase: AgentPhase,
    blocked: () => boolean,
    now: number,
    user: UserInput = { focused: false, inputAt: null },
  ): string | null {
    this.observe(id, phase);
    const e = this.panes.get(id);
    if (!e || e.items.length === 0 || e.pending) return null;
    if (phase !== "ready") return null;
    if (now < e.retryAt) return null;
    // The user acted on the pane since the unsent paste: it is theirs now.
    if (e.unsentAt > 0 && user.inputAt !== null && user.inputAt > e.unsentAt) e.unsentAt = 0;
    if (e.unsentAt > 0) return null;
    if (user.focused && user.inputAt !== null && now - user.inputAt < TYPING_QUIET_MS) return null;
    if (e.awaitWork && now - e.deliveredAt < REARM_MS) return null;
    if (blocked()) return null;
    e.pending = true;
    return e.items[0];
  }

  /// End a delivery started by `claim`. `delivered` consumes the prompt and
  /// waits for the next turn; `retry` keeps it and tries again later. A pane
  /// forgotten meanwhile is ignored. `sent` = false: the text was pasted but
  /// not submitted (it now sits in the input box, see `unsentAt`).
  settle(id: Uuid, outcome: SettleOutcome, now: number, sent = true): void {
    const e = this.panes.get(id);
    if (!e || !e.pending) return;
    e.pending = false;
    if (outcome === "delivered") {
      e.items.shift();
      e.awaitWork = true;
      e.deliveredAt = now;
      if (!sent) e.unsentAt = now;
    } else {
      e.retryAt = now + RETRY_MS;
    }
  }
}

/// Suffix for a tab label: "" or " ▸2".
export function queueSuffix(n: number): string {
  return n > 0 ? ` ▸${n}` : "";
}
