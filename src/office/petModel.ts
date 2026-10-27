// Where the office pets go next. A pet lives on the corridors and in the
// lobby, never inside a room: from where it stands it either trots to a
// corridor junction, wanders down into the lobby (or back up), or potters
// about the lobby, then sits or naps for a while. Pure: the view walks the
// path it gets back and calls again when the rest is over.

import type { Point } from "./officeModel";
import { nearestNode, route, simplify, type CorridorGraph } from "./officePaths";
import { PET_SPECIES, type PetSpecies } from "./petArt";

export interface PetSpot {
  at: Point;
  /// In the lobby rather than on a corridor.
  lobby: boolean;
}

export interface PetErrand {
  /// Points to walk through (the start excluded); empty to stay put.
  path: Point[];
  end: PetSpot;
  rest: "sit" | "sleep";
  restMs: number;
}

/// The lobby strip pets walk in: its top and bottom, and the way up to the
/// corridors.
export interface PetLobby {
  top: number;
  bottom: number;
  gate: Point;
}

const pick = <T>(xs: readonly T[], rnd: () => number): T => xs[Math.min(xs.length - 1, Math.floor(rnd() * xs.length))];

function lobbySpot(lobby: PetLobby, rnd: () => number): Point {
  return { x: 0.06 + rnd() * 0.88, y: lobby.top + rnd() * (lobby.bottom - lobby.top) };
}

export function petErrand(graph: CorridorGraph, lobby: PetLobby, from: PetSpot, rnd: () => number = Math.random): PetErrand {
  const roll = rnd();
  const rest = (): Pick<PetErrand, "rest" | "restMs"> =>
    rnd() < 0.3 ? { rest: "sleep", restMs: 6000 + rnd() * 9000 } : { rest: "sit", restMs: 1500 + rnd() * 4500 };
  const corridors = graph.nodes.length > 0;
  // Into (or around) the lobby.
  if (!corridors || roll < 0.35) {
    const to = lobbySpot(lobby, rnd);
    if (from.lobby) return { path: [to], end: { at: to, lobby: true }, ...rest() };
    const up = route(graph, nearestNode(graph, from.at), nearestNode(graph, lobby.gate));
    return { path: simplify([from.at, ...up, lobby.gate, to]).slice(1), end: { at: to, lobby: true }, ...rest() };
  }
  // To a corridor junction, by the corridors.
  const target = pick(graph.nodes.map((_, i) => i), rnd);
  const start = from.lobby ? nearestNode(graph, lobby.gate) : nearestNode(graph, from.at);
  const lead = from.lobby ? [lobby.gate] : [];
  const path = simplify([from.at, ...lead, ...route(graph, start, target)]).slice(1);
  return { path, end: { at: graph.nodes[target], lobby: false }, ...rest() };
}

/// Which pets live in the office: always the cat and the dog, then one more
/// for every three rooms, up to all four.
export function petsFor(rooms: number): PetSpecies[] {
  return PET_SPECIES.slice(0, Math.min(PET_SPECIES.length, 2 + Math.floor(rooms / 3)));
}
