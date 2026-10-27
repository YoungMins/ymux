// Pure model behind the office view: the workspace's split layout becomes a
// floor plan (one room per pane or tab group, one desk per tab) and every
// agent in it becomes a character with a place to be. No DOM, no IPC:
// `OfficeView` renders whatever `buildOffice` returns and animates the
// difference between two of them.

import type { AgentSnapshot, AgentStatus, LayoutNode, PaneKind, PaneSpec, Uuid } from "../types";
import type { PaneStatus } from "../terminal/paneStatus";
import { corridorGraph, nearestNode, route, simplify, type CorridorGraph } from "./officePaths";
import type { Mail } from "./officeLog";

/// A point or box in floor coordinates: fractions of the office floor,
/// (0, 0) top-left, (1, 1) bottom-right.
export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/// Share of the floor's height kept free at the bottom for the lobby, where
/// the user's own character waits between errands.
export const LOBBY_H = 0.14;

/// Where the user's character stands in the lobby.
export const MANAGER_HOME: Point = { x: 0.5, y: 1 - LOBBY_H / 2 + 0.02 };

export interface DeskModel {
  paneId: Uuid;
  label: string;
  /// The tab on screen in a tab group (always true for a single pane).
  active: boolean;
  status: PaneStatus;
  /// Desk centre, in floor coordinates.
  at: Point;
}

export interface RoomModel {
  /// The pane id, or the tab group's id.
  id: string;
  rect: Rect;
  kind: PaneKind;
  label: string;
  /// The most urgent status of its desks (attention > done > running > idle).
  status: PaneStatus;
  focused: boolean;
  desks: DeskModel[];
  /// The doorway, just inside the room at the bottom centre.
  door: Point;
  /// The room's split cell: the room plus half a corridor on every side.
  cell: Rect;
  /// Where the doorway meets the corridor's centre line.
  exit: Point;
  /// Letters in this room's mailbox, newest first, with the desk they are for.
  mail: { paneId: Uuid; mail: Mail }[];
  /// A desk whose agent finished and the user has not looked yet: the room
  /// flies a flag until they do.
  doneDesk: Uuid | null;
}

export type Pose = "typing" | "raising" | "relaxing" | "sleeping";

export interface CharModel {
  /// `${paneId}:lead` or `${paneId}:sub:${id}`.
  key: string;
  paneId: Uuid;
  roomId: string;
  role: "lead" | "sub";
  /// Lead: the agent CLI ("claude", "codex", …). Subagent: its lead's kind.
  kind: string;
  /// Lead: the CLI's display name. Subagent: its agent_type.
  label: string;
  status: AgentStatus;
  tool: string | null;
  pose: Pose;
  /// Where it belongs right now: at its desk (lead) or a helper spot (sub).
  home: Point;
}

export interface OfficeModel {
  rooms: RoomModel[];
  chars: CharModel[];
  /// The corridors between and around the rooms.
  graph: CorridorGraph;
  /// Where the lobby opens onto the corridors, straight up from the user.
  lobbyGate: Point;
}

export interface OfficeInput {
  root: LayoutNode;
  agents: AgentSnapshot;
  statusOf: (paneId: Uuid) => PaneStatus;
  labelOf: (paneId: Uuid) => string;
  focused: Uuid | null;
  /// Corridor width as a share of the floor's width (x) and height (y), so it
  /// is the same number of pixels both ways. None by default.
  corridor?: { x: number; y: number };
  /// A pane's mailbox (`officeLog`). Empty by default.
  mailOf?: (paneId: Uuid) => readonly Mail[];
}

const STATUS_RANK: Record<PaneStatus, number> = { idle: 0, running: 1, done: 2, attention: 3 };

export function worstStatus(statuses: readonly PaneStatus[]): PaneStatus {
  let worst: PaneStatus = "idle";
  for (const s of statuses) if (STATUS_RANK[s] > STATUS_RANK[worst]) worst = s;
  return worst;
}

export function poseOf(status: AgentStatus): Pose {
  switch (status) {
    case "working":
      return "typing";
    case "waiting":
      return "raising";
    case "done":
      return "relaxing";
    case "idle":
      return "sleeping";
  }
}

const KIND_NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini",
  opencode: "opencode",
  aider: "Aider",
  amp: "Amp",
  copilot: "Copilot",
  kimi: "Kimi",
  qwen: "Qwen",
  crush: "Crush",
};

export function kindName(kind: string): string {
  return KIND_NAMES[kind] ?? kind;
}

interface Leaf {
  id: string;
  rect: Rect;
  panes: PaneSpec[];
  active: number;
}

/// Every room of `root`, laid out in `area` with the same proportions the
/// split view uses. A split's `horizontal` direction is side by side (the
/// split view's flex row), `vertical` is stacked; `ratio` is `a`'s share.
export function layoutLeaves(root: LayoutNode, area: Rect): Leaf[] {
  const out: Leaf[] = [];
  const walk = (node: LayoutNode, r: Rect): void => {
    if (node.kind === "pane") {
      out.push({ id: node.id, rect: r, panes: [node], active: 0 });
      return;
    }
    if (node.kind === "tabs") {
      const panes: PaneSpec[] = [];
      for (const child of node.children) collectPanes(child, panes);
      if (panes.length > 0) {
        out.push({ id: node.id, rect: r, panes, active: Math.min(node.active, panes.length - 1) });
      }
      return;
    }
    const ratio = Math.min(0.95, Math.max(0.05, node.ratio));
    if (node.direction === "horizontal") {
      const wa = r.w * ratio;
      walk(node.a, { x: r.x, y: r.y, w: wa, h: r.h });
      walk(node.b, { x: r.x + wa, y: r.y, w: r.w - wa, h: r.h });
    } else {
      const ha = r.h * ratio;
      walk(node.a, { x: r.x, y: r.y, w: r.w, h: ha });
      walk(node.b, { x: r.x, y: r.y + ha, w: r.w, h: r.h - ha });
    }
  };
  walk(root, area);
  return out;
}

function collectPanes(node: LayoutNode, into: PaneSpec[]): void {
  if (node.kind === "pane") into.push(node);
  else if (node.kind === "tabs") for (const c of node.children) collectPanes(c, into);
  else {
    collectPanes(node.a, into);
    collectPanes(node.b, into);
  }
}

/// Room-relative point → floor point.
export function inRoom(room: Rect, fx: number, fy: number): Point {
  return { x: room.x + room.w * fx, y: room.y + room.h * fy };
}

/// Desk centres for `n` desks in a room: up to three per row, in the upper
/// middle of the room, leaving the bottom strip for helpers.
export function deskSpots(n: number): Point[] {
  if (n <= 0) return [];
  const cols = Math.min(n, 3);
  const rows = Math.ceil(n / cols);
  const out: Point[] = [];
  for (let i = 0; i < n; i++) {
    const col = i % cols;
    const row = Math.floor(i / cols);
    out.push({
      x: (col + 0.5) / cols,
      y: rows === 1 ? 0.42 : 0.28 + (row * 0.34) / Math.max(1, rows - 1),
    });
  }
  return out;
}

/// Where helper `i` of `n` works: a row in front of the desks, so a lead with
/// many subagents gets a busy front row.
export function helperSpot(i: number, n: number): Point {
  const perRow = Math.max(1, Math.min(n, 5));
  const row = Math.floor(i / perRow);
  const col = i % perRow;
  return { x: (col + 0.5) / perRow, y: Math.min(0.9, 0.7 + row * 0.08) };
}

/// Build the floor plan and its people.
export function buildOffice(input: OfficeInput): OfficeModel {
  const cx = input.corridor?.x ?? 0;
  const cy = input.corridor?.y ?? 0;
  // Cells tile this area; each edge of a cell is a corridor's centre line,
  // including the ring around the outside.
  const area: Rect = { x: cx / 2, y: cy / 2, w: 1 - cx, h: 1 - LOBBY_H - cy };
  const rooms: RoomModel[] = [];
  const chars: CharModel[] = [];
  for (const cell of layoutLeaves(input.root, area)) {
    const leaf = { ...cell, rect: inset(cell.rect, cx / 2, cy / 2) };
    const spots = deskSpots(leaf.panes.length);
    const desks: DeskModel[] = leaf.panes.map((p, i) => ({
      paneId: p.id,
      label: input.labelOf(p.id),
      active: i === leaf.active,
      status: input.statusOf(p.id),
      at: inRoom(leaf.rect, spots[i].x, spots[i].y),
    }));
    const lead = leaf.panes[leaf.active] ?? leaf.panes[0];
    const room: RoomModel = {
      id: leaf.id,
      rect: leaf.rect,
      kind: lead.pane_kind ?? "terminal",
      label: leaf.panes.length > 1 ? desks.map((d) => d.label).join(" · ") : desks[0].label,
      status: worstStatus(desks.map((d) => d.status)),
      focused: leaf.panes.some((p) => p.id === input.focused),
      desks,
      door: inRoom(leaf.rect, 0.5, 0.97),
      cell: cell.rect,
      exit: { x: cell.rect.x + cell.rect.w / 2, y: cell.rect.y + cell.rect.h },
      mail: desks
        .flatMap((d) => (input.mailOf?.(d.paneId) ?? []).map((mail) => ({ paneId: d.paneId, mail })))
        .sort((a, b) => b.mail.at - a.mail.at),
      doneDesk: desks.find((d) => d.status === "done")?.paneId ?? null,
    };
    rooms.push(room);

    // Helpers are numbered across the whole room so two tabs' subagents
    // don't stand on each other.
    const subs = desks.flatMap((d) =>
      (input.agents[d.paneId]?.subagents ?? []).map((s) => ({ desk: d, s })),
    );
    for (const desk of desks) {
      const a = input.agents[desk.paneId];
      if (!a?.lead) continue;
      chars.push({
        key: `${desk.paneId}:lead`,
        paneId: desk.paneId,
        roomId: room.id,
        role: "lead",
        kind: a.lead.kind,
        label: kindName(a.lead.kind),
        status: a.lead.status,
        tool: a.lead.tool,
        pose: poseOf(a.lead.status),
        // At the desk itself: the view seats it behind the desk, beside the
        // monitor, with the desk hiding its legs.
        home: desk.at,
      });
    }
    subs.forEach(({ desk, s }, i) => {
      const spot = helperSpot(i, subs.length);
      const leadKind = input.agents[desk.paneId]?.lead?.kind ?? "claude";
      chars.push({
        key: `${desk.paneId}:sub:${s.id}`,
        paneId: desk.paneId,
        roomId: room.id,
        role: "sub",
        kind: leadKind,
        label: s.agent_type || "subagent",
        status: s.status,
        tool: null,
        pose: poseOf(s.status),
        home: inRoom(leaf.rect, spot.x, spot.y),
      });
    });
  }
  const bottom = area.y + area.h;
  const lobbyGate = { x: MANAGER_HOME.x, y: bottom };
  const graph = corridorGraph(
    rooms.map((r) => r.cell),
    [...rooms.map((r) => r.exit), lobbyGate],
  );
  return { rooms, chars, graph, lobbyGate };
}

function inset(r: Rect, dx: number, dy: number): Rect {
  return {
    x: r.x + Math.min(dx, r.w / 4),
    y: r.y + Math.min(dy, r.h / 4),
    w: Math.max(0, r.w - 2 * Math.min(dx, r.w / 4)),
    h: Math.max(0, r.h - 2 * Math.min(dy, r.h / 4)),
  };
}

/// Where someone is: a point, and the room it is in (null: the lobby).
export interface Place {
  at: Point;
  room: string | null;
}

/// The points to walk through from `from` to `to` (the start excluded): out
/// of the room by its door, along the corridors, in by the other door. Within
/// one room it is a straight line.
export function planPath(model: OfficeModel, from: Place, to: Place): Point[] {
  if (from.room === to.room) return [to.at];
  const room = (id: string | null) => model.rooms.find((r) => r.id === id);
  const src = room(from.room);
  const dst = room(to.room);
  const path: Point[] = [];
  // Leave: to the door, then out to the corridor (or up from the lobby).
  if (src) path.push(src.door, src.exit);
  else path.push({ x: from.at.x, y: model.lobbyGate.y + (from.at.y - model.lobbyGate.y) * 0.4 }, model.lobbyGate);
  const start = nearestNode(model.graph, src ? src.exit : model.lobbyGate);
  const end = nearestNode(model.graph, dst ? dst.exit : model.lobbyGate);
  path.push(...route(model.graph, start, end));
  // Arrive.
  if (dst) path.push(dst.exit, dst.door, to.at);
  else path.push(model.lobbyGate, to.at);
  return simplify([from.at, ...path]).slice(1);
}

/// What happened to the characters between two models, for the view to
/// animate: who walked in, who finished their errand, who left.
export interface CharChanges {
  entered: CharModel[];
  /// Subagents that just reached `done`: they report to their lead.
  finished: CharModel[];
  /// Leads that just reached `done`: they celebrate.
  cheered: CharModel[];
  left: string[];
}

export function diffChars(prev: readonly CharModel[], next: readonly CharModel[]): CharChanges {
  const before = new Map(prev.map((c) => [c.key, c]));
  const after = new Set(next.map((c) => c.key));
  const entered: CharModel[] = [];
  const finished: CharModel[] = [];
  const cheered: CharModel[] = [];
  for (const c of next) {
    const old = before.get(c.key);
    if (!old) entered.push(c);
    else if (c.status === "done" && old.status !== "done") (c.role === "sub" ? finished : cheered).push(c);
  }
  const left = prev.filter((c) => !after.has(c.key)).map((c) => c.key);
  return { entered, finished, cheered, left };
}

/// Walk time between two floor points: a steady pace, clamped so a short
/// hop is still visible and a cross-office trip doesn't drag.
export function walkMs(a: Point, b: Point): number {
  const d = Math.hypot(a.x - b.x, a.y - b.y);
  return Math.round(Math.min(2400, Math.max(350, d * 2600)));
}

/// The spot next to `target` a visitor walks to: just beside it, on the side
/// facing the room's middle so it stays inside the room.
export function besideSpot(target: Point, room: Rect): Point {
  const mid = room.x + room.w / 2;
  const dx = Math.min(0.06, room.w * 0.18);
  return { x: target.x <= mid ? target.x + dx : target.x - dx, y: target.y + room.h * 0.02 };
}
