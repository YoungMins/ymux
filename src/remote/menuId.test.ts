import { describe, it, expect } from "vitest";
import type { AgentSnapshot } from "../types";
import { BLOCK_ROWS, MenuIds, agentKey, menuBlock, menuFingerprint, randomMenuId, type Gens } from "./menuId";

const opts = (...labels: string[]) => labels.map((label, i) => ({ n: i + 1, label }));
const BLOCK = ["Allow?", "1. Yes", "2. No"];
const FP = menuFingerprint(opts("Yes", "No"), BLOCK);
const at = (over: Partial<Gens> = {}): Gens => ({ fp: FP, input: 0, status: 0, ...over });

function ids() {
  let i = 0;
  return new MenuIds(() => `m${++i}`);
}

const snap = (status: "working" | "waiting" | "done" | "idle", tool: string | null = null): AgentSnapshot => ({
  p: { lead: { kind: "claude", status, source: "hook", tool }, subagents: [] },
});

describe("randomMenuId", () => {
  it("is a short URL-safe token and differs every time", () => {
    const seen = new Set<string>();
    for (let i = 0; i < 50; i++) {
      const id = randomMenuId();
      expect(id).toMatch(/^[A-Za-z0-9_-]{1,32}$/);
      seen.add(id);
    }
    expect(seen.size).toBe(50);
  });
});

describe("menuBlock", () => {
  const menu = ["❯ 1. Yes", "  2. No"];

  it("is empty without a menu", () => {
    expect(menuBlock(["$ ls", "file"])).toEqual([]);
  });

  it("takes the options and every row above them back to two blank rows", () => {
    const rows = ["earlier output", "", "", "● Bash(rm -rf build)", "  cwd: /x", "", "  Proceed?", ...menu];
    // One blank row inside the block does not end it; two in a row do.
    expect(menuBlock(rows)).toEqual(["● Bash(rm -rf build)", "cwd: /x", "", "Proceed?", "1. Yes", "2. No"]);
  });

  it("strips the box and the pointer so moving the pointer does not change it", () => {
    const a = menuBlock(["│ Proceed?      │", "│ ❯ 1. Yes      │", "│   2. No       │"]);
    const b = menuBlock(["│ Proceed?      │", "│   1. Yes      │", "│ ❯ 2. No       │"]);
    expect(a).toEqual(["Proceed?", "1. Yes", "2. No"]);
    expect(b).toEqual(a);
  });

  it("is at most 60 rows, counted up from the last option", () => {
    const rows = [...Array.from({ length: 150 }, (_, i) => `line ${i}`), ...menu];
    const block = menuBlock(rows);
    expect(block).toHaveLength(BLOCK_ROWS);
    expect(block.slice(-2)).toEqual(["1. Yes", "2. No"]);
    expect(block[0]).toBe("line 92");
  });

  it("includes rows between the options (a description under one)", () => {
    const rows = ["Proceed?", "❯ 1. Yes", "     runs it now", "  2. No"];
    expect(menuBlock(rows)).toEqual(["Proceed?", "1. Yes", "runs it now", "2. No"]);
  });

  it("differs for two questions that only differ above their last three lines", () => {
    const mk = (cmd: string) => [`● Bash(${cmd})`, "  one", "  two", "  Proceed?", ...menu];
    expect(menuBlock(mk("rm -rf /"))).not.toEqual(menuBlock(mk("ls")));
  });
});

describe("menuFingerprint", () => {
  it("is empty without options, and changes with options or block", () => {
    expect(menuFingerprint([], BLOCK)).toBe("");
    expect(menuFingerprint(opts("Yes", "No"), BLOCK)).toBe(FP);
    expect(menuFingerprint(opts("Yes", "Nope"), BLOCK)).not.toBe(FP);
    expect(menuFingerprint(opts("Yes", "No", "Maybe"), BLOCK)).not.toBe(FP);
    expect(menuFingerprint(opts("Yes", "No"), ["Allow this?", "1. Yes", "2. No"])).not.toBe(FP);
    expect(menuFingerprint(opts("Yes", "No"), [])).not.toBe(FP);
    // Numbers given as strings (the screen's spelling) fingerprint the same.
    expect(menuFingerprint([{ n: "1", label: "Yes" }, { n: "2", label: "No" }], BLOCK)).toBe(FP);
  });
});

describe("agentKey", () => {
  it("is the raw kind, status and tool; none without an agent", () => {
    expect(agentKey(null)).toBe("none");
    expect(agentKey(undefined)).toBe("none");
    const base = agentKey({ kind: "claude", status: "waiting", source: "hook", tool: null });
    expect(agentKey({ kind: "claude", status: "waiting", source: "hook", tool: null })).toBe(base);
    expect(agentKey({ kind: "claude", status: "working", source: "hook", tool: null })).not.toBe(base);
    expect(agentKey({ kind: "claude", status: "waiting", source: "hook", tool: "Bash" })).not.toBe(base);
    expect(agentKey({ kind: "codex", status: "waiting", source: "hook", tool: null })).not.toBe(base);
  });
});

describe("MenuIds", () => {
  it("mints an id when a menu is first seen and keeps it while nothing changed", () => {
    const m = ids();
    expect(m.issue("p", at())).toBe("m1");
    expect(m.issue("p", at())).toBe("m1");
    expect(m.current("p")).toBe("m1");
  });

  it("has no id without a menu, and the id is gone once the menu is", () => {
    const m = ids();
    expect(m.issue("p", at({ fp: "" }))).toBe("");
    m.issue("p", at());
    expect(m.issue("p", at({ fp: "" }))).toBe("");
    expect(m.current("p")).toBe("");
    // The same menu coming back is a new appearance.
    expect(m.issue("p", at())).toBe("m2");
  });

  it("mints a new id whenever any one of the three differs", () => {
    const m = ids();
    expect(m.issue("p", at())).toBe("m1");
    expect(m.issue("p", at({ fp: menuFingerprint(opts("Yes", "No"), ["Other?", "1. Yes", "2. No"]) }))).toBe("m2");
    expect(m.issue("p", at({ input: 1 }))).toBe("m3");
    expect(m.issue("p", at({ input: 1 }))).toBe("m3");
    expect(m.issue("p", at({ input: 1, status: 1 }))).toBe("m4");
  });

  it("holds only for the exact id and the exact values, and never mints", () => {
    const m = ids();
    const id = m.issue("p", at({ input: 3, status: 2 }));
    expect(m.holds("p", id, at({ input: 3, status: 2 }))).toBe(true);
    expect(m.holds("p", id, at({ input: 4, status: 2 }))).toBe(false);
    expect(m.holds("p", id, at({ input: 3, status: 3 }))).toBe(false);
    expect(m.holds("p", id, at({ input: 3, status: 2, fp: "x" }))).toBe(false);
    expect(m.holds("p", "m9", at({ input: 3, status: 2 }))).toBe(false);
    expect(m.holds("p", "", at({ input: 3, status: 2 }))).toBe(false);
    expect(m.holds("other", id, at({ input: 3, status: 2 }))).toBe(false);
    // Comparing changed nothing: the id is still the pane's.
    expect(m.current("p")).toBe(id);
    expect(m.issue("p", at({ input: 3, status: 2 }))).toBe(id);
  });

  it("keeps ids per pane", () => {
    const m = ids();
    expect(m.issue("a", at())).toBe("m1");
    expect(m.issue("b", at())).toBe("m2");
    expect(m.issue("a", at())).toBe("m1");
  });

  it("counts a status generation from the agent key and bumps it on every change", () => {
    const m = ids();
    expect(m.statusGen("p")).toBe(0);
    m.noteAgent("p", "a");
    const first = m.statusGen("p");
    m.noteAgent("p", "a");
    expect(m.statusGen("p")).toBe(first);
    m.noteAgent("p", "b");
    expect(m.statusGen("p")).toBe(first + 1);
    // A -> B -> A seen by the feed is two bumps: the id minted at A is dead.
    m.noteAgent("p", "a");
    expect(m.statusGen("p")).toBe(first + 2);
  });

  it("feeds the generation from agent snapshots: raw status and tool, panes that left", () => {
    const m = ids();
    m.observeAgents(snap("waiting"));
    const g = m.statusGen("p");
    m.observeAgents(snap("waiting"));
    expect(m.statusGen("p")).toBe(g);
    m.observeAgents(snap("waiting", "Bash"));
    expect(m.statusGen("p")).toBe(g + 1);
    m.observeAgents(snap("working", "Bash"));
    expect(m.statusGen("p")).toBe(g + 2);
    // The agent is gone from the snapshot: that is a change too.
    m.observeAgents({});
    expect(m.statusGen("p")).toBe(g + 3);
    m.observeAgents({});
    expect(m.statusGen("p")).toBe(g + 3);
  });

  it("an id from before the status moved cannot be used even for an identical menu", () => {
    const m = ids();
    m.observeAgents(snap("waiting"));
    const held = m.issue("p", at({ status: m.statusGen("p") }));
    m.observeAgents(snap("working"));
    m.observeAgents(snap("waiting"));
    expect(m.holds("p", held, at({ status: m.statusGen("p") }))).toBe(false);
    expect(m.issue("p", at({ status: m.statusGen("p") }))).not.toBe(held);
  });

  it("retires an id after an answer and hands out none for the same menu until the agent moves on", () => {
    const m = ids();
    const first = m.issue("p", at());
    m.retire("p");
    expect(m.current("p")).toBe("");
    expect(m.holds("p", first, at())).toBe(false);
    // Still painted (and the user's own input moves the generation): no new id.
    expect(m.issue("p", at())).toBe("");
    expect(m.issue("p", at({ input: 5 }))).toBe("");
    // A different menu is a new one.
    const other = menuFingerprint(opts("A", "B"), ["Next?", "1. A", "2. B"]);
    expect(m.issue("p", at({ fp: other, input: 5 }))).toBe("m2");
    // The same one again after the agent's status moved: also new.
    m.retire("p");
    expect(m.issue("p", at({ fp: other, status: 1 }))).toBe("m3");
    // Or after the menu disappeared.
    m.retire("p");
    expect(m.issue("p", at({ fp: "" }))).toBe("");
    expect(m.issue("p", at({ fp: other, status: 1 }))).toBe("m4");
  });

  it("retire without a live id does nothing", () => {
    const m = ids();
    m.retire("p");
    expect(m.issue("p", at())).toBe("m1");
  });

  it("forgets panes that are gone", () => {
    const m = ids();
    m.issue("a", at());
    m.issue("b", at());
    m.noteAgent("b", "x");
    m.keepOnly(["a"]);
    expect(m.current("a")).toBe("m1");
    expect(m.current("b")).toBe("");
    expect(m.statusGen("b")).toBe(0);
  });
});
