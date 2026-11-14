// Remote server, pure half: the pane list a phone sees, the prompt/answer
// result tables and the status-line decisions. No DOM, no IPC (rule: pure
// modules are unit-tested; `remoteOps.ts` / `remoteWiring.ts` do the rest).

import { panes as layoutPanes } from "../layout/LayoutTree";
import type { PaneStatus } from "../terminal/paneStatus";
import type {
  AgentSnapshot,
  LayoutNode,
  RemoteCap,
  RemotePane,
  RemoteStatus,
  Uuid,
} from "../types";
import { effectiveLeadStatus } from "../workspace/agentTree";

/// What `buildPaneList` needs from the app. Closures, so the model never
/// touches WorkspaceManager.
export interface RemoteListInput {
  workspaces: readonly { label: string; root: LayoutNode }[];
  agents: AgentSnapshot;
  paneStatus: (id: Uuid) => PaneStatus | undefined;
  title: (id: Uuid) => string;
  /// When the pane entered its status (epoch ms); null/undefined = unknown.
  since: (id: Uuid) => number | null | undefined;
  queued: (id: Uuid) => number;
  /// A live TerminalPane exists for the id (its workspace was hydrated).
  live: (id: Uuid) => boolean;
}

/// Terminal panes of every workspace that a phone can usefully see:
///  - every terminal with a lead agent (even if its workspace is not
///    hydrated, where the agent snapshot still knows it); and
///  - every terminal that is live (workspace hydrated), agent or not, so its
///    screen can be read.
/// Browser/editor/files/git panes are never listed, and neither is a terminal
/// of an unhydrated workspace that runs no agent (nothing to show or drive).
///
/// `status` is the status the UI shows for the lead: the backend's for a
/// hook-fed lead, the pane's for a process-scan lead (`effectiveLeadStatus`),
/// and a pane flagged `attention` counts as waiting, as in the office view's
/// waiting list. "none" when no agent runs there.
export function buildPaneList(input: RemoteListInput): RemotePane[] {
  const out: RemotePane[] = [];
  for (const ws of input.workspaces) {
    for (const spec of layoutPanes(ws.root)) {
      if ((spec.pane_kind ?? "terminal") !== "terminal") continue;
      const lead = input.agents[spec.id]?.lead ?? null;
      if (!lead && !input.live(spec.id)) continue;
      const ps = input.paneStatus(spec.id) ?? "idle";
      let status: RemotePane["status"] = "none";
      if (lead) {
        const eff = effectiveLeadStatus(lead, ps);
        status = ps === "attention" ? "waiting" : eff;
      }
      out.push({
        pane: spec.id,
        ws: ws.label,
        title: input.title(spec.id),
        kind: lead?.kind ?? "",
        status,
        since_ms: input.since(spec.id) ?? 0,
        queued: input.queued(spec.id),
      });
    }
  }
  return out;
}

// ── Result tables ──────────────────────────────────────────────────────────

export type PromptResult = "sent" | "queued" | "waiting" | "blocked" | "gone" | "failed";

/// `WorkspaceManager.sendToPane`'s result as the protocol's `prompt` result.
/// `pasted` (text typed but Enter withheld) has no protocol value: the text
/// did not go out as a prompt, so it is `failed`. `waiting` from sendToPane is
/// a permission prompt or dialog on screen: `blocked`.
export function mapSendResult(r: "sent" | "pasted" | "waiting" | "no-paste" | "gone" | "failed"): PromptResult {
  switch (r) {
    case "sent":
      return "sent";
    case "waiting":
      return "blocked";
    case "gone":
      return "gone";
    default:
      return "failed";
  }
}

/// Longest prompt the protocol allows.
export const MAX_PROMPT_CHARS = 16000;

export type AnswerResult = "sent" | "stale" | "gone" | "failed";

/// Option labels leave the desktop clipped (120) and redacted, and the phone
/// sends them back. A label matches the screen's when equal, or when it is a
/// clipped copy (ends with an ellipsis) that is a prefix of the screen's.
/// A short, unclipped label must match exactly: "Yes" must never pick
/// "Yes, and don't ask again".
export function labelMatches(onScreen: string, got: string): boolean {
  if (onScreen === got) return true;
  const m = /(…|\.\.\.)$/.exec(got);
  if (!m) return false;
  const head = got.slice(0, got.length - m[0].length);
  return head.length >= 20 && onScreen.startsWith(head);
}

// ── Settings status line ───────────────────────────────────────────────────

const KNOWN_ERRORS = ["bad_token", "key_mismatch", "version", "bad_sig", "limit", "origin_mismatch", "secret_store"] as const;

/// Translation key suffix (under `settings.remote.`) for a server error code.
export function errorKey(code: string): string {
  const c = code.trim();
  return (KNOWN_ERRORS as readonly string[]).includes(c) ? `err.${c}` : "err.other";
}

export type StatusTone = "" | "ok" | "warn" | "error";

/// The status line: a translation key (under `settings.remote.`), the tone
/// and, for an unknown error, the raw text to show with it.
export function statusLine(s: Pick<RemoteStatus, "state" | "error">): {
  key: string;
  tone: StatusTone;
  detail: string;
} {
  switch (s.state) {
    case "online":
      return { key: "state.online", tone: "ok", detail: "" };
    case "connecting":
      return { key: "state.connecting", tone: "warn", detail: "" };
    case "error":
      return { key: errorKey(s.error), tone: "error", detail: s.error.trim() };
    case "unavailable":
      return { key: "state.unavailable", tone: "warn", detail: "" };
    default:
      return { key: "state.off", tone: "", detail: "" };
  }
}

/// Whether panes should be published (a server is, or is about to be, there).
export function shouldPublish(state: RemoteStatus["state"] | undefined): boolean {
  return state === "online" || state === "connecting";
}

export const REMOTE_CAPS: readonly RemoteCap[] = ["view", "respond", "prompt"];

export function isRemoteCap(v: string): v is RemoteCap {
  return (REMOTE_CAPS as readonly string[]).includes(v);
}

/// `m:ss` for a countdown; 0 when already past.
export function countdown(expiresMs: number, nowMs: number): string {
  const total = Math.max(0, Math.ceil((expiresMs - nowMs) / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}
