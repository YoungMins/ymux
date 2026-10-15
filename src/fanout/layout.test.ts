import { describe, expect, it } from "vitest";
import { attachNode, fanoutLayout } from "./layout";
import { newPane } from "../layout/LayoutTree";
import type { LayoutNode } from "../types";

const specs = (n: number) => Array.from({ length: n }, () => newPane("pwsh", "C:/x"));
const ids = (n: LayoutNode): string[] =>
  n.kind === "pane" ? [n.id] : n.kind === "split" ? [...ids(n.a), ...ids(n.b)] : n.children.flatMap(ids);

describe("fanoutLayout", () => {
  it("throws on empty", () => {
    expect(() => fanoutLayout([])).toThrow();
  });
  it("1 -> single pane", () => {
    const s = specs(1);
    const n = fanoutLayout(s);
    expect(n.kind).toBe("pane");
    expect(ids(n)).toEqual([s[0].id]);
  });
  it("2 -> side by side", () => {
    expect(fanoutLayout(specs(2))).toMatchObject({ kind: "split", direction: "horizontal", ratio: 0.5 });
  });
  it("3 -> two top, one bottom", () => {
    const s = specs(3);
    const n = fanoutLayout(s);
    if (n.kind !== "split") throw new Error("not split");
    expect(n.direction).toBe("vertical");
    expect(n.a).toMatchObject({ kind: "split", direction: "horizontal" });
    expect(n.b.kind).toBe("pane");
    expect(ids(n)).toEqual(s.map((x) => x.id));
  });
  it("4 -> 2x2 in order", () => {
    const s = specs(4);
    const n = fanoutLayout(s);
    if (n.kind !== "split") throw new Error("not split");
    expect(n.a.kind).toBe("split");
    expect(n.b.kind).toBe("split");
    expect(ids(n)).toEqual(s.map((x) => x.id));
  });
  it("5+ -> one tab group with all panes", () => {
    const s = specs(7);
    const n = fanoutLayout(s);
    expect(n).toMatchObject({ kind: "tabs", active: 0 });
    if (n.kind !== "tabs") throw new Error("not tabs");
    expect(n.children.every((c) => c.kind === "pane")).toBe(true);
    expect(ids(n)).toEqual(s.map((x) => x.id));
  });
  it("forceTabs groups even small counts", () => {
    expect(fanoutLayout(specs(2), true).kind).toBe("tabs");
  });
  it("carries pane fields through paneNode", () => {
    const s = specs(1);
    s[0].startup_cmd = "claude";
    expect(fanoutLayout(s)).toMatchObject({ kind: "pane", startup_cmd: "claude", cwd: "C:/x" });
  });
});

describe("attachNode", () => {
  it("puts the node beside the target, leaving the target in place", () => {
    const target = newPane("pwsh", "C:/t");
    const lanes = fanoutLayout(specs(2));
    const out = attachNode({ kind: "pane", ...target } as LayoutNode, target.id, lanes);
    expect(out.kind).toBe("split");
    expect(ids(out)[0]).toBe(target.id);
    expect(ids(out)).toHaveLength(3);
  });
  it("returns the same tree when the target is missing", () => {
    const root = fanoutLayout(specs(2));
    expect(attachNode(root, "nope", fanoutLayout(specs(1)))).toBe(root);
  });
  it("splits a tab group as a whole", () => {
    const tabs = fanoutLayout(specs(5));
    const first = ids(tabs)[0];
    const out = attachNode(tabs, first, fanoutLayout(specs(2)));
    expect(out.kind).toBe("split");
    expect(ids(out)).toHaveLength(7);
  });
  it("keeps account, worktree_path and title through paneNode", () => {
    const s = specs(1);
    s[0].account = "ab12cd34";
    s[0].worktree_path = "C:/wt";
    s[0].title = "fanout/claude-1";
    expect(fanoutLayout(s)).toMatchObject({ account: "ab12cd34", worktree_path: "C:/wt", title: "fanout/claude-1" });
  });
});
