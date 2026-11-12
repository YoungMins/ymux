import { describe, expect, it } from "vitest";
import { agentDisplayName, joinAgentNames, liveAgentNames } from "./closeConfirm";
import type { AgentSnapshot } from "../types";

const snap = (m: Record<string, string | null>): AgentSnapshot =>
  Object.fromEntries(
    Object.entries(m).map(([id, kind]) => [id, { lead: kind ? ({ kind } as never) : null, subagents: [] }]),
  );

describe("closeConfirm", () => {
  it("capitalises kinds", () => {
    expect(agentDisplayName("claude")).toBe("Claude");
    expect(agentDisplayName("")).toBe("");
  });
  it("lists leads of the given panes only", () => {
    const a = snap({ p1: "claude", p2: null, p3: "codex" });
    expect(liveAgentNames(a, ["p1", "p2"])).toEqual(["Claude"]);
    expect(liveAgentNames(a, ["p2", "p4"])).toEqual([]);
    expect(liveAgentNames(a, ["p1", "p3"])).toEqual(["Claude", "Codex"]);
  });
  it("folds repeats", () => {
    expect(joinAgentNames(["Claude", "Claude", "Codex"])).toBe("Claude ×2, Codex");
  });
});
