import { describe, expect, it } from "vitest";
import { agentBadges, elapsedText } from "./agentTiming";
import { LONG_TOOL_MS, SILENT_MS } from "../office/officeQueue";

describe("elapsedText", () => {
  it("is compact at every scale", () => {
    expect(elapsedText(0)).toBe("0s");
    expect(elapsedText(45_000)).toBe("45s");
    expect(elapsedText(59_999)).toBe("59s");
    expect(elapsedText(60_000)).toBe("1m");
    expect(elapsedText(12 * 60_000 + 30_000)).toBe("12m");
    expect(elapsedText(59 * 60_000)).toBe("59m");
    expect(elapsedText(63 * 60_000)).toBe("1h03m");
    expect(elapsedText(120 * 60_000)).toBe("2h00m");
  });
  it("clamps a clock that ran backwards", () => {
    expect(elapsedText(-5000)).toBe("0s");
  });
});

describe("agentBadges", () => {
  const now = 10_000_000;
  it("reports unknown start as no elapsed", () => {
    const b = agentBadges("idle", null, { since: null, outputAt: null, toolSince: null }, now);
    expect(b).toEqual({ elapsed: null, tool: null, stuck: null });
  });
  it("uses the office thresholds for stuck", () => {
    const quiet = { since: now - 1000, outputAt: now - SILENT_MS, toolSince: null };
    expect(agentBadges("working", null, quiet, now).stuck).toBe("silent");
    const longTool = { since: now - 1000, outputAt: now - 1000, toolSince: now - LONG_TOOL_MS };
    expect(agentBadges("working", "Bash", longTool, now).stuck).toBe("tool");
  });
  it("is never stuck unless working", () => {
    const quiet = { since: now - 1000, outputAt: now - SILENT_MS * 2, toolSince: now - LONG_TOOL_MS * 2 };
    expect(agentBadges("waiting", "Bash", quiet, now).stuck).toBeNull();
    expect(agentBadges("done", "Bash", quiet, now).stuck).toBeNull();
  });
});
