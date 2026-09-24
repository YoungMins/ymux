import { describe, it, expect, beforeEach } from "vitest";
import { FollowGate, QUIET_MS, followDelay } from "./followGate";

describe("followDelay", () => {
  it("applies at once when the list is not focused", () => {
    expect(followDelay(1000, 990, false)).toBe(0);
  });
  it("applies at once when no key was ever pressed", () => {
    expect(followDelay(1000, null, true)).toBe(0);
  });
  it("waits out the quiet period after a key in the focused list", () => {
    expect(followDelay(1000, 900, true)).toBe(QUIET_MS - 100);
    expect(followDelay(1000, 1000 - QUIET_MS, true)).toBe(0);
  });
});

describe("FollowGate", () => {
  let clock: number;
  let focused: boolean;
  let applied: string[];
  let timers: { at: number; fn: () => void }[];
  let gate: FollowGate;

  const advance = (ms: number) => {
    clock += ms;
    const due = timers.filter((t) => t.at <= clock);
    timers = timers.filter((t) => t.at > clock);
    for (const t of due) t.fn();
  };

  beforeEach(() => {
    clock = 0;
    focused = true;
    applied = [];
    timers = [];
    gate = new FollowGate({
      now: () => clock,
      focused: () => focused,
      apply: (dir) => applied.push(dir),
      setTimer: (fn, ms) => {
        const t = { at: clock + ms, fn };
        timers.push(t);
        return () => {
          timers = timers.filter((x) => x !== t);
        };
      },
    });
  });

  it("applies immediately when the user is idle", () => {
    gate.offer("/a");
    expect(applied).toEqual(["/a"]);
  });

  it("holds a follow while the user is typing in the list, then applies the latest", () => {
    gate.onKey();
    gate.offer("/a");
    advance(100);
    gate.offer("/b");
    expect(applied).toEqual([]);
    advance(100);
    gate.onKey(); // still typing: the quiet period restarts
    advance(QUIET_MS - 1);
    expect(applied).toEqual([]);
    advance(1);
    expect(applied).toEqual(["/b"]);
  });

  it("applies once focus leaves the list", () => {
    gate.onKey();
    gate.offer("/a");
    focused = false;
    advance(0);
    gate.offer("/b");
    expect(applied).toEqual(["/b"]);
    advance(QUIET_MS);
    expect(applied).toEqual(["/b"]);
  });

  it("recheck applies a held follow as soon as focus has gone", () => {
    gate.onKey();
    gate.offer("/a");
    focused = false;
    gate.recheck();
    expect(applied).toEqual(["/a"]);
  });

  it("cancel drops a held follow (the user navigated themselves)", () => {
    gate.onKey();
    gate.offer("/a");
    gate.cancel();
    advance(QUIET_MS * 2);
    expect(applied).toEqual([]);
  });
});
