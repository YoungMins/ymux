import { describe, expect, it } from "vitest";
import { ReadyGate, deliveryCheck, enterDecision, plainWriteAllowed, shellReady } from "./readyGate";

describe("ReadyGate", () => {
  const mk = () => new ReadyGate({ start: 0 });
  it("waits until lead, bracketed, a fresh snapshot and quiet", () => {
    const g = mk();
    g.noteOutput(100);
    expect(g.poll(5000)).toBe("wait");
    g.setLead(true);
    expect(g.poll(5000)).toBe("wait");
    g.setBracketed(true);
    expect(g.poll(5000)).toBe("wait");
    g.noteSnapshot(150);
    expect(g.poll(2599)).toBe("wait");
    expect(g.poll(2600)).toBe("ready");
  });
  it("output resets the quiet window and invalidates older snapshots", () => {
    const g = mk();
    g.setLead(true);
    g.setBracketed(true);
    g.noteSnapshot(10);
    g.noteOutput(2000);
    // quiet again by 4500, but the only snapshot predates the output
    expect(g.poll(4500)).toBe("wait");
    g.noteSnapshot(4600);
    expect(g.poll(4700)).toBe("ready");
  });
  it("a snapshot must be strictly after the last output", () => {
    const g = mk();
    g.setLead(true);
    g.setBracketed(true);
    g.noteOutput(1000);
    g.noteSnapshot(1000);
    expect(g.poll(9000)).toBe("wait");
    g.noteSnapshot(1001);
    expect(g.poll(9001)).toBe("ready");
  });
  it("stale lead: agent exited, scan has not noticed yet, never ready inside 2.5 s", () => {
    const g = mk();
    g.setLead(true);
    g.setBracketed(true);
    g.noteSnapshot(1000); // scan still lists the lead
    g.noteOutput(1100); // the shell printed after the agent died
    expect(g.poll(2400)).toBe("wait");
    expect(g.poll(3599)).toBe("wait");
    // the next scan (<= 2 s later) drops the lead before quiet elapses
    g.setLead(false);
    g.noteSnapshot(3000);
    expect(g.poll(3700)).toBe("wait");
  });
  it("needs each of lead and bracketed", () => {
    const g = mk();
    g.setLead(true);
    g.noteSnapshot(1);
    expect(g.poll(10000)).toBe("wait");
    g.setLead(false);
    g.setBracketed(true);
    expect(g.poll(10000)).toBe("wait");
  });
  it("times out after timeoutMs from start", () => {
    const g = new ReadyGate({ start: 1000 });
    expect(g.poll(90999)).toBe("wait");
    expect(g.poll(91000)).toBe("timeout");
  });
  it("ready beats timeout when both hold", () => {
    const g = mk();
    g.setLead(true);
    g.setBracketed(true);
    g.noteSnapshot(5);
    expect(g.poll(95000)).toBe("ready");
  });
  it("with no output, quiet counts from start", () => {
    const g = mk();
    g.setLead(true);
    g.setBracketed(true);
    g.noteSnapshot(1);
    expect(g.poll(2499)).toBe("wait");
    expect(g.poll(2500)).toBe("ready");
  });
  it("honours custom options", () => {
    const g = new ReadyGate({ start: 0, quietMs: 10, timeoutMs: 50 });
    expect(g.poll(50)).toBe("timeout");
  });
});

describe("ReadyGate blocked", () => {
  const ready = () => {
    const g = new ReadyGate({ start: 0 });
    g.setLead(true);
    g.setBracketed(true);
    g.noteSnapshot(1);
    return g;
  };
  it("is blocked, never ready, while a dialog shows", () => {
    const g = ready();
    g.setBlocked(true);
    expect(g.poll(5000)).toBe("blocked");
  });
  it("does not time out while blocked, and gets the full window after", () => {
    const g = ready();
    g.setBlocked(true);
    expect(g.poll(60000)).toBe("blocked");
    expect(g.poll(120000)).toBe("blocked");
    g.setBlocked(false);
    g.noteSnapshot(120050);
    expect(g.poll(120100)).toBe("wait");
    expect(g.poll(122600)).toBe("ready");
  });
  it("restarts the quiet window when the dialog clears", () => {
    const g = ready();
    g.setBlocked(true);
    g.poll(10000);
    g.setBlocked(false);
    g.noteSnapshot(10100);
    expect(g.poll(10500)).toBe("wait");
    expect(g.poll(12500)).toBe("ready");
  });
  it("after the dialog clears, an older snapshot is not enough", () => {
    const g = ready();
    g.setBlocked(true);
    g.poll(10000);
    g.setBlocked(false);
    expect(g.poll(20000)).toBe("wait");
  });
  it("times out on unblocked time only", () => {
    const g = new ReadyGate({ start: 0, timeoutMs: 1000 });
    g.poll(100);
    g.setBlocked(true);
    g.poll(5000);
    g.setBlocked(false);
    expect(g.poll(5500)).toBe("wait");
    expect(g.poll(6000)).toBe("timeout");
  });
  it("gives up on a dialog left up past the cap", () => {
    const g = new ReadyGate({ start: 0, blockedCapMs: 30 * 60 * 1000 });
    g.setBlocked(true);
    expect(g.poll(1000)).toBe("blocked");
    expect(g.poll(1000 + 30 * 60 * 1000 - 1)).toBe("blocked");
    expect(g.poll(1000 + 30 * 60 * 1000)).toBe("timeout");
  });
  it("the cap restarts after the dialog clears and returns", () => {
    const g = new ReadyGate({ start: 0, blockedCapMs: 1000 });
    g.setBlocked(true);
    g.poll(100);
    g.setBlocked(false);
    g.poll(900);
    g.setBlocked(true);
    expect(g.poll(1500)).toBe("blocked");
    expect(g.poll(2500)).toBe("timeout");
  });
  it("defaults to a 30 minute cap", () => {
    const g = new ReadyGate({ start: 0 });
    g.setBlocked(true);
    g.poll(0);
    expect(g.poll(29 * 60 * 1000)).toBe("blocked");
    expect(g.poll(30 * 60 * 1000)).toBe("timeout");
  });
});

describe("ReadyGate sampling time", () => {
  it("a heartbeat sampled before the last output never makes it ready", () => {
    const g = new ReadyGate({ start: 0 });
    g.setLead(true);
    g.setBracketed(true);
    g.noteOutput(5000);
    g.noteSnapshot(4900); // arrived at ~6000, but sampled before the output
    expect(g.poll(8000)).toBe("wait");
    g.noteSnapshot(5100);
    expect(g.poll(8000)).toBe("ready");
  });
});

describe("enterDecision", () => {
  it("aborts when the target is no longer fit, even with a snapshot", () => {
    expect(enterDecision({ now: 5, pastedAt: 1, snapshotAt: 4, fit: false })).toBe("abort");
  });
  it("waits for a snapshot newer than the paste", () => {
    expect(enterDecision({ now: 400, pastedAt: 100, snapshotAt: 100, fit: true })).toBe("wait");
    expect(enterDecision({ now: 400, pastedAt: 100, snapshotAt: 50, fit: true })).toBe("wait");
  });
  it("sends once a fresh snapshot shows the lead still fit", () => {
    expect(enterDecision({ now: 400, pastedAt: 100, snapshotAt: 101, fit: true })).toBe("send");
  });
  it("does not send on a heartbeat that arrived after the paste but was sampled before it", () => {
    // Sampling time (not arrival time) is what the manager stores.
    expect(enterDecision({ now: 450, pastedAt: 400, snapshotAt: 399, fit: true })).toBe("wait");
    expect(enterDecision({ now: 450, pastedAt: 400, snapshotAt: 400, fit: true })).toBe("wait");
  });
  it("gives up when no snapshot arrives in time", () => {
    expect(enterDecision({ now: 6099, pastedAt: 100, snapshotAt: 0, fit: true })).toBe("wait");
    expect(enterDecision({ now: 6100, pastedAt: 100, snapshotAt: 0, fit: true })).toBe("abort");
    expect(enterDecision({ now: 600, pastedAt: 100, snapshotAt: 0, fit: true, maxWaitMs: 500 })).toBe("abort");
  });
});

describe("shellReady", () => {
  const base = { startedAt: 0, firstOutputAt: null, lastOutputAt: 0, cwdSeen: false };
  it("is ready as soon as the shell reports its cwd", () => {
    expect(shellReady({ ...base, now: 10, cwdSeen: true })).toBe("ready");
  });
  it("is ready after first output then 500 ms of quiet", () => {
    const o = { ...base, firstOutputAt: 100, lastOutputAt: 300 };
    expect(shellReady({ ...o, now: 799 })).toBe("wait");
    expect(shellReady({ ...o, now: 800 })).toBe("ready");
  });
  it("silence alone is not a prompt", () => {
    expect(shellReady({ ...base, now: 9999 })).toBe("wait");
  });
  it("caps at 10 s", () => {
    expect(shellReady({ ...base, now: 10000 })).toBe("cap");
    expect(shellReady({ ...base, now: 3000, capMs: 2000 })).toBe("cap");
  });
});

describe("deliveryCheck", () => {
  it("confirmed when prompt submit seen at or after submit", () => {
    expect(deliveryCheck(1000, 1500, 1600)).toBe("confirmed");
    expect(deliveryCheck(1000, 1000, 1000)).toBe("confirmed");
  });
  it("ignores a submit seen before ours", () => {
    expect(deliveryCheck(1000, 900, 1500)).toBe("pending");
  });
  it("pending inside window, unconfirmed after", () => {
    expect(deliveryCheck(1000, null, 10999)).toBe("pending");
    expect(deliveryCheck(1000, null, 11000)).toBe("unconfirmed");
    expect(deliveryCheck(1000, null, 2000, 500)).toBe("unconfirmed");
  });
});

describe("plainWriteAllowed", () => {
  const ok = {
    text: "git merge --no-ff feat/x-1.2",
    shellExecutable: "C:\\Windows\\System32\\cmd.exe",
    lastCwdAt: 1000,
    lastSubmitAt: 500,
    lastOutputAt: 1100,
  };
  it("allows the unquoted merge command at a fresh prompt", () => {
    expect(plainWriteAllowed(ok)).toBe(true);
    expect(plainWriteAllowed({ ...ok, shellExecutable: "/bin/zsh" })).toBe(true);
  });
  it("refuses anything but the exact unquoted form", () => {
    for (const text of [
      "git merge --no-ff 'a b'",
      "git merge --no-ff a\r",
      "git merge --no-ff a;ls",
      "git merge --no-ff -x",
      "git merge --no-ff ",
      "ls",
    ]) {
      expect(plainWriteAllowed({ ...ok, text })).toBe(false);
    }
  });
  it("refuses when the prompt is not provably back", () => {
    expect(plainWriteAllowed({ ...ok, lastSubmitAt: 1000 })).toBe(false);
    expect(plainWriteAllowed({ ...ok, lastSubmitAt: 1200 })).toBe(false);
    expect(plainWriteAllowed({ ...ok, lastCwdAt: 0, lastSubmitAt: 0 })).toBe(false);
    expect(plainWriteAllowed({ ...ok, lastOutputAt: 1250 })).toBe(false);
    expect(plainWriteAllowed({ ...ok, lastOutputAt: 1249 })).toBe(true);
  });
  it("refuses shells without an OSC 7 prompt hook", () => {
    for (const shellExecutable of ["/bin/sh", "wsl.exe", "nu", "", null, undefined]) {
      expect(plainWriteAllowed({ ...ok, shellExecutable })).toBe(false);
    }
  });
});
