import { describe, expect, it } from "vitest";
import { phaseOf, PromptQueues, queueSuffix, REARM_MS, RETRY_MS, MAX_QUEUED, TYPING_QUIET_MS } from "./promptQueue";

const no = () => false;

describe("phaseOf", () => {
  const hook = (status: "working" | "waiting" | "done" | "idle") => ({ status, source: "hook" as const });
  const proc = { status: "idle" as const, source: "process" as const };

  it("maps hook-fed agent statuses and ignores the pane status", () => {
    expect(phaseOf(null, "idle")).toBe("none");
    expect(phaseOf(undefined, "idle")).toBe("none");
    expect(phaseOf(hook("working"), "idle")).toBe("working");
    expect(phaseOf(hook("waiting"), "idle")).toBe("waiting");
    expect(phaseOf(hook("done"), "running")).toBe("ready");
    expect(phaseOf(hook("idle"), "running")).toBe("ready");
  });

  it("reads a process-scan lead from the pane status, not its permanent idle", () => {
    expect(phaseOf(proc, "running")).toBe("working");
    expect(phaseOf(proc, "attention")).toBe("waiting");
    expect(phaseOf(proc, "done")).toBe("ready");
    expect(phaseOf(proc, "idle")).toBe("ready");
  });
});

describe("PromptQueues", () => {
  it("is FIFO and ignores blank prompts", () => {
    const q = new PromptQueues();
    expect(q.add("a", "  \n ")).toBe(-1);
    expect(q.add("a", "one\n")).toBe(1);
    expect(q.add("a", "two")).toBe(2);
    expect(q.list("a")).toEqual(["one", "two"]);
    expect(q.count("b")).toBe(0);
  });

  it("caps the queue", () => {
    const q = new PromptQueues();
    for (let i = 0; i < MAX_QUEUED; i++) q.add("a", `p${i}`);
    expect(q.add("a", "more")).toBe(-1);
  });

  it("delivers when idle at once, one prompt only", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    q.add("a", "two");
    expect(q.claim("a", "ready", no, 0)).toBe("one");
    // A second event while the first is in flight (double Stop) delivers nothing.
    expect(q.claim("a", "ready", no, 1)).toBeNull();
    q.settle("a", "delivered", 2);
    // Still ready, agent not seen working since: the same turn end, no second prompt.
    expect(q.claim("a", "ready", no, 3)).toBeNull();
    expect(q.claim("a", "ready", no, 4)).toBeNull();
    expect(q.list("a")).toEqual(["two"]);
  });

  it("waits for the next turn end after seeing the agent work", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    q.add("a", "two");
    q.claim("a", "ready", no, 0);
    q.settle("a", "delivered", 1);
    expect(q.claim("a", "working", no, 2)).toBeNull();
    expect(q.claim("a", "ready", no, 3)).toBe("two");
  });

  it("re-arms after REARM_MS when no work was ever seen", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    q.add("a", "two");
    q.claim("a", "ready", no, 0);
    q.settle("a", "delivered", 0);
    expect(q.claim("a", "ready", no, REARM_MS - 1)).toBeNull();
    expect(q.claim("a", "ready", no, REARM_MS)).toBe("two");
  });

  it("never delivers while working, waiting for input, or without an agent", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    for (const phase of ["working", "waiting", "none"] as const) {
      expect(q.claim("a", phase, no, 0)).toBeNull();
    }
    expect(q.count("a")).toBe(1);
  });

  it("never delivers into a dialog and only asks the screen when it matters", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    let asked = 0;
    expect(q.claim("a", "working", () => (asked++, false), 0)).toBeNull();
    expect(asked).toBe(0);
    expect(q.claim("a", "ready", () => (asked++, true), 0)).toBeNull();
    expect(asked).toBe(1);
    expect(q.claim("a", "ready", no, 0)).toBe("one");
  });

  it("keeps a refused prompt and retries after RETRY_MS", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    q.claim("a", "ready", no, 0);
    q.settle("a", "retry", 100);
    expect(q.claim("a", "ready", no, 100 + RETRY_MS - 1)).toBeNull();
    expect(q.claim("a", "ready", no, 100 + RETRY_MS)).toBe("one");
  });

  it("empty queue claims nothing", () => {
    const q = new PromptQueues();
    expect(q.claim("a", "ready", no, 0)).toBeNull();
  });

  it("removes and clears, protecting a prompt mid-delivery", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    q.add("a", "two");
    q.add("a", "three");
    expect(q.remove("a", 1)).toBe(true);
    expect(q.list("a")).toEqual(["one", "three"]);
    expect(q.remove("a", 9)).toBe(false);
    q.claim("a", "ready", no, 0);
    expect(q.remove("a", 0)).toBe(false);
    q.clear("a");
    expect(q.list("a")).toEqual(["one"]);
    q.settle("a", "delivered", 1);
    expect(q.count("a")).toBe(0);
  });

  it("forgetting a pane with items queued drops them and ignores a late settle", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    q.add("a", "two");
    q.claim("a", "ready", no, 0);
    q.forget("a");
    q.settle("a", "delivered", 1);
    expect(q.count("a")).toBe(0);
    expect(q.queuedPanes()).toEqual([]);
    expect(q.claim("a", "ready", no, 2)).toBeNull();
  });

  it("lists only panes with prompts", () => {
    const q = new PromptQueues();
    q.add("a", "x");
    q.add("b", "y");
    q.remove("b", 0);
    expect(q.queuedPanes()).toEqual(["a"]);
  });
});

describe("process-source leads", () => {
  it("never re-arms while the pane status says working", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    q.add("a", "two");
    const proc = { status: "idle" as const, source: "process" as const };
    expect(q.claim("a", phaseOf(proc, "done"), no, 0)).toBe("one");
    q.settle("a", "delivered", 0);
    // The pane runs the turn: far beyond REARM_MS nothing goes out.
    expect(q.claim("a", phaseOf(proc, "running"), no, REARM_MS * 5)).toBeNull();
    // Turn over, pane done again: the work was seen, so the next one goes.
    expect(q.claim("a", phaseOf(proc, "done"), no, REARM_MS * 5 + 1)).toBe("two");
  });

  it("does not deliver mid-turn on a lead whose raw status is idle", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    const proc = { status: "idle" as const, source: "process" as const };
    expect(q.claim("a", phaseOf(proc, "running"), no, 0)).toBeNull();
    expect(q.claim("a", phaseOf(proc, "attention"), no, 0)).toBeNull();
  });
});

describe("the user's own input", () => {
  it("holds delivery while the user types in the focused pane", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    const typed = 1000;
    expect(q.claim("a", "ready", no, typed + 1, { focused: true, inputAt: typed })).toBeNull();
    expect(q.claim("a", "ready", no, typed + TYPING_QUIET_MS, { focused: true, inputAt: typed })).toBe("one");
  });

  it("ignores typing in a pane that is not focused", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    expect(q.claim("a", "ready", no, 1001, { focused: false, inputAt: 1000 })).toBe("one");
  });

  it("does not paste on top of an unsent paste until the user acts or the agent works", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    q.add("a", "two");
    q.add("a", "three");
    expect(q.claim("a", "ready", no, 1000)).toBe("one");
    q.settle("a", "delivered", 1000, false);
    // Well past the re-arm time, still ready: the text still sits in the box.
    expect(q.claim("a", "ready", no, 1000 + REARM_MS * 3)).toBeNull();
    // The user typed in the pane after the paste: theirs now.
    expect(q.claim("a", "ready", no, 1000 + REARM_MS * 3 + 1, { focused: false, inputAt: 1500 })).toBe("two");
    q.settle("a", "delivered", 1000 + REARM_MS * 3 + 2, false);
    // Seeing the agent work also clears it.
    expect(q.claim("a", "working", no, 1000 + REARM_MS * 4)).toBeNull();
    expect(q.claim("a", "ready", no, 1000 + REARM_MS * 4 + 1)).toBe("three");
  });

  it("an unsent paste is forgotten when the agent is gone", () => {
    const q = new PromptQueues();
    q.add("a", "one");
    q.add("a", "two");
    q.claim("a", "ready", no, 1000);
    q.settle("a", "delivered", 1000, false);
    q.claim("a", "none", no, 2000);
    expect(q.claim("a", "ready", no, 1000 + REARM_MS)).toBe("two");
  });
});

describe("queueSuffix", () => {
  it("is empty for none", () => {
    expect(queueSuffix(0)).toBe("");
    expect(queueSuffix(3)).toBe(" ▸3");
  });
});
