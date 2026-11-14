// Remote server: one id per appearance of one menu on one pane (PROTOCOL.md
// 5.2 `menu_id`). A phone's `answer` is only typed when it carries the pane's
// current id AND three things still have the values the id was minted for:
//
//   (a) the fingerprint: the options plus every row above them back to the
//       first run of two blank rows (at most 60 rows), as the menu detector
//       sees them, so two questions that end alike but differ above (another
//       command) do not share an id;
//   (b) the pane's input generation (`inputGen.ts`): advances on every write
//       to that pane's terminal from any source, and on every automatic
//       answer, so anything typed since makes the id stale;
//   (c) the agent's status generation: advances on every change of the lead's
//       RAW status, tool or kind in the agent snapshot (not the attention-
//       coerced status the pane list shows, which does not move for a pane
//       nobody is watching).
//
// `peek` mints and `answer` compares; an id is retired the moment it is used.
// Pure (the random source is injectable): `remoteOps.ts` / `remoteWiring.ts`
// and `WorkspaceManager` call it.

import { menuOptions } from "../office/peekKeys";
import type { Agent, AgentSnapshot } from "../types";

/// 22 chars of `[A-Za-z0-9_-]`, from the OS random source. Fits the
/// protocol's 32-char bound.
export function randomMenuId(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/// Most rows of a menu block.
export const BLOCK_ROWS = 60;
/// Rows from the bottom of the screen the detector looks at.
const DETECT_ROWS = 30;

const OPTION = /^[│┃║|\s]*([❯›>→▶])?\s*(\d{1,2})[.)]\s+(\S.*?)[│┃║|\s]*$/;

/// A screen row without the box it may be drawn in and without the menu's
/// pointer (which moves when the user arrows through the options).
function plain(row: string): string {
  return row
    .replace(/^[\s│┃║|]+/, "")
    .replace(/^[❯›>→▶]\s*/, "")
    .replace(/[\s│┃║|]+$/, "");
}

function blank(row: string): boolean {
  return plain(row).replace(/[╭╮╰╯─━═┌┐└┘]/g, "").trim() === "";
}

/// The menu block of `rows` (what the menu detector reads): from the first
/// option down to the last option, plus every row above the first option back
/// to the first run of two blank rows, at most [`BLOCK_ROWS`] rows in all.
/// Empty when there is no menu. Rows lose their box and pointer, so moving the
/// pointer does not change it.
export function menuBlock(rows: readonly string[]): string[] {
  const opts = menuOptions(rows);
  if (opts.length === 0) return [];
  // Indices are into `rows`; the options themselves are found in the rows
  // the detector reads (its last DETECT_ROWS), the block reaches further up.
  const base = Math.max(0, rows.length - DETECT_ROWS);
  const isOption = (l: string, o: { n: string; label: string }): boolean => {
    const m = OPTION.exec(l);
    return m !== null && m[2] === o.n && l.includes(o.label);
  };
  let first = -1;
  for (let i = base; i < rows.length; i++) {
    if (isOption(rows[i], opts[0])) {
      first = i;
      break;
    }
  }
  if (first < 0) return [];
  const last = opts[opts.length - 1];
  let end = first;
  for (let i = rows.length - 1; i >= first; i--) {
    if (isOption(rows[i], last)) {
      end = i;
      break;
    }
  }
  // Up from the first option until two blank rows in a row (kept out of the
  // block), the top of the screen, or the row budget.
  let start = first;
  const floor = Math.max(0, end + 1 - BLOCK_ROWS);
  while (start > floor) {
    if (blank(rows[start - 1]) && start - 2 >= 0 && blank(rows[start - 2])) break;
    start--;
  }
  return rows.slice(start, end + 1).map(plain);
}

/// What makes two menus "the same one": their options and the whole block
/// around them. "" means there is no menu.
export function menuFingerprint(
  options: readonly { n: number | string; label: string }[],
  block: readonly string[],
): string {
  if (options.length === 0) return "";
  return JSON.stringify([options.map((o) => [String(o.n), o.label]), block]);
}

/// The agent's raw state for the status generation: its kind, status and
/// current tool, straight from the snapshot. "none" when no agent runs there.
export function agentKey(lead: Agent | null | undefined): string {
  return lead ? JSON.stringify([lead.kind, lead.status, lead.tool ?? ""]) : "none";
}

/// The three things an id is bound to.
export interface Gens {
  fp: string;
  input: number;
  status: number;
}

interface Live {
  id: string;
  at: Gens;
}

export class MenuIds {
  private live = new Map<string, Live>();
  /// The menu an answer was just typed into: it stays on screen for a moment,
  /// and must not be handed a fresh id until it is gone or the agent moved on.
  private retired = new Map<string, { fp: string; status: number }>();
  /// Per pane: the last raw agent key seen and how many times it changed.
  private agents = new Map<string, { key: string; gen: number }>();

  constructor(private readonly mint: () => string = randomMenuId) {}

  /// Note the pane's raw agent state; a change advances its status generation.
  noteAgent(pane: string, key: string): void {
    const cur = this.agents.get(pane);
    if (!cur) {
      this.agents.set(pane, { key, gen: 1 });
      return;
    }
    if (cur.key !== key) {
      cur.key = key;
      cur.gen += 1;
    }
  }

  /// Every pane's raw agent state from a fresh agent snapshot (panes that have
  /// no agent any more count as "none").
  observeAgents(snapshot: AgentSnapshot): void {
    const seen = new Set<string>();
    for (const [pane, a] of Object.entries(snapshot)) {
      seen.add(pane);
      this.noteAgent(pane, agentKey(a?.lead));
    }
    for (const pane of [...this.agents.keys()]) {
      if (!seen.has(pane)) this.noteAgent(pane, "none");
    }
  }

  /// The pane's status generation (0 if never noted).
  statusGen(pane: string): number {
    return this.agents.get(pane)?.gen ?? 0;
  }

  /// `peek`: the pane's id for the menu read now, minting a new one whenever
  /// the fingerprint, the input generation or the status generation differs
  /// from the values the current id was minted for. "" when there is no menu
  /// (or it is the one just answered).
  issue(pane: string, at: Gens): string {
    if (at.fp === "") {
      this.live.delete(pane);
      this.retired.delete(pane);
      return "";
    }
    const r = this.retired.get(pane);
    if (r) {
      if (r.fp === at.fp && r.status === at.status) return "";
      this.retired.delete(pane);
    }
    const cur = this.live.get(pane);
    if (
      cur &&
      cur.at.fp === at.fp &&
      cur.at.input === at.input &&
      cur.at.status === at.status
    ) {
      return cur.id;
    }
    const id = this.mint();
    this.live.set(pane, { id, at: { ...at } });
    return id;
  }

  /// `answer`: is `id` the pane's current id, minted for exactly these
  /// values? A pure comparison: it mints nothing and changes nothing.
  holds(pane: string, id: string, at: Gens): boolean {
    const cur = this.live.get(pane);
    return (
      cur !== undefined &&
      id !== "" &&
      cur.id === id &&
      cur.at.fp === at.fp &&
      cur.at.input === at.input &&
      cur.at.status === at.status
    );
  }

  /// The id the pane has right now, without reading its screen ("" if none).
  current(pane: string): string {
    return this.live.get(pane)?.id ?? "";
  }

  /// An answer is about to be typed: its id can never be used again.
  retire(pane: string): void {
    const cur = this.live.get(pane);
    if (!cur) return;
    this.retired.set(pane, { fp: cur.at.fp, status: cur.at.status });
    this.live.delete(pane);
  }

  /// Panes that are gone.
  keepOnly(panes: Iterable<string>): void {
    const keep = new Set(panes);
    for (const id of [...this.live.keys()]) if (!keep.has(id)) this.live.delete(id);
    for (const id of [...this.retired.keys()]) if (!keep.has(id)) this.retired.delete(id);
    for (const id of [...this.agents.keys()]) if (!keep.has(id)) this.agents.delete(id);
  }
}
