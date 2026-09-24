import { describe, it, expect } from "vitest";

import {
  spawnAction,
  shouldPersistScrollback,
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
  it("resumes instead of restoring when the backend offers a plan", () => {
    // The whole point of spec §4/§5: the replay is skipped, not layered under
    // the resumed conversation.
    expect(spawnAction({ plan, persistEnabled: true })).toEqual({
      kind: "resume",
      plan,
    });
  });

  it("resumes even with persistence off", () => {
    expect(spawnAction({ plan, persistEnabled: false })).toEqual({
      kind: "resume",
      plan,
    });
  });

  it("leaves a shell pane restoring its scrollback exactly as before", () => {
    expect(spawnAction({ plan: null, persistEnabled: true })).toEqual({
      kind: "restore",
    });
    expect(spawnAction({ plan: undefined, persistEnabled: true })).toEqual({
      kind: "restore",
    });
  });

  it("starts clean when there is neither a plan nor persistence", () => {
    expect(spawnAction({ plan: null, persistEnabled: false })).toEqual({
      kind: "fresh",
    });
  });
});

describe("shouldPersistScrollback", () => {
  it("stops saving for a pane that resumed its agent", () => {
    expect(
      shouldPersistScrollback({ persistEnabled: true, resuming: true }),
    ).toBe(false);
  });

  it("keeps saving for every other pane", () => {
    expect(
      shouldPersistScrollback({ persistEnabled: true, resuming: false }),
    ).toBe(true);
  });

  it("never saves when persistence is off", () => {
    expect(
      shouldPersistScrollback({ persistEnabled: false, resuming: false }),
    ).toBe(false);
    expect(
      shouldPersistScrollback({ persistEnabled: false, resuming: true }),
    ).toBe(false);
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
