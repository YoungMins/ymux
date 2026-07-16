// Client-side mirror of the Rust `LayoutNode` tree operations. Keeping the
// logic on both sides lets the frontend mutate layouts responsively without a
// round-trip, then persist the resulting tree via `save_config`.
//
// The functions here operate on immutable-ish clones so React-style rendering
// can diff easily, but we do return the same reference when a subtree is
// unchanged for cheap equality checks.

import type { HotKeyDef, LayoutNode, PaneSpec, Uuid } from "../types";
import type { SplitDir } from "../types";
import { uuidv4 } from "../types";

export function newPane(shell: string, cwd: string | null = null): PaneSpec {
  return {
    id: uuidv4(),
    title: null,
    shell,
    cwd,
    startup_cmd: null,
    env: [],
    pane_kind: "terminal",
    url: null,
    hotkeys: [],
  };
}

export function newBrowserPane(url: string): PaneSpec {
  return {
    id: uuidv4(),
    title: null,
    shell: "",
    cwd: null,
    startup_cmd: null,
    env: [],
    pane_kind: "native_browser",
    url,
    hotkeys: [],
  };
}

export function paneNode(spec: PaneSpec): LayoutNode {
  return {
    kind: "pane",
    id: spec.id,
    title: spec.title,
    shell: spec.shell,
    cwd: spec.cwd,
    startup_cmd: spec.startup_cmd,
    env: spec.env,
    pane_kind: spec.pane_kind ?? "terminal",
    url: spec.url ?? null,
    hotkeys: spec.hotkeys ?? [],
    bg_color: spec.bg_color ?? "",
    worktree_path: spec.worktree_path ?? "",
  };
}

export function nodeToSpec(node: LayoutNode & { kind: "pane" }): PaneSpec {
  return {
    id: node.id,
    title: node.title,
    shell: node.shell,
    cwd: node.cwd,
    startup_cmd: node.startup_cmd,
    env: node.env,
    pane_kind: node.pane_kind ?? "terminal",
    url: node.url ?? null,
    hotkeys: (node.hotkeys ?? []) as HotKeyDef[],
    bg_color: node.bg_color ?? "",
    worktree_path: node.worktree_path ?? "",
  };
}

/// Replace the node matching `targetId` with a split containing the original
/// pane on the `a` side and a freshly-created pane on the `b` side.
export function splitPane(
  root: LayoutNode,
  targetId: Uuid,
  direction: SplitDir,
  newPaneSpec: PaneSpec,
): LayoutNode {
  if (root.kind === "pane") {
    if (root.id !== targetId) return root;
    return {
      kind: "split",
      direction,
      ratio: 0.5,
      a: root,
      b: paneNode(newPaneSpec),
    };
  }
  if (root.kind === "split") {
    const a = splitPane(root.a, targetId, direction, newPaneSpec);
    const b = a === root.a ? splitPane(root.b, targetId, direction, newPaneSpec) : root.b;
    if (a === root.a && b === root.b) return root;
    return { ...root, a, b };
  }
  if (root.kind === "tabs") {
    const children = root.children.map((c) =>
      splitPane(c, targetId, direction, newPaneSpec),
    );
    return { ...root, children };
  }
  return root;
}

/// Remove the pane with `paneId`. If the resulting split has only one child,
/// the split is collapsed to that child. Returns the new tree, or `null` if
/// the entire workspace becomes empty (caller should create a replacement pane).
export function removePane(root: LayoutNode, paneId: Uuid): LayoutNode | null {
  if (root.kind === "pane") {
    return root.id === paneId ? null : root;
  }
  if (root.kind === "split") {
    const a = removePane(root.a, paneId);
    if (a === null) return root.b;
    const b = removePane(root.b, paneId);
    if (b === null) return root.a;
    if (a === root.a && b === root.b) return root;
    return { ...root, a, b };
  }
  if (root.kind === "tabs") {
    const children: LayoutNode[] = [];
    for (const c of root.children) {
      const next = removePane(c, paneId);
      if (next !== null) children.push(next);
    }
    if (children.length === 0) return null;
    const active = Math.min(root.active, children.length - 1);
    return { ...root, active, children };
  }
  return root;
}

/// Update the ratio of the split that directly contains `paneId` as one of
/// its immediate children. Returns the new tree, leaving other subtrees untouched.
export function setSplitRatioContaining(
  root: LayoutNode,
  paneId: Uuid,
  ratio: number,
): LayoutNode {
  if (root.kind === "split") {
    const aIsTarget = root.a.kind === "pane" && root.a.id === paneId;
    const bIsTarget = root.b.kind === "pane" && root.b.id === paneId;
    if (aIsTarget || bIsTarget) {
      return { ...root, ratio: clamp(ratio, 0.05, 0.95) };
    }
    return {
      ...root,
      a: setSplitRatioContaining(root.a, paneId, ratio),
      b: setSplitRatioContaining(root.b, paneId, ratio),
    };
  }
  if (root.kind === "tabs") {
    return {
      ...root,
      children: root.children.map((c) => setSplitRatioContaining(c, paneId, ratio)),
    };
  }
  return root;
}

/// Set the ratio of a specific split identified by the path from the root.
export function setRatioByPath(
  root: LayoutNode,
  path: number[],
  ratio: number,
): LayoutNode {
  if (path.length === 0) {
    if (root.kind !== "split") return root;
    return { ...root, ratio: clamp(ratio, 0.05, 0.95) };
  }
  if (root.kind === "split") {
    const [head, ...rest] = path;
    if (head === 0) {
      return { ...root, a: setRatioByPath(root.a, rest, ratio) };
    }
    return { ...root, b: setRatioByPath(root.b, rest, ratio) };
  }
  return root;
}

/// Swap the positions of two leaf panes in the tree. Pane ids are preserved,
/// so the caller's pane cache, DOM elements, and PTYs stay put — only their
/// slots in the layout change, keeping terminal scrollback intact. Returns the
/// same tree unchanged if `idA === idB` or either id is missing.
export function swapPanes(root: LayoutNode, idA: Uuid, idB: Uuid): LayoutNode {
  if (idA === idB) return root;
  const nodeA = findNode(root, idA);
  const nodeB = findNode(root, idB);
  if (!nodeA || !nodeB) return root;
  return swapLeaves(root, idA, nodeA, idB, nodeB);
}

function findNode(
  root: LayoutNode,
  id: Uuid,
): (LayoutNode & { kind: "pane" }) | null {
  let found: (LayoutNode & { kind: "pane" }) | null = null;
  walk(root, (n) => {
    if (!found && n.kind === "pane" && n.id === id) found = n;
  });
  return found;
}

/// Rebuild the tree placing `nodeB` where `idA` was and `nodeA` where `idB` was.
function swapLeaves(
  node: LayoutNode,
  idA: Uuid,
  nodeA: LayoutNode,
  idB: Uuid,
  nodeB: LayoutNode,
): LayoutNode {
  if (node.kind === "pane") {
    if (node.id === idA) return nodeB;
    if (node.id === idB) return nodeA;
    return node;
  }
  if (node.kind === "split") {
    return {
      ...node,
      a: swapLeaves(node.a, idA, nodeA, idB, nodeB),
      b: swapLeaves(node.b, idA, nodeA, idB, nodeB),
    };
  }
  if (node.kind === "tabs") {
    return {
      ...node,
      children: node.children.map((c) =>
        swapLeaves(c, idA, nodeA, idB, nodeB),
      ),
    };
  }
  return node;
}

/// Depth-first list of pane ids for focus cycling and persistence.
export function panes(root: LayoutNode): PaneSpec[] {
  const out: PaneSpec[] = [];
  walk(root, (node) => {
    if (node.kind === "pane") out.push(nodeToSpec(node));
  });
  return out;
}

export function findPane(root: LayoutNode, id: Uuid): PaneSpec | null {
  let found: PaneSpec | null = null;
  walk(root, (node) => {
    if (found) return;
    if (node.kind === "pane" && node.id === id) found = nodeToSpec(node);
  });
  return found;
}

function walk(node: LayoutNode, visit: (n: LayoutNode) => void): void {
  visit(node);
  if (node.kind === "split") {
    walk(node.a, visit);
    walk(node.b, visit);
  } else if (node.kind === "tabs") {
    for (const c of node.children) walk(c, visit);
  }
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}
