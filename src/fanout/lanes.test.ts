import { describe, expect, it } from "vitest";
import { laneBranches, newRunId, runShort, validateLanes, MAX_LANES, MAX_PROMPT_BYTES } from "./lanes";

const lanes = (n: number) => Array.from({ length: n }, () => ({ agent: "claude", account: "" }));

describe("laneBranches", () => {
  it("numbers lanes by index and keeps them unique", () => {
    const b = laneBranches("fanout", "r1", [
      { agent: "claude", account: "" },
      { agent: "codex", account: "" },
      { agent: "claude", account: "x" },
    ]);
    expect(b).toEqual(["fanout/r1-claude-1", "fanout/r1-codex-2", "fanout/r1-claude-3"]);
    expect(new Set(b).size).toBe(3);
  });
  it("sanitizes to git ref rules", () => {
    const [b] = laneBranches("-my feat..x/", "r", [{ agent: "we ird~^:", account: "" }]);
    expect(b).not.toMatch(/\s|\.\.|[~^:?*[\\]/);
    expect(b.startsWith("-")).toBe(false);
    expect(b.startsWith("/")).toBe(false);
    expect(b.endsWith("/")).toBe(false);
    expect(b.endsWith(".lock")).toBe(false);
    expect(b.endsWith(".")).toBe(false);
  });
  it("falls back when prefix is empty or all invalid", () => {
    expect(laneBranches("", "r", [{ agent: "claude", account: "" }])[0]).toBe("fanout/r-claude-1");
    expect(laneBranches("///", "r", [{ agent: "claude", account: "" }])[0]).toBe("fanout/r-claude-1");
  });
  it("never ends in .lock", () => {
    const [b] = laneBranches("a.lock", "r", [{ agent: "x", account: "" }]);
    expect(b.endsWith(".lock")).toBe(false);
    expect(b.split("/").some((s) => s.endsWith(".lock"))).toBe(false);
  });
  it("sanitizes an agent ending in .lock", () => {
    const [b] = laneBranches("p", "r", [{ agent: "x.lock", account: "" }]);
    expect(b.split("/").some((s) => s.endsWith(".lock"))).toBe(false);
  });
});

describe("laneBranches across runs", () => {
  const one = [{ agent: "claude", account: "" }];
  it("two runs get different names for the same lane", () => {
    const a = laneBranches("fanout", "20261016-0705-ab12", one);
    const b = laneBranches("fanout", "20261016-0705-cd34", one);
    expect(a[0]).toBe("fanout/05ab12-claude-1");
    expect(b[0]).toBe("fanout/05cd34-claude-1");
    expect(a[0]).not.toBe(b[0]);
  });
  it("skips names that already exist", () => {
    const [n] = laneBranches("fanout", "r1", one, ["fanout/r1-claude-1"]);
    expect(n).toBe("fanout/r1-claude-1-2");
    const [m] = laneBranches("fanout", "r1", one, ["fanout/r1-claude-1", "fanout/r1-claude-1-2"]);
    expect(m).toBe("fanout/r1-claude-1-3");
  });
  it("keeps lanes of one run unique when existing collides with a sibling", () => {
    const b = laneBranches("f", "r1", [
      { agent: "claude", account: "" },
      { agent: "claude", account: "" },
    ], ["f/r1-claude-1"]);
    expect(new Set(b).size).toBe(2);
    expect(b).not.toContain("f/r1-claude-1");
  });
  it("runShort takes the last 6 alphanumerics and survives odd ids", () => {
    expect(runShort("20261016-0705-ab12")).toBe("05ab12");
    expect(runShort("ab")).toBe("ab");
    expect(runShort("---")).toBe("run");
  });
});

describe("newRunId", () => {
  it("formats yyyymmdd-hhmm-xxxx", () => {
    const id = newRunId(new Date(2026, 9, 16, 7, 5), () => 0.5);
    expect(id).toMatch(/^20261016-0705-[a-z0-9]{4}$/);
    expect(id).toMatch(/^[A-Za-z0-9-]{1,64}$/);
  });
  it("is deterministic for the same inputs and varies with rand", () => {
    const d = new Date(2026, 0, 2, 3, 4);
    expect(newRunId(d, () => 0.1)).toBe(newRunId(d, () => 0.1));
    expect(newRunId(d, () => 0.1)).not.toBe(newRunId(d, () => 0.9));
  });
  it("handles rand returning 0 and values near 1", () => {
    const d = new Date(2026, 0, 2, 3, 4);
    expect(newRunId(d, () => 0)).toMatch(/^[A-Za-z0-9-]{1,64}$/);
    expect(newRunId(d, () => 0.9999999)).toMatch(/-[a-z0-9]{4}$/);
  });
});

describe("validateLanes", () => {
  it("accepts 1..8 lanes with a prompt", () => {
    expect(validateLanes(lanes(1), "hi")).toBeNull();
    expect(validateLanes(lanes(MAX_LANES), "hi")).toBeNull();
  });
  it("rejects 0 and 9 lanes", () => {
    expect(validateLanes([], "hi")).toBe("lanes");
    expect(validateLanes(lanes(MAX_LANES + 1), "hi")).toBe("lanes");
  });
  it("rejects empty or whitespace prompt", () => {
    expect(validateLanes(lanes(1), "")).toBe("prompt-empty");
    expect(validateLanes(lanes(1), " \n\t")).toBe("prompt-empty");
  });
  it("caps prompt at 32 KiB in UTF-8 bytes", () => {
    expect(validateLanes(lanes(1), "a".repeat(MAX_PROMPT_BYTES))).toBeNull();
    expect(validateLanes(lanes(1), "a".repeat(MAX_PROMPT_BYTES + 1))).toBe("prompt-too-long");
    expect(validateLanes(lanes(1), "한".repeat(11000))).toBe("prompt-too-long");
  });
});
