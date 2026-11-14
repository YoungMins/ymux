// Remote server: carrying out one authorised command. The backend has already
// checked the device, its capability and the request's shape; this decides
// what the app does and always returns the protocol's reply data, never
// throws (a failure is the op's failure shape: the backend gives up after 5 s).
//
// Nothing here focuses a pane, switches workspace or waits for a frame, so it
// works for a pane of a background workspace and with the window in the tray.

import { menuOptions, menuQuestion, sameOption } from "../office/peekKeys";
import type { AgentPhase } from "../workspace/promptQueue";
import type { RemotePane, RemoteRequest, Uuid } from "../types";
import { MenuIds, menuBlock, menuFingerprint, type Gens } from "./menuId";
import {
  MAX_PROMPT_CHARS,
  buildPaneList,
  labelMatches,
  mapSendResult,
  type AnswerResult,
  type PromptResult,
  type RemoteListInput,
} from "./remoteModel";

/// The slice of a TerminalPane the remote ops use.
export interface RemoteTerminal {
  /// One string per terminal row (what menu detection reads).
  screenText(history?: number): string[];
  /// One string per logical line: soft-wrapped rows joined (what is sent out).
  screenLogicalText(history?: number): string[];
  screenBlock(): string | null;
  typeText(text: string): void;
}

/// What the app offers the remote ops (implemented by WorkspaceManager's
/// `remote*` methods).
export interface RemoteHost {
  listInput(): RemoteListInput;
  /// The per-pane menu ids (`peek` hands one out, `answer` must echo it).
  menus: MenuIds;
  /// The pane's input generation (`inputGen.ts`): advances on every write to
  /// its terminal. `null` when it cannot be read (an answer is then stale).
  inputGen(id: Uuid): number | null;
  /// The raw lead state of the pane's agent from the agent snapshot (`agentKey`
  /// in `menuId.ts`), not the attention-coerced status. `null` when it cannot
  /// be read (an answer is then stale).
  agentKey(id: Uuid): string | null;
  /// The live terminal of a pane, or null when absent or not yet hydrated.
  terminal(id: Uuid): RemoteTerminal | null;
  phase(id: Uuid): AgentPhase;
  queued(id: Uuid): number;
  /// Add to the pane's prompt queue; false when blank or full.
  enqueue(id: Uuid, text: string): boolean;
  /// `sendToPane` without focusing or switching workspace.
  /// `notAfterMs` (epoch ms): Enter is not pressed after it.
  send(
    id: Uuid,
    text: string,
    notAfterMs?: number,
  ): Promise<"sent" | "pasted" | "waiting" | "no-paste" | "gone" | "failed">;
}

export interface PeekReply {
  lines: string[];
  menu: { n: number; label: string; selected: boolean; input: boolean }[];
  /// One appearance of one menu; "" when there is none.
  menu_id: string;
  question: string[];
  blocked: string | null;
  status: RemotePane["status"];
}

export const DEFAULT_PEEK_LINES = 80;
export const MAX_PEEK_LINES = 200;

export function list(host: RemoteHost): { panes: RemotePane[] } {
  return { panes: buildPaneList(host.listInput()) };
}

/// What an id is bound to, read now: the menu block, the pane's input
/// generation and its agent's status generation. `null` when any of them
/// cannot be read.
function gensNow(host: RemoteHost, id: Uuid, rows: readonly string[]): Gens | null {
  const input = host.inputGen(id);
  const key = host.agentKey(id);
  if (input === null || key === null) return null;
  host.menus.noteAgent(id, key);
  return {
    fp: menuFingerprint(menuOptions(rows), menuBlock(rows)),
    input,
    status: host.menus.statusGen(id),
  };
}

export function peek(host: RemoteHost, id: Uuid, lines: number | undefined): PeekReply {
  const status = buildPaneList(host.listInput()).find((p) => p.pane === id)?.status ?? "none";
  const term = host.terminal(id);
  if (!term) return { lines: [], menu: [], menu_id: "", question: [], blocked: null, status };
  const want = Math.min(
    MAX_PEEK_LINES,
    Math.max(1, Number.isFinite(lines) ? Math.floor(lines as number) : DEFAULT_PEEK_LINES),
  );
  // Menus are found on terminal rows (that is what the detector reads); the
  // lines sent out are logical: soft-wrapped rows are joined (nothing more),
  // so redaction sees a soft-wrapped secret whole.
  const rows = term.screenText(MAX_PEEK_LINES);
  const menu = menuOptions(rows);
  const question = menuQuestion(rows);
  // No generations to bind an id to: show the menu but hand out no id.
  const gens = gensNow(host, id, rows);
  return {
    lines: term.screenLogicalText(MAX_PEEK_LINES).slice(-want),
    menu: menu.map((o) => ({
      n: Number(o.n),
      label: o.label,
      selected: o.selected,
      input: o.input,
    })),
    menu_id: gens ? host.menus.issue(id, gens) : "",
    question,
    blocked: term.screenBlock(),
    status,
  };
}

/// Type the option number (no Enter, like the office peek's buttons) when
/// `menuId` is the pane's current menu id, all three things it was minted for
/// (the menu block, the pane's input generation, the agent's status
/// generation) are unchanged, AND that very option is still on screen. The id
/// is retired BEFORE anything is typed. Anything in doubt is "stale": a pane
/// whose terminal is not up, no menu found, generations that cannot be read.
/// Options numbered above 9 (typing "10" presses "1" first) and options that
/// open a text field are refused.
export function answer(host: RemoteHost, id: Uuid, n: number, label: string, menuId: string): AnswerResult {
  const term = host.terminal(id);
  if (!term) {
    // Known to the app but its terminal is not up (workspace never opened):
    // the phone's screen is of something this cannot check. Unknown: gone.
    return buildPaneList(host.listInput()).some((p) => p.pane === id) ? "stale" : "gone";
  }
  if (!Number.isInteger(n) || n < 1 || n > 9 || typeof label !== "string") return "failed";
  if (typeof menuId !== "string" || menuId === "") return "stale";
  const rows = term.screenText(MAX_PEEK_LINES);
  const options = menuOptions(rows);
  if (options.length === 0) return "stale";
  const gens = gensNow(host, id, rows);
  if (!gens || !host.menus.holds(id, menuId, gens)) return "stale";
  const o = options.find((x) => x.n === String(n) && labelMatches(x.label, label));
  if (!o) return "stale";
  // Re-check through the same function the office peek uses, on a fresh read.
  if (!sameOption(term.screenText(MAX_PEEK_LINES), o)) return "stale";
  if (o.input) return "failed";
  // One answer per appearance of a menu: retired before the key goes out.
  host.menus.retire(id);
  term.typeText(o.n);
  return "sent";
}

/// One Escape, and only to a pane that has an agent: a plain shell is `gone`.
export function interrupt(host: RemoteHost, id: Uuid): "sent" | "gone" {
  const term = host.terminal(id);
  if (!term) return "gone";
  if (host.phase(id) === "none") return "gone";
  term.typeText("\x1b");
  return "sent";
}

/// Deliver a prompt like a user's paste + Enter, only into an agent that is
/// ready. A pane with no agent is `gone` (never a shell: a paste there would
/// run as a command). A permission prompt or dialog is `blocked`. A working
/// agent (or a non-empty queue, which keeps its order) takes the text into the
/// prompt queue when asked, else `waiting`.
export async function prompt(
  host: RemoteHost,
  id: Uuid,
  text: string,
  queue: boolean,
  /// Epoch ms at which the backend answers `timeout` and ignores any later
  /// reply (`deadline_ms` of the request). Nothing is submitted after it.
  deadlineMs?: number,
  now: () => number = Date.now,
): Promise<PromptResult> {
  if (typeof text !== "string" || text.trim() === "" || text.length > MAX_PROMPT_CHARS) return "failed";
  if (deadlineMs !== undefined && now() >= deadlineMs) return "failed";
  const term = host.terminal(id);
  if (!term) return "gone";
  const phase = host.phase(id);
  if (phase === "none") return "gone";
  if (phase === "waiting" || term.screenBlock() !== null) return "blocked";
  if (phase === "working" || host.queued(id) > 0) {
    if (!queue) return "waiting";
    return host.enqueue(id, text) ? "queued" : "failed";
  }
  return mapSendResult(await host.send(id, text, deadlineMs));
}

const FAILURE: Record<RemoteRequest["op"], unknown> = {
  list: { panes: [] },
  peek: { lines: [], menu: [], menu_id: "", question: [], blocked: null, status: "none" },
  answer: { result: "failed" },
  interrupt: { result: "failed" },
  prompt: { result: "failed" },
};

/// The reply data for `req`. Never rejects.
export async function handleRequest(host: RemoteHost, req: RemoteRequest): Promise<unknown> {
  try {
    const id = req.pane ?? "";
    switch (req.op) {
      case "list":
        return list(host);
      case "peek":
        return peek(host, id, req.lines);
      case "answer":
        return { result: answer(host, id, req.n ?? 0, req.label ?? "", req.menu_id ?? "") };
      case "interrupt":
        return { result: interrupt(host, id) };
      case "prompt":
        return {
          result: await prompt(
            host,
            id,
            req.text ?? "",
            req.queue === true,
            typeof req.deadline_ms === "number" ? req.deadline_ms : undefined,
          ),
        };
      default:
        return { result: "failed" };
    }
  } catch (e) {
    console.warn("remote request failed:", req.op, e);
    return FAILURE[req.op] ?? { result: "failed" };
  }
}
