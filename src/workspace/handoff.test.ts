import { describe, expect, it } from "vitest";
import { handoffStep, HANDOFF_SCAN_GRACE_MS, HANDOFF_TTL_MS, type Handoff } from "./handoff";

const make = (over: Partial<Handoff> = {}): Handoff => ({
  text: "n",
  startedAt: 1000,
  respawnedAt: null,
  seenGone: false,
  ...over,
});

describe("handoffStep", () => {
  it("never queues while the respawn is still running, even with a lead", () => {
    const h = make();
    expect(handoffStep(h, true, 99_999, 2000)).toBe("wait");
  });

  it("does not queue the old lead lingering right after the respawn", () => {
    const h = make({ respawnedAt: 2000 });
    expect(handoffStep(h, true, 2000 + HANDOFF_SCAN_GRACE_MS - 1, 2500)).toBe("wait");
  });

  it("queues once the old lead was seen gone and a lead is back", () => {
    const h = make({ respawnedAt: 2000 });
    expect(handoffStep(h, false, 2100, 2200)).toBe("wait");
    expect(h.seenGone).toBe(true);
    expect(handoffStep(h, true, 2300, 2400)).toBe("queue");
  });

  it("queues when a scan sampled well after the respawn shows a lead", () => {
    const h = make({ respawnedAt: 2000 });
    expect(handoffStep(h, true, 2000 + HANDOFF_SCAN_GRACE_MS, 9000)).toBe("queue");
  });

  it("drops after the TTL", () => {
    const h = make({ respawnedAt: 2000 });
    expect(handoffStep(h, false, 0, 1000 + HANDOFF_TTL_MS + 1)).toBe("drop");
  });
});
