import { describe, expect, it } from "vitest";
import {
  buildInbox,
  inboxSummary,
  nextAttention,
  ReadHistory,
  type InboxCandidate,
  type InboxItem,
  type ReadEntry,
} from "./inbox";

function c(paneId: string, status: InboxCandidate["status"], since: number): InboxCandidate {
  return { paneId, wsId: 1, wsLabel: "ws", paneLabel: paneId, agentKind: null, status, since };
}

function item(paneId: string, status: InboxItem["status"], since = 0): InboxItem {
  return { paneId, wsId: 1, wsLabel: "ws", paneLabel: paneId, agentKind: null, status, since };
}

describe("buildInbox", () => {
  it("keeps only attention and done panes", () => {
    const out = buildInbox([c("a", "idle", 1), c("b", "running", 2), c("c", "done", 3), c("d", "attention", 4)]);
    expect(out.map((i) => i.paneId)).toEqual(["d", "c"]);
  });

  it("puts attention before done, oldest first within a group", () => {
    const out = buildInbox([
      c("d1", "done", 10),
      c("a2", "attention", 30),
      c("d2", "done", 5),
      c("a1", "attention", 20),
    ]);
    expect(out.map((i) => i.paneId)).toEqual(["a1", "a2", "d2", "d1"]);
  });

  it("is stable for equal since and does not mutate input", () => {
    const input = [c("x", "done", 1), c("y", "done", 1)];
    const copy = [...input];
    expect(buildInbox(input).map((i) => i.paneId)).toEqual(["x", "y"]);
    expect(input).toEqual(copy);
  });

  it("returns an empty list for no candidates", () => {
    expect(buildInbox([])).toEqual([]);
  });
});

describe("nextAttention", () => {
  it("returns null when empty", () => {
    expect(nextAttention([], "a")).toBeNull();
    expect(nextAttention([], null)).toBeNull();
  });

  it("returns the first item when current is not listed", () => {
    expect(nextAttention([item("a", "attention"), item("b", "done")], "zzz")).toBe("a");
    expect(nextAttention([item("a", "attention")], null)).toBe("a");
  });

  it("returns the next item after current, wrapping", () => {
    const items = [item("a", "attention"), item("b", "attention"), item("c", "done")];
    expect(nextAttention(items, "a")).toBe("b");
    expect(nextAttention(items, "c")).toBe("a");
  });

  it("prefers attention items over done ones", () => {
    const items = [item("a", "attention"), item("b", "attention"), item("c", "done")];
    expect(nextAttention(items, "b")).toBe("a");
    const mixed = [item("a", "attention"), item("c", "done")];
    expect(nextAttention(mixed, "c")).toBe("a");
    // current is attention, only other attention is behind it: wraps over done
    expect(nextAttention([item("a", "attention"), item("c", "done"), item("b", "attention")], "b")).toBe("a");
  });

  it("falls back to done when no other attention exists", () => {
    expect(nextAttention([item("a", "attention"), item("c", "done")], "a")).toBe("c");
  });

  it("returns the current pane when it is the only item", () => {
    expect(nextAttention([item("a", "done")], "a")).toBe("a");
  });
});

function entry(paneId: string, status: ReadEntry["status"], readAt: number): ReadEntry {
  return { paneId, readAt, status, agentKind: null };
}

describe("ReadHistory", () => {
  it("lists newest first", () => {
    const h = new ReadHistory();
    h.push(entry("a", "done", 1));
    h.push(entry("b", "attention", 2));
    expect(h.list().map((e) => e.paneId)).toEqual(["b", "a"]);
    expect(h.list()[0].readAt).toBe(2);
  });

  it("dedups by paneId, latest wins", () => {
    const h = new ReadHistory();
    h.push(entry("a", "done", 1));
    h.push(entry("b", "done", 2));
    h.push(entry("a", "attention", 3));
    const l = h.list();
    expect(l.map((e) => e.paneId)).toEqual(["a", "b"]);
    expect(l[0].status).toBe("attention");
    expect(l[0].readAt).toBe(3);
  });

  it("caps at 20, dropping the oldest", () => {
    const h = new ReadHistory();
    for (let i = 0; i < 25; i++) h.push(entry(`p${i}`, "done", i));
    const l = h.list();
    expect(l).toHaveLength(20);
    expect(l[0].paneId).toBe("p24");
    expect(l[19].paneId).toBe("p5");
  });

  it("list returns a copy", () => {
    const h = new ReadHistory();
    h.push(entry("a", "done", 1));
    h.list().pop();
    expect(h.list()).toHaveLength(1);
  });

  it("remove drops a pane", () => {
    const h = new ReadHistory();
    h.push(entry("a", "done", 1));
    h.remove("a");
    expect(h.list()).toEqual([]);
  });
});

describe("inboxSummary", () => {
  const text = { none: "Inbox", summary: "Inbox: {attention} need attention, {done} done" };
  it("is the plain title when empty", () => {
    expect(inboxSummary({ attention: 0, done: 0 }, text)).toBe("Inbox");
  });
  it("includes both counts", () => {
    expect(inboxSummary({ attention: 2, done: 3 }, text)).toBe("Inbox: 2 need attention, 3 done");
  });
});
