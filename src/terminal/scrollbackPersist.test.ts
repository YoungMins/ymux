import { describe, it, expect } from "vitest";
import { shouldSaveScrollback } from "./scrollbackPersist";

describe("shouldSaveScrollback", () => {
  it("saves only when persistence is on AND the user did something this session", () => {
    expect(shouldSaveScrollback({ persistEnabled: true, hadUserActivity: true })).toBe(true);
  });

  it("does NOT save an idle session, so a restored-then-untouched pane can't pile up", () => {
    // The reported bug: an idle terminal re-serialized its own restored
    // history + separator + guard every open/close, compounding to the cap.
    // Gating on real activity is what stops that.
    expect(shouldSaveScrollback({ persistEnabled: true, hadUserActivity: false })).toBe(false);
  });

  it("never saves when persistence is disabled, regardless of activity", () => {
    expect(shouldSaveScrollback({ persistEnabled: false, hadUserActivity: true })).toBe(false);
    expect(shouldSaveScrollback({ persistEnabled: false, hadUserActivity: false })).toBe(false);
  });
});
