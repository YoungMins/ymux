import { describe, expect, it } from "vitest";
import { BroadcastSwitch, applyZoom, broadcastParticipants, broadcastTargets, mirrorableInput, type BroadcastPane } from "./broadcast";

const panes: BroadcastPane[] = [
  { id: "t1", terminal: true, shown: true },
  { id: "t2", terminal: true, shown: true },
  { id: "t3", terminal: true, shown: false }, // hidden tab
  { id: "ed", terminal: false, shown: true },
  { id: "git", terminal: false, shown: true },
];

describe("broadcastTargets", () => {
  it("is every other shown terminal, never editors or hidden tabs", () => {
    expect(broadcastTargets("t1", panes)).toEqual(["t2"]);
    expect(broadcastTargets("t2", panes)).toEqual(["t1"]);
  });

  it("is empty when the source is not a participating terminal", () => {
    expect(broadcastTargets("ed", panes)).toEqual([]);
    expect(broadcastTargets("t3", panes)).toEqual([]);
    expect(broadcastTargets("nope", panes)).toEqual([]);
  });

  it("excludes the source itself (no echo loop)", () => {
    expect(broadcastTargets("t1", panes)).not.toContain("t1");
  });

  it("lists participants including the source", () => {
    expect(broadcastParticipants(panes)).toEqual(["t1", "t2"]);
  });
});

describe("mirrorableInput", () => {
  it("passes typing and control keys", () => {
    for (const s of ["a", "ls -la", "\r", "\x03", "\x7f", "\x1b[A", "\x1b[3~", "\x1bb", "한"]) {
      expect(mirrorableInput(s)).toBe(true);
    }
  });

  it("drops terminal auto-replies", () => {
    for (const s of [
      "",
      "\x1b[I",
      "\x1b[O",
      "\x1b[12;40R",
      "\x1b[0n",
      "\x1b[?62;4c",
      "\x1b[>0;276;0c",
      "\x1b[?2004;2$y",
      "\x1b[<0;10;5M",
      "\x1b[<0;10;5m",
      "\x1b]11;rgb:0000/0000/0000\x07",
    ]) {
      expect(mirrorableInput(s)).toBe(false);
    }
  });
});

describe("BroadcastSwitch", () => {
  it("is off by default, per workspace", () => {
    const s = new BroadcastSwitch();
    expect(s.isOn(1)).toBe(false);
    expect(s.toggle(1)).toBe(true);
    expect(s.isOn(1)).toBe(true);
    expect(s.isOn(2)).toBe(false);
    expect(s.toggle(1)).toBe(false);
    s.toggle(2);
    s.off(2);
    expect(s.isOn(2)).toBe(false);
  });
});

describe("applyZoom", () => {
  it("leaves panes alone when nothing is zoomed", () => {
    expect(applyZoom(panes, null)).toEqual(panes);
  });

  it("lets only the zoomed group's shown tab take part", () => {
    const out = applyZoom(panes, new Set(["t1", "t3"]));
    expect(broadcastParticipants(out)).toEqual(["t1"]);
    expect(broadcastTargets("t1", out)).toEqual([]);
    expect(broadcastTargets("t2", out)).toEqual([]);
  });
});
