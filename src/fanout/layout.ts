import type { LayoutNode, PaneSpec } from "../types";
import { uuidv4 } from "../types";
import { newPane, paneNode, splitPane } from "../layout/LayoutTree";

const split = (direction: "horizontal" | "vertical", a: LayoutNode, b: LayoutNode): LayoutNode => ({
  kind: "split",
  direction,
  ratio: 0.5,
  a,
  b,
});

/// Fan-out layout: 1 pane, 2 side by side, 3-4 a 2x2 grid (3 = two on top,
/// one below), 5+ (or `forceTabs`) one tab group. "horizontal" = side by side.
export function fanoutLayout(specs: PaneSpec[], forceTabs = false): LayoutNode {
  const n = specs.length;
  if (n === 0) throw new Error("fanoutLayout: no panes");
  const p = specs.map(paneNode);
  if (forceTabs || n >= 5) return { kind: "tabs", id: uuidv4(), active: 0, children: p };
  if (n === 1) return p[0];
  if (n === 2) return split("horizontal", p[0], p[1]);
  const top = split("horizontal", p[0], p[1]);
  return split("vertical", top, n === 3 ? p[2] : split("horizontal", p[2], p[3]));
}

/// Put `node` beside pane/group `targetId` (side by side) in `root`, reusing
/// `splitPane`'s handling of tab groups. Returns `root` unchanged when
/// `targetId` is not in it.
export function attachNode(root: LayoutNode, targetId: string, node: LayoutNode): LayoutNode {
  const marker = newPane("", null);
  const split_ = splitPane(root, targetId, "horizontal", marker);
  const swap = (n: LayoutNode): LayoutNode => {
    if (n.kind === "pane") return n.id === marker.id ? node : n;
    if (n.kind === "split") return { ...n, a: swap(n.a), b: swap(n.b) };
    return { ...n, children: n.children.map(swap) };
  };
  return split_ === root ? root : swap(split_);
}
