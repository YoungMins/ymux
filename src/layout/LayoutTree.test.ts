import { describe, it, expect } from "vitest";
import {
  newPane,
  paneNode,
  panes,
  removePane,
  splitPane,
  nodeToSpec,
  worktreePaths,
} from "./LayoutTree";
import type { LayoutNode } from "../types";

describe("paneNode / splitPane field persistence", () => {
  it("paneNode -> nodeToSpec round-trips worktree_path and bg_color", () => {
    const spec = newPane("bash", "/repo");
    spec.worktree_path = "/repo/.worktrees/feature-x";
    spec.bg_color = "#112233";

    const node = paneNode(spec) as LayoutNode & { kind: "pane" };
    const roundTripped = nodeToSpec(node);

    expect(roundTripped.worktree_path).toBe("/repo/.worktrees/feature-x");
    expect(roundTripped.bg_color).toBe("#112233");
  });

  it("splitPane inserts a 'b' node that carries worktree_path and bg_color", () => {
    const rootSpec = newPane("bash", "/repo");
    const rootNode = paneNode(rootSpec) as LayoutNode & { kind: "pane" };

    const newSpec = newPane("bash", "/repo/.worktrees/feature-x");
    newSpec.worktree_path = "/repo/.worktrees/feature-x";
    newSpec.bg_color = "#445566";

    const tree = splitPane(rootNode, rootSpec.id, "horizontal", newSpec);

    expect(tree.kind).toBe("split");
    if (tree.kind !== "split") throw new Error("expected split node");
    expect(tree.b.kind).toBe("pane");
    if (tree.b.kind !== "pane") throw new Error("expected pane node");

    const insertedSpec = nodeToSpec(tree.b);

    expect(insertedSpec.worktree_path).toBe("/repo/.worktrees/feature-x");
    expect(insertedSpec.bg_color).toBe("#445566");
  });
});

describe("worktreePaths", () => {
  it("collects worktree paths from every pane in the tree, skipping panes without one", () => {
    const wtSpec = newPane("bash", "/repo/.worktrees/feature-x");
    wtSpec.worktree_path = "/repo/.worktrees/feature-x";
    const plainSpec = newPane("bash", "/repo");
    const otherWtSpec = newPane("bash", "/repo/.worktrees/feature-y");
    otherWtSpec.worktree_path = "/repo/.worktrees/feature-y";

    const tree: LayoutNode = {
      kind: "split",
      direction: "horizontal",
      ratio: 0.5,
      a: paneNode(wtSpec),
      b: {
        kind: "split",
        direction: "vertical",
        ratio: 0.5,
        a: paneNode(plainSpec),
        b: paneNode(otherWtSpec),
      },
    };

    expect(worktreePaths(tree)).toEqual([
      "/repo/.worktrees/feature-x",
      "/repo/.worktrees/feature-y",
    ]);
  });

  it("returns an empty list for a tree with no worktree panes", () => {
    expect(worktreePaths(paneNode(newPane("bash", "/repo")))).toEqual([]);
  });
});

describe("removePane in a tab group", () => {
  const G = "gggggggg-gggg-4ggg-8ggg-gggggggggggg";
  const t1 = newPane("pwsh");
  const t2 = newPane("cmd");
  const t3 = newPane("bash");
  const group: LayoutNode = {
    kind: "tabs",
    id: G,
    active: 2,
    children: [paneNode(t1), paneNode(t2), paneNode(t3)],
  };

  it("keeps the same tab active when a tab below it is closed", () => {
    const next = removePane(group, t1.id)!;
    expect(next.kind).toBe("tabs");
    const tabs = next as LayoutNode & { kind: "tabs" };
    expect(tabs.children).toHaveLength(2);
    expect(tabs.active).toBe(1);
    expect(panes(tabs).map((p) => p.id)).toEqual([t2.id, t3.id]);
  });

  // With `active: 2` above, clamping to `children.length - 1` happens to give
  // the same answer as decrementing, so it cannot tell the two apart. This
  // case can: clamping would leave `active` at 1 (showing t3) instead of
  // following t2 down to 0.
  it("follows the active tab down when a tab below it is closed", () => {
    const midActive: LayoutNode = {
      kind: "tabs",
      id: G,
      active: 1,
      children: [paneNode(t1), paneNode(t2), paneNode(t3)],
    };
    const next = removePane(midActive, t1.id)! as LayoutNode & { kind: "tabs" };
    expect(next.active).toBe(0);
    expect(panes(next).map((p) => p.id)).toEqual([t2.id, t3.id]);
  });

  it("clamps when the active tab itself is closed", () => {
    const next = removePane(group, t3.id)! as LayoutNode & { kind: "tabs" };
    expect(next.active).toBe(1);
  });

  it("unwraps back to a plain pane when one tab is left", () => {
    const twoTabs: LayoutNode = {
      kind: "tabs",
      id: G,
      active: 1,
      children: [paneNode(t1), paneNode(t2)],
    };
    const next = removePane(twoTabs, t2.id)!;
    expect(next.kind).toBe("pane");
    expect((next as LayoutNode & { kind: "pane" }).id).toBe(t1.id);
  });

  it("removes the whole group when its last tab goes, collapsing the split", () => {
    const other = newPane("zsh");
    const root: LayoutNode = {
      kind: "split",
      direction: "horizontal",
      ratio: 0.5,
      a: { kind: "tabs", id: G, active: 0, children: [paneNode(t1)] },
      b: paneNode(other),
    };
    const next = removePane(root, t1.id)!;
    expect(next.kind).toBe("pane");
    expect((next as LayoutNode & { kind: "pane" }).id).toBe(other.id);
  });

  it("returns the same reference when the pane is not in the group", () => {
    expect(removePane(group, newPane("fish").id)).toBe(group);
  });
});
