// Pure readiness decision for pasting a prompt into a freshly started agent.
// Callers feed it facts (output timestamps, lead present, bracketed paste,
// agent-snapshot arrival times) and poll; it holds no timers.
//
// The "lead present" fact comes from the 2 s process scan, which can drop a
// lead up to 2 s after the agent exited. So a lead only counts when a snapshot
// arrived AFTER the last output, and the output must have been quiet for more
// than one scan interval: by then a stale lead has been corrected.

/// "blocked": a dialog needs the user; nothing may be written. The timeout
/// clock stands still meanwhile (up to `blockedCapMs`), and the quiet window
/// restarts when it clears.
export type GateState = "wait" | "ready" | "timeout" | "blocked";

/// Longer than the 2 s scan interval, so a stale lead is corrected first.
export const QUIET_MS = 2500;
/// A dialog left unanswered this long ends the wait (state "timeout").
export const BLOCKED_CAP_MS = 30 * 60 * 1000;

export interface ReadyGateOpts {
  quietMs?: number;
  timeoutMs?: number;
  blockedCapMs?: number;
  /// Clock value when the gate starts (default 0; pass `Date.now()`).
  start?: number;
}

export class ReadyGate {
  private readonly quietMs: number;
  private readonly timeoutMs: number;
  private readonly blockedCapMs: number;
  private start: number;
  private lastOutput: number;
  private snapshotAt = -Infinity;
  private lead = false;
  private bracketed = false;
  private blocked = false;
  private blockedSince: number | null = null;
  private lastPoll: number;

  constructor(opts: ReadyGateOpts = {}) {
    this.quietMs = opts.quietMs ?? QUIET_MS;
    this.timeoutMs = opts.timeoutMs ?? 90000;
    this.blockedCapMs = opts.blockedCapMs ?? BLOCKED_CAP_MS;
    this.start = opts.start ?? 0;
    this.lastOutput = this.start;
    this.lastPoll = this.start;
  }

  noteOutput(at: number): void {
    if (at > this.lastOutput) this.lastOutput = at;
  }
  /// An agent snapshot (or scan heartbeat) arrived at `at`.
  noteSnapshot(at: number): void {
    if (at > this.snapshotAt) this.snapshotAt = at;
  }
  setLead(present: boolean): void {
    this.lead = present;
  }
  setBracketed(on: boolean): void {
    this.bracketed = on;
  }

  setBlocked(on: boolean): void {
    this.blocked = on;
  }

  poll(now: number): GateState {
    const elapsed = now - this.lastPoll;
    this.lastPoll = now;
    if (this.blocked) {
      if (this.blockedSince === null) this.blockedSince = now;
      if (now - this.blockedSince >= this.blockedCapMs) return "timeout";
      this.start += Math.max(0, elapsed);
      this.lastOutput = Math.max(this.lastOutput, now);
      return "blocked";
    }
    this.blockedSince = null;
    if (
      this.lead &&
      this.bracketed &&
      this.snapshotAt > this.lastOutput &&
      now - this.lastOutput >= this.quietMs
    ) {
      return "ready";
    }
    if (now - this.start >= this.timeoutMs) return "timeout";
    return "wait";
  }
}

export type EnterDecision = "send" | "wait" | "abort";

/// May the Enter after a paste be written? `fit`: the target is still an
/// agent that is not waiting on a dialog. The Enter additionally needs an
/// agent snapshot that arrived after the paste (`pastedAt`), so a lead the scan
/// has not yet noticed gone cannot receive it; if none arrives within
/// `maxWaitMs`, the Enter is dropped.
export function enterDecision(o: {
  now: number;
  pastedAt: number;
  snapshotAt: number;
  fit: boolean;
  maxWaitMs?: number;
}): EnterDecision {
  if (!o.fit) return "abort";
  if (o.snapshotAt > o.pastedAt) return "send";
  return o.now - o.pastedAt >= (o.maxWaitMs ?? 6000) ? "abort" : "wait";
}

export type ShellReady = "wait" | "ready" | "cap";

/// Has a freshly spawned shell shown its first prompt? Its cwd was reported
/// (OSC 7), or it produced output and then stayed quiet for `quietMs`; gives
/// up ("cap") after `capMs`.
export function shellReady(o: {
  now: number;
  startedAt: number;
  firstOutputAt: number | null;
  lastOutputAt: number;
  cwdSeen: boolean;
  quietMs?: number;
  capMs?: number;
}): ShellReady {
  if (o.cwdSeen) return "ready";
  if (o.firstOutputAt !== null && o.now - o.lastOutputAt >= (o.quietMs ?? 500)) return "ready";
  return o.now - o.startedAt >= (o.capMs ?? 10000) ? "cap" : "wait";
}

/// Shells for which ymux injects (or the shell itself emits) an OSC 7 cwd
/// report at every prompt: bash/zsh (rcfile / ZDOTDIR shim), PowerShell and
/// cmd (prompt), fish (native). Anything else may never report a prompt.
const OSC7_SHELLS = new Set(["bash", "zsh", "fish", "pwsh", "powershell", "cmd"]);
const MERGE_TEXT = /^git merge --no-ff [A-Za-z0-9._/-]+$/;
/// A report this close to the latest output counts as "the prompt is the last
/// thing drawn" (events and data travel on separate channels).
const PROMPT_SETTLE_MS = 250;

/// May `text` be written as plain keystrokes (no bracketed paste) right now?
/// Only the unquoted `git merge --no-ff <branch>` form, and only when the
/// shell's prompt is back: an OSC 7 report newer than the last submit, with
/// output ending right at it. The caller still checks no lead / no dialog /
/// spawned. A single line, never an Enter.
export function plainWriteAllowed(o: {
  text: string;
  shellExecutable: string | null | undefined;
  lastCwdAt: number;
  lastSubmitAt: number;
  lastOutputAt: number;
}): boolean {
  if (!MERGE_TEXT.test(o.text) || o.text.split(" ")[3].startsWith("-")) return false;
  if (!o.shellExecutable) return false;
  const base = o.shellExecutable.split(/[\\/]/).pop()!.toLowerCase();
  const stem = base.endsWith(".exe") ? base.slice(0, -4) : base;
  if (!OSC7_SHELLS.has(stem)) return false;
  if (o.lastCwdAt <= 0 || o.lastCwdAt <= o.lastSubmitAt) return false;
  return o.lastOutputAt - o.lastCwdAt < PROMPT_SETTLE_MS;
}

export type Delivery = "confirmed" | "pending" | "unconfirmed";

/// Did the agent report our submit? `promptSubmitSeenAt` is the time of a
/// `UserPromptSubmit` hook seen for the pane (null = none yet).
export function deliveryCheck(
  submittedAt: number,
  promptSubmitSeenAt: number | null,
  now: number,
  windowMs = 10000,
): Delivery {
  if (promptSubmitSeenAt !== null && promptSubmitSeenAt >= submittedAt) return "confirmed";
  return now - submittedAt >= windowMs ? "unconfirmed" : "pending";
}
