import { describe, it, expect } from "vitest";
import { newPane, paneNode, panes } from "./LayoutTree";
import {
  activateTabFor,
  addTab,
  findGroup,
  groupOfPane,
  isVisibleTab,
  paneGroups,
  setActiveTab,
  sameTabIds,
  stepTab,
  swappablePanes,
  tabIds,
  tabsNode,
  visiblePanes,
  wrapInTabs,
} from "./tabs";
import type { LayoutNode } from "../types";

const A = newPane("pwsh");
const B = newPane("cmd");
const C = newPane("bash");
const D = newPane("zsh");
const GROUP = "gggggggg-gggg-4ggg-8ggg-gggggggggggg";

function split(a: LayoutNode, b: LayoutNode): LayoutNode {
  return { kind: "split", direction: "horizontal", ratio: 0.5, a, b };
}

describe("wrapInTabs", () => {
  it("replaces the pane with a one-tab group containing it", () => {
    const root = wrapInTabs(paneNode(A), A.id, GROUP);
    expect(root.kind).toBe("tabs");
    const group = findGroup(root, GROUP)!;
    expect(group.id).toBe(GROUP);
    expect(group.active).toBe(0);
    expect(tabIds(group)).toEqual([A.id]);
  });

  it("wraps a pane that sits inside a split, leaving the sibling alone", () => {
    const root = wrapInTabs(split(paneNode(A), paneNode(B)), B.id, GROUP);
    expect(root.kind).toBe("split");
    expect(groupOfPane(root, B.id)?.id).toBe(GROUP);
    expect(groupOfPane(root, A.id)).toBeNull();
  });

  it("is a no-op for an unknown pane id", () => {
    const before = paneNode(A);
    expect(wrapInTabs(before, B.id, GROUP)).toBe(before);
  });
});

describe("addTab", () => {
  it("appends the new tab and makes it active", () => {
    let root = wrapInTabs(paneNode(A), A.id, GROUP);
    root = addTab(root, GROUP, B);
    const group = findGroup(root, GROUP)!;
    expect(tabIds(group)).toEqual([A.id, B.id]);
    expect(group.active).toBe(1);
  });

  it("is a no-op for an unknown group", () => {
    const before = wrapInTabs(paneNode(A), A.id, GROUP);
    expect(addTab(before, "nope", B)).toBe(before);
  });
});

describe("setActiveTab / stepTab / activateTabFor", () => {
  const threeTabs = addTab(
    addTab(wrapInTabs(paneNode(A), A.id, GROUP), GROUP, B),
    GROUP,
    C,
  );

  it("clamps an out-of-range index instead of breaking the tree", () => {
    expect(findGroup(setActiveTab(threeTabs, GROUP, 9), GROUP)!.active).toBe(2);
    expect(findGroup(setActiveTab(threeTabs, GROUP, -4), GROUP)!.active).toBe(0);
  });

  it("wraps at both ends", () => {
    const group = findGroup(threeTabs, GROUP)!;
    expect(stepTab(group, 1)).toBe(0); // active is 2 of 3
    expect(stepTab(tabsNode(GROUP, group.children, 0), -1)).toBe(2);
    expect(stepTab(tabsNode(GROUP, group.children, 0), 1)).toBe(1);
  });

  it("makes a hidden tab the active one, by pane id", () => {
    const root = activateTabFor(threeTabs, A.id);
    expect(findGroup(root, GROUP)!.active).toBe(0);
  });

  it("leaves an ungrouped pane's tree untouched", () => {
    const plain = split(paneNode(A), paneNode(B));
    expect(activateTabFor(plain, A.id)).toBe(plain);
  });
});

describe("visiblePanes / isVisibleTab", () => {
  const root = split(
    setActiveTab(addTab(wrapInTabs(paneNode(A), A.id, GROUP), GROUP, B), GROUP, 1),
    paneNode(C),
  );

  it("lists every pane for panes(), but only the active tab for visiblePanes()", () => {
    expect(panes(root).map((p) => p.id)).toEqual([A.id, B.id, C.id]);
    expect(visiblePanes(root).map((p) => p.id)).toEqual([B.id, C.id]);
  });

  it("answers isVisibleTab per pane, and false for an unknown id", () => {
    expect(isVisibleTab(root, B.id)).toBe(true);
    expect(isVisibleTab(root, A.id)).toBe(false);
    expect(isVisibleTab(root, C.id)).toBe(true);
    expect(isVisibleTab(root, D.id)).toBe(false);
  });
});

describe("swappablePanes", () => {
  it("excludes every tab of a group, so a swap can't break chrome sharing", () => {
    const root = split(
      addTab(wrapInTabs(paneNode(A), A.id, GROUP), GROUP, B),
      paneNode(C),
    );
    expect(swappablePanes(root).map((p) => p.id)).toEqual([C.id]);
  });
});

describe("paneGroups", () => {
  it("returns one entry per group and one per ungrouped pane, in tree order", () => {
    const root = split(
      setActiveTab(addTab(wrapInTabs(paneNode(A), A.id, GROUP), GROUP, B), GROUP, 1),
      paneNode(C),
    );
    const groups = paneGroups(root);
    expect(groups).toHaveLength(2);
    expect(groups[0].groupId).toBe(GROUP);
    expect(groups[0].members.map((p) => p.id)).toEqual([A.id, B.id]);
    expect(groups[0].activeIndex).toBe(1);
    expect(groups[1].groupId).toBeNull();
    expect(groups[1].members.map((p) => p.id)).toEqual([C.id]);
    expect(groups[1].activeIndex).toBe(0);
  });
});

describe("sameTabIds", () => {
  it("is true only for the same ids in the same order", () => {
    expect(sameTabIds([A.id, B.id], [A.id, B.id])).toBe(true);
    expect(sameTabIds([], [])).toBe(true);
  });

  it("is false when a tab was added, removed or replaced", () => {
    expect(sameTabIds([A.id], [A.id, B.id])).toBe(false);
    expect(sameTabIds([A.id, B.id], [A.id])).toBe(false);
    expect(sameTabIds([A.id, B.id], [A.id, C.id])).toBe(false);
  });

  it("is false for the same set in a different order, so the strip is rebuilt", () => {
    expect(sameTabIds([A.id, B.id], [B.id, A.id])).toBe(false);
  });
});
