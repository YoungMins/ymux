import { describe, expect, it } from "vitest";
import { corridorGraph } from "./officePaths";
import { petErrand, petsFor, type PetLobby } from "./petModel";

const graph = corridorGraph([
  { x: 0, y: 0, w: 0.5, h: 0.86 },
  { x: 0.5, y: 0, w: 0.5, h: 0.86 },
]);
const lobby: PetLobby = { top: 0.9, bottom: 0.97, gate: { x: 0.5, y: 0.86 } };
const onCorridor = (p: { x: number; y: number }) =>
  [0, 0.5, 1].some((x) => Math.abs(p.x - x) < 1e-9) || [0, 0.86].some((y) => Math.abs(p.y - y) < 1e-9);
const seq = (...xs: number[]) => {
  let i = 0;
  return () => xs[i++ % xs.length];
};

describe("petErrand", () => {
  it("walks the corridors to a junction", () => {
    const e = petErrand(graph, lobby, { at: graph.nodes[0], lobby: false }, seq(0.9, 0.99, 0.5, 0.5));
    expect(e.end.lobby).toBe(false);
    for (const p of e.path) expect(onCorridor(p), JSON.stringify(p)).toBe(true);
    expect(e.path[e.path.length - 1]).toEqual(e.end.at);
  });

  it("goes down to the lobby through its gate and stays inside it", () => {
    const e = petErrand(graph, lobby, { at: graph.nodes[0], lobby: false }, seq(0.1, 0.1, 0.5, 0.9));
    expect(e.end.lobby).toBe(true);
    expect(e.path).toContainEqual(lobby.gate);
    expect(e.end.at.y).toBeGreaterThanOrEqual(lobby.top);
    expect(e.end.at.y).toBeLessThanOrEqual(lobby.bottom);
  });

  it("leaves the lobby by the gate", () => {
    const e = petErrand(graph, lobby, { at: { x: 0.2, y: 0.93 }, lobby: true }, seq(0.9, 0.2, 0.5));
    expect(e.path[0]).toEqual(lobby.gate);
    expect(e.end.lobby).toBe(false);
  });

  it("stays in the lobby when there are no corridors", () => {
    const e = petErrand({ nodes: [], edges: [] }, lobby, { at: { x: 0.5, y: 0.93 }, lobby: true }, seq(0.9, 0.3, 0.3, 0.9));
    expect(e.end.lobby).toBe(true);
    expect(e.path).toHaveLength(1);
  });

  it("rests after every errand", () => {
    for (let i = 0; i < 20; i++) {
      const e = petErrand(graph, lobby, { at: graph.nodes[1], lobby: false });
      expect(["sit", "sleep"]).toContain(e.rest);
      expect(e.restMs).toBeGreaterThan(1000);
    }
  });
});

describe("petsFor", () => {
  it("keeps a cat and a dog, and adds more with the rooms", () => {
    expect(petsFor(1)).toEqual(["cat", "dog"]);
    expect(petsFor(3)).toEqual(["cat", "dog", "rabbit"]);
    expect(petsFor(20)).toEqual(["cat", "dog", "rabbit", "chick"]);
  });
});
