import { describe, it, expect } from "vitest";

import {
  spawnAction,
  describeAge,
  type ResumePlan,
} from "./resumePlan";

const plan: ResumePlan = {
  agent: "claude",
  command:
    "claude --resume 20aebce7-f8e2-4582-b480-bf1ae90d7a0b --dangerously-skip-permissions",
  cwd: "D:\\Git\\ymux",
  age_secs: 3 * 3600,
};

describe("spawnAction", () => {
  const resume = { kind: "resume" as const, plan };

  it("resumes instead of restoring when the backend offers a plan", () => {
    // The whole point of spec §4/§5: the replay is skipped, not layered under
    // the resumed conversation.
    expect(spawnAction({ outcome: resume, persistEnabled: true })).toEqual({
      kind: "resume",
      plan,
    });
  });

  it("resumes even with persistence off", () => {
    expect(spawnAction({ outcome: resume, persistEnabled: false })).toEqual({
      kind: "resume",
      plan,
    });
  });

  it("leaves a shell pane restoring its scrollback exactly as before", () => {
    for (const outcome of [{ kind: "none" as const }, null, undefined]) {
      expect(spawnAction({ outcome, persistEnabled: true })).toEqual({
        kind: "restore",
        missingAgent: null,
      });
    }
  });

  it("starts clean when there is neither a plan nor persistence", () => {
    expect(
      spawnAction({ outcome: { kind: "none" }, persistEnabled: false }),
    ).toEqual({ kind: "fresh", missingAgent: null });
  });

  it("carries the agent name through when the transcript is gone", () => {
    // Spec §4.3: the pane starts normally, but the user is told why the
    // conversation they expected back is not there. "No record at all" and
    // "a recent record whose transcript is gone" must stay distinguishable.
    const missing = { kind: "missing" as const, agent: "claude" };
    expect(spawnAction({ outcome: missing, persistEnabled: true })).toEqual({
      kind: "restore",
      missingAgent: "claude",
    });
    expect(spawnAction({ outcome: missing, persistEnabled: false })).toEqual({
      kind: "fresh",
      missingAgent: "claude",
    });
  });
});

describe("describeAge", () => {
  it("rounds down so the banner never overstates recency", () => {
    expect(describeAge(0)).toEqual({ unit: "now", count: 0 });
    expect(describeAge(59)).toEqual({ unit: "now", count: 0 });
    expect(describeAge(60)).toEqual({ unit: "minutes", count: 1 });
    expect(describeAge(3599)).toEqual({ unit: "minutes", count: 59 });
    expect(describeAge(3600)).toEqual({ unit: "hours", count: 1 });
    // The spec's own example line: "세션 복원 (claude · 3시간 전)".
    expect(describeAge(3 * 3600 + 59 * 60)).toEqual({
      unit: "hours",
      count: 3,
    });
  });

  it("survives a negative or fractional age from clock skew", () => {
    expect(describeAge(-5)).toEqual({ unit: "now", count: 0 });
    expect(describeAge(90.7)).toEqual({ unit: "minutes", count: 1 });
  });
});
