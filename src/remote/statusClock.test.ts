import { describe, it, expect } from "vitest";
import { StatusClock } from "./statusClock";

function rig() {
  const t = { now: 1000 };
  return { t, clock: new StatusClock(() => t.now) };
}

describe("StatusClock", () => {
  it("seeds the first sighting from the seed, else now", () => {
    const { clock } = rig();
    expect(clock.observe("a", "working", 400)).toBe(400);
    expect(clock.observe("b", "working", null)).toBe(1000);
  });

  it("ignores a seed from the future", () => {
    const { clock } = rig();
    expect(clock.observe("a", "working", 5000)).toBe(1000);
  });

  it("keeps the time while the reported status is unchanged, whatever the seed does", () => {
    const { t, clock } = rig();
    expect(clock.observe("a", "working", 400)).toBe(400);
    t.now = 9000;
    expect(clock.observe("a", "working", 8000)).toBe(400);
    expect(clock.observe("a", "working", null)).toBe(400);
  });

  it("moves to now when the reported status changes, ignoring the seed", () => {
    const { t, clock } = rig();
    clock.observe("a", "working", 400);
    t.now = 2000;
    expect(clock.observe("a", "waiting", 400)).toBe(2000);
    t.now = 3000;
    expect(clock.observe("a", "waiting", null)).toBe(2000);
    expect(clock.observe("a", "working", null)).toBe(3000);
  });

  it("read has no side effects: it answers only for the observed status", () => {
    const { t, clock } = rig();
    expect(clock.read("a", "working")).toBeNull();
    // A read never records a first sighting.
    t.now = 5000;
    expect(clock.observe("a", "working", null)).toBe(5000);
    t.now = 6000;
    expect(clock.read("a", "working")).toBe(5000);
    expect(clock.read("a", "done")).toBeNull();
    // ... and a read of another status did not move anything.
    expect(clock.read("a", "working")).toBe(5000);
    expect(clock.observe("a", "working", null)).toBe(5000);
  });

  it("starts from when the clock first saw the status, however long the link was down", () => {
    // The clock runs on every heartbeat; remote being off changes nothing.
    const { t, clock } = rig();
    clock.observe("a", "working", null);
    for (t.now = 3000; t.now <= 600_000; t.now += 2000) clock.observe("a", "working", null);
    // The link comes up: the age is that of the status, not of the link.
    expect(clock.read("a", "working")).toBe(1000);
  });

  it("forgets a pane that is gone, one by one or by what is still listed", () => {
    const { t, clock } = rig();
    clock.observe("a", "working", 400);
    clock.observe("b", "done", null);
    clock.observe("c", "idle", null);
    clock.forget("a");
    clock.retainOnly(["b"]);
    expect(clock.read("a", "working")).toBeNull();
    expect(clock.read("c", "idle")).toBeNull();
    expect(clock.read("b", "done")).toBe(1000);
    t.now = 5000;
    expect(clock.observe("a", "working", null)).toBe(5000);
    clock.forget("never-seen");
  });
});
