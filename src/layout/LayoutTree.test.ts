import { describe, it, expect } from "vitest";
import { newPane, paneNode, splitPane, nodeToSpec } from "./LayoutTree";
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
