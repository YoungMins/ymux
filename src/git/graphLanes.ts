// The git pane's commit graph, as pure data (spec §4.1).
//
// The retired ygit TUI drew `git log --graph`'s ASCII art and coloured it
// back by column. The pane computes the same picture from
// parent hashes instead: each row gets the lane its commit sits in and the
// line segments crossing the row's upper half (from the row above into the
// node) and lower half (from the node toward the parents below). The DOM
// draws those as SVG; nothing here knows about pixels.
//
// The visual spec kept from graph.rs is its palette model: colour is a
// function of the lane index, cycling through six colours.

/// graph.rs's six-colour lane palette (yellow, cyan, green, magenta, blue,
/// red); `laneColor` indexes it.
export const LANE_COLORS = 6;

export function laneColor(lane: number): number {
  return lane % LANE_COLORS;
}

/// A line within one half of a row, from lane `from` at the half's top to
/// lane `to` at its bottom. A straight pass-through has `from === to`.
export interface Edge {
  from: number;
  to: number;
  /// The colour of the lane that is not the node's own — the branch the
  /// line belongs to.
  color: number;
}

export interface LaneRow {
  /// The commit's lane.
  lane: number;
  color: number;
  /// Upper half: lines arriving from the row above.
  up: Edge[];
  /// Lower half: lines leaving toward the row below.
  down: Edge[];
  /// Lanes this row spans (the highest lane touched, plus one).
  width: number;
}

export interface LaneLayout {
  rows: LaneRow[];
  /// The widest row.
  width: number;
}

export interface GraphCommit {
  hash: string;
  parents: string[];
}

function firstFree(lanes: (string | null)[]): number {
  const i = lanes.indexOf(null);
  return i === -1 ? lanes.length : i;
}

/// Lay out `commits` — newest first, as `git log --date-order` returns them,
/// all loaded pages together — into lanes.
///
/// Each active lane holds the hash of the commit it is waiting for. A commit
/// takes the first lane waiting for it (any others waiting for it converge
/// into its node), or the first free lane when nothing is (a branch tip, or
/// history whose child is not loaded). Its first parent continues in its
/// lane; every further parent joins the lane already waiting for it or
/// opens a new one. A parent missing from `commits` (the next page) keeps
/// its lane open to the bottom, which is what the history actually does.
export function assignLanes(commits: readonly GraphCommit[]): LaneLayout {
  const lanes: (string | null)[] = [];
  const rows: LaneRow[] = [];
  let width = 0;

  for (const commit of commits) {
    let lane = lanes.indexOf(commit.hash);

    const up: Edge[] = [];
    lanes.forEach((waiting, k) => {
      if (waiting === null) return;
      up.push({ from: k, to: waiting === commit.hash ? lane : k, color: laneColor(k) });
    });

    if (lane === -1) lane = firstFree(lanes);
    // Every other lane that was waiting for this commit has arrived.
    for (let k = 0; k < lanes.length; k++) {
      if (k !== lane && lanes[k] === commit.hash) lanes[k] = null;
    }

    const [first, ...rest] = commit.parents;
    lanes[lane] = first ?? null;
    const opened = new Set<number>();
    const mergeInto: number[] = [];
    for (const p of rest) {
      let k = lanes.indexOf(p);
      if (k === -1) {
        k = firstFree(lanes);
        lanes[k] = p;
        opened.add(k);
      }
      mergeInto.push(k);
    }

    const down: Edge[] = [];
    lanes.forEach((waiting, k) => {
      if (waiting === null) return;
      // A lane opened by this merge starts at the node; everything else
      // (this commit's own first-parent line included) runs straight on.
      if (!opened.has(k)) down.push({ from: k, to: k, color: laneColor(k) });
    });
    for (const k of mergeInto) down.push({ from: lane, to: k, color: laneColor(k) });

    while (lanes.length > 0 && lanes[lanes.length - 1] === null) lanes.pop();

    let rowWidth = lane + 1;
    for (const e of up) rowWidth = Math.max(rowWidth, e.from + 1, e.to + 1);
    for (const e of down) rowWidth = Math.max(rowWidth, e.from + 1, e.to + 1);
    width = Math.max(width, rowWidth);
    rows.push({ lane, color: laneColor(lane), up, down, width: rowWidth });
  }

  return { rows, width };
}
