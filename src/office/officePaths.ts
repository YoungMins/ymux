// Corridors: the office's rooms are its split cells shrunk by half a
// corridor on every side, so the cell edges are exactly the corridors'
// centre lines. Those edges form a graph (corners and T-junctions are its
// nodes); characters walk it from door to door instead of through walls.
// Pure: points and rects in floor coordinates.

import type { Point, Rect } from "./officeModel";

export interface Segment {
  a: Point;
  b: Point;
}

export interface CorridorGraph {
  nodes: Point[];
  /// Adjacency: node index → [neighbour index, length].
  edges: [number, number][][];
}

const EPS = 1e-6;

function key(p: Point): string {
  return `${Math.round(p.x / EPS)}:${Math.round(p.y / EPS)}`;
}

/// Every corridor piece between two nodes, axis-aligned.
export function segmentsOf(g: CorridorGraph): Segment[] {
  const out: Segment[] = [];
  g.edges.forEach((list, i) => {
    for (const [j] of list) if (j > i) out.push({ a: g.nodes[i], b: g.nodes[j] });
  });
  return out;
}

/// The corridor network of `cells` (each room's split cell). `extra` points
/// that lie on a cell edge (doorways, the lobby's way in) become nodes too,
/// splitting the edge they sit on.
export function corridorGraph(cells: readonly Rect[], extra: readonly Point[] = []): CorridorGraph {
  const nodes: Point[] = [];
  const index = new Map<string, number>();
  const node = (p: Point): number => {
    const k = key(p);
    let i = index.get(k);
    if (i === undefined) {
      i = nodes.length;
      nodes.push(p);
      index.set(k, i);
    }
    return i;
  };
  const points: Point[] = [...extra];
  const lines: Segment[] = [];
  for (const c of cells) {
    const tl = { x: c.x, y: c.y };
    const tr = { x: c.x + c.w, y: c.y };
    const bl = { x: c.x, y: c.y + c.h };
    const br = { x: c.x + c.w, y: c.y + c.h };
    points.push(tl, tr, bl, br);
    lines.push({ a: tl, b: tr }, { a: bl, b: br }, { a: tl, b: bl }, { a: tr, b: br });
  }
  for (const p of points) node(p);
  const edges: [number, number][][] = nodes.map(() => []);
  const linked = new Set<string>();
  for (const line of lines) {
    const horizontal = Math.abs(line.a.y - line.b.y) < EPS;
    const lo = horizontal ? Math.min(line.a.x, line.b.x) : Math.min(line.a.y, line.b.y);
    const hi = horizontal ? Math.max(line.a.x, line.b.x) : Math.max(line.a.y, line.b.y);
    const fixed = horizontal ? line.a.y : line.a.x;
    // Every node on this line, in order along it.
    const on = nodes
      .map((p, i) => ({ p, i }))
      .filter(({ p }) => {
        const along = horizontal ? p.x : p.y;
        const across = horizontal ? p.y : p.x;
        return Math.abs(across - fixed) < EPS && along >= lo - EPS && along <= hi + EPS;
      })
      .sort((u, v) => (horizontal ? u.p.x - v.p.x : u.p.y - v.p.y));
    for (let k = 1; k < on.length; k++) {
      const i = on[k - 1].i;
      const j = on[k].i;
      if (i === j) continue;
      const id = i < j ? `${i}-${j}` : `${j}-${i}`;
      if (linked.has(id)) continue;
      linked.add(id);
      const len = Math.hypot(nodes[i].x - nodes[j].x, nodes[i].y - nodes[j].y);
      edges[i].push([j, len]);
      edges[j].push([i, len]);
    }
  }
  return { nodes, edges };
}

/// Index of the node at `p`, or the nearest one.
export function nearestNode(g: CorridorGraph, p: Point): number {
  let best = -1;
  let bestD = Infinity;
  g.nodes.forEach((n, i) => {
    const d = Math.hypot(n.x - p.x, n.y - p.y);
    if (d < bestD) {
      bestD = d;
      best = i;
    }
  });
  return best;
}

/// The shortest corridor walk from node `from` to node `to`, as the points to
/// pass through (both ends included). Just the two ends when they are not
/// connected.
export function route(g: CorridorGraph, from: number, to: number): Point[] {
  if (from < 0 || to < 0) return [];
  if (from === to) return [g.nodes[from]];
  const dist = g.nodes.map(() => Infinity);
  const prev = g.nodes.map(() => -1);
  const done = g.nodes.map(() => false);
  dist[from] = 0;
  for (;;) {
    let u = -1;
    for (let i = 0; i < dist.length; i++) if (!done[i] && (u < 0 || dist[i] < dist[u])) u = i;
    if (u < 0 || dist[u] === Infinity || u === to) break;
    done[u] = true;
    for (const [v, w] of g.edges[u]) {
      if (dist[u] + w < dist[v]) {
        dist[v] = dist[u] + w;
        prev[v] = u;
      }
    }
  }
  if (dist[to] === Infinity) return [g.nodes[from], g.nodes[to]];
  const path: Point[] = [];
  for (let v = to; v !== -1; v = prev[v]) path.unshift(g.nodes[v]);
  return path;
}

/// Drop consecutive points that repeat or keep going straight, so a walk is
/// one segment per turn.
export function simplify(path: readonly Point[]): Point[] {
  const out: Point[] = [];
  for (const p of path) {
    const last = out[out.length - 1];
    if (last && Math.abs(last.x - p.x) < EPS && Math.abs(last.y - p.y) < EPS) continue;
    if (out.length >= 2) {
      const a = out[out.length - 2];
      const b = last;
      const cross = (b.x - a.x) * (p.y - b.y) - (b.y - a.y) * (p.x - b.x);
      if (Math.abs(cross) < EPS * EPS) {
        out[out.length - 1] = p;
        continue;
      }
    }
    out.push(p);
  }
  return out;
}
