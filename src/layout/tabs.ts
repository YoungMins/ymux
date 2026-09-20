// Pure arithmetic for tabbed panes. A pane has tabs when it is wrapped in a
// `Tabs` node whose children are panes; nothing here touches the DOM, so all
// of it is vitest-testable. `SplitContainer` / `PaneGroup` render whatever
// these functions produce, and `WorkspaceManager` mutates the tree through
// them.

import type { LayoutNode, PaneSpec, Uuid } from "../types";
import { nodeToSpec, paneNode } from "./LayoutTree";

export type TabsNode = LayoutNode & { kind: "tabs" };
export type PaneNode = LayoutNode & { kind: "pane" };

export function tabsNode(id: Uuid, children: LayoutNode[], active = 0): TabsNode {
  return { kind: "tabs", id, active: clampIndex(active, children.length), children };
}

/// Replace the pane `paneId` with a one-tab group. Returns the same tree when
/// the pane is missing or already a direct child of a group, so callers can
/// call it unconditionally before `addTab`.
export function wrapInTabs(root: LayoutNode, paneId: Uuid, groupId: Uuid): LayoutNode {
  if (groupOfPane(root, paneId)) return root;
  return mapTree(root, (node) =>
    node.kind === "pane" && node.id === paneId ? tabsNode(groupId, [node], 0) : node,
  );
}

/// Append `spec` as the group's last tab and make it active.
export function addTab(root: LayoutNode, groupId: Uuid, spec: PaneSpec): LayoutNode {
  return mapTree(root, (node) => {
    if (node.kind !== "tabs" || node.id !== groupId) return node;
    const children = [...node.children, paneNode(spec)];
    return { ...node, active: children.length - 1, children };
  });
}

export function setActiveTab(
  root: LayoutNode,
  groupId: Uuid,
  index: number,
): LayoutNode {
  return mapTree(root, (node) =>
    node.kind === "tabs" && node.id === groupId
      ? { ...node, active: clampIndex(index, node.children.length) }
      : node,
  );
}

/// Make `paneId` the active tab of whichever group holds it. A pane that is
/// in no group leaves the tree untouched (same reference).
export function activateTabFor(root: LayoutNode, paneId: Uuid): LayoutNode {
  const group = groupOfPane(root, paneId);
  if (!group) return root;
  const index = tabIds(group).indexOf(paneId);
  if (index < 0 || index === group.active) return root;
  return setActiveTab(root, group.id, index);
}

/// Index of the previous / next tab, wrapping at both ends.
export function stepTab(group: TabsNode, delta: 1 | -1): number {
  const n = group.children.length;
  if (n === 0) return 0;
  return (group.active + delta + n) % n;
}

export function findGroup(root: LayoutNode, groupId: Uuid): TabsNode | null {
  let found: TabsNode | null = null;
  walk(root, (node) => {
    if (!found && node.kind === "tabs" && node.id === groupId) found = node;
  });
  return found;
}

/// The group that has `paneId` as a *direct* child, or null.
export function groupOfPane(root: LayoutNode, paneId: Uuid): TabsNode | null {
  let found: TabsNode | null = null;
  walk(root, (node) => {
    if (found || node.kind !== "tabs") return;
    if (node.children.some((c) => c.kind === "pane" && c.id === paneId)) found = node;
  });
  return found;
}

export function tabIds(group: TabsNode): Uuid[] {
  return group.children.filter(isPaneNode).map((c) => c.id);
}

export interface PaneGroupEntry {
  /// null for a pane that is not in any group (today's plain pane).
  groupId: Uuid | null;
  members: PaneSpec[];
  activeIndex: number;
}

/// Depth-first list of what the user sees as "panes": one entry per tab group
/// and one per ungrouped pane. Backs the workspace tree's Pane › Tab level.
export function paneGroups(root: LayoutNode): PaneGroupEntry[] {
  const out: PaneGroupEntry[] = [];
  const visit = (node: LayoutNode): void => {
    if (node.kind === "pane") {
      out.push({ groupId: null, members: [nodeToSpec(node)], activeIndex: 0 });
      return;
    }
    if (node.kind === "split") {
      visit(node.a);
      visit(node.b);
      return;
    }
    const members = node.children.filter(isPaneNode).map(nodeToSpec);
    if (members.length !== node.children.length) {
      // Nested tabs: not created by any command. Fall back to flattening.
      for (const c of node.children) visit(c);
      return;
    }
    out.push({
      groupId: node.id,
      members,
      activeIndex: clampIndex(node.active, members.length),
    });
  };
  visit(root);
  return out;
}

/// Depth-first list of the panes actually on screen: a group contributes only
/// its active tab. Used wherever "the pane the user works in" is meant —
/// active-pane tracking, focus cycling, focus fallback after a close.
export function visiblePanes(root: LayoutNode): PaneSpec[] {
  const out: PaneSpec[] = [];
  const visit = (node: LayoutNode): void => {
    if (node.kind === "pane") {
      out.push(nodeToSpec(node));
      return;
    }
    if (node.kind === "split") {
      visit(node.a);
      visit(node.b);
      return;
    }
    const child = node.children[clampIndex(node.active, node.children.length)];
    if (child) visit(child);
  };
  visit(root);
  return out;
}

export function isVisibleTab(root: LayoutNode, paneId: Uuid): boolean {
  return visiblePanes(root).some((p) => p.id === paneId);
}

/// Visible panes that are not tabs. `Ctrl+Shift+←/→` swaps slots in the
/// layout, which across a group boundary would move a terminal out of the
/// chrome it shares and pull an unrelated one in.
export function swappablePanes(root: LayoutNode): PaneSpec[] {
  const grouped = new Set<Uuid>();
  walk(root, (node) => {
    if (node.kind === "tabs") for (const id of tabIds(node)) grouped.add(id);
  });
  return visiblePanes(root).filter((p) => !grouped.has(p.id));
}

function isPaneNode(node: LayoutNode): node is PaneNode {
  return node.kind === "pane";
}

function clampIndex(index: number, length: number): number {
  if (length <= 0) return 0;
  return Math.min(length - 1, Math.max(0, Math.trunc(index)));
}

/// Rebuild the tree applying `f` to every node bottom-up, returning the same
/// reference when nothing changed so callers can use `===` as "no-op".
function mapTree(node: LayoutNode, f: (n: LayoutNode) => LayoutNode): LayoutNode {
  if (node.kind === "split") {
    const a = mapTree(node.a, f);
    const b = mapTree(node.b, f);
    const self = a === node.a && b === node.b ? node : { ...node, a, b };
    return f(self);
  }
  if (node.kind === "tabs") {
    const children = node.children.map((c) => mapTree(c, f));
    const changed = children.some((c, i) => c !== node.children[i]);
    return f(changed ? { ...node, children } : node);
  }
  return f(node);
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
