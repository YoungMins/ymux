import { describe, it, expect } from "vitest";
import { newPane, paneNode, splitPane, nodeToSpec, worktreePaths } from "./LayoutTree";
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
