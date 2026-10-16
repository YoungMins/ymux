import { describe, it, expect } from "vitest";
import {
  ATTACH_GROUPS,
  clampInt,
  defaultEndpoint,
  endpointOnProviderChange,
  formatBytes,
  groupEnabled,
  isRelativeReportDir,
  newDestination,
  recentTurns,
  setGroup,
} from "./activityModel";
import type { ActivityRecord } from "../types";

const rec = (id: string, kind: string, ts: number, data: Record<string, unknown> = {}): ActivityRecord => ({
  v: 1, id, ts, ws: 1, pane: "p", agent: "claude", account: "", session: "s", turn: "t",
  cwd: "", branch: "", kind, data,
});

describe("summarizer endpoints", () => {
  it("has a default per provider", () => {
    expect(defaultEndpoint("ollama")).toBe("http://127.0.0.1:11434");
    expect(defaultEndpoint("openai")).toBe("http://127.0.0.1:1234/v1");
    expect(defaultEndpoint("rule")).toBe("http://127.0.0.1:11434");
  });
  it("follows the provider only while the endpoint is untouched", () => {
    expect(endpointOnProviderChange("ollama", "http://127.0.0.1:11434", "openai")).toBe(
      "http://127.0.0.1:1234/v1",
    );
    expect(endpointOnProviderChange("ollama", "  ", "openai")).toBe("http://127.0.0.1:1234/v1");
    expect(endpointOnProviderChange("ollama", "http://10.0.0.5:9000", "openai")).toBe(
      "http://10.0.0.5:9000",
    );
  });
});

describe("attachment type groups", () => {
  const images = ATTACH_GROUPS.find((g) => g.id === "images")!;
  it("enables a group only when all its extensions are present", () => {
    expect(groupEnabled(["png"], images)).toBe(false);
    expect(groupEnabled(images.exts, images)).toBe(true);
  });
  it("toggles a group without touching other types or duplicating", () => {
    const on = setGroup(["pdf", "png"], images, true);
    expect(on.filter((e) => e === "png")).toHaveLength(1);
    expect(on).toContain("pdf");
    expect(groupEnabled(on, images)).toBe(true);
    const off = setGroup(on, images, false);
    expect(off).toEqual(["pdf"]);
  });
  it("never lists an extension in two groups", () => {
    const all = ATTACH_GROUPS.flatMap((g) => [...g.exts]);
    expect(new Set(all).size).toBe(all.length);
  });
});

describe("newDestination", () => {
  it("is a disabled json destination with a uuid id", () => {
    const d = newDestination("Hook 1");
    expect(d.id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(d.enabled).toBe(false);
    expect(d.format).toBe("json");
    expect(d.url_hint).toBe("");
  });
});

describe("clampInt / formatBytes", () => {
  it("clamps and falls back", () => {
    expect(clampInt("500", 1, 365, 14)).toBe(365);
    expect(clampInt("0", 1, 365, 14)).toBe(1);
    expect(clampInt("abc", 1, 365, 14)).toBe(14);
    expect(clampInt("7.9", 1, 365, 14)).toBe(7);
  });
  it("formats sizes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(3 * 1024 * 1024)).toBe("3.0 MB");
  });
});

describe("recentTurns", () => {
  it("returns the newest turn_end records first, capped", () => {
    const rs = [
      rec("a", "turn_end", 1, { summary: { headline: "one" } }),
      rec("b", "tool", 2),
      rec("c", "turn_end", 3, { summary: { headline: "three" } }),
      rec("d", "turn_end", 4),
    ];
    const out = recentTurns(rs, 2);
    expect(out.map((r) => r.id)).toEqual(["d", "c"]);
    expect(out[0].headline).toBe("");
    expect(out[1].headline).toBe("three");
  });
});

describe("isRelativeReportDir", () => {
  it("accepts repo-relative folders", () => {
    expect(isRelativeReportDir(".ymux/report")).toBe(true);
    expect(isRelativeReportDir(String.raw`out\reports`)).toBe(true);
    expect(isRelativeReportDir("")).toBe(true);
  });
  it("rejects absolute, drive, home and parent paths", () => {
    for (const bad of ["/etc", String.raw`\\srv\x`, String.raw`C:\x`, "c:x", "~/r", "../r", "a/../../b"]) {
      expect(isRelativeReportDir(bad)).toBe(false);
    }
  });
});
