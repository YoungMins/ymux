import { describe, expect, it } from "vitest";
import { tags } from "@lezer/highlight";
import { DEFAULT_SYNTAX, chromeSpec, safeHex, syntaxRules } from "./theme";
import type { SyntaxColors } from "../settings/types";

const custom: SyntaxColors = {
  keyword: "#111111",
  string: "#222222",
  comment: "#333333",
  number: "#444444",
  function: "#555555",
  type_name: "#666666",
  variable: "#777777",
  punctuation: "#888888",
};

function colorOf(rules: ReturnType<typeof syntaxRules>, tag: unknown): string | undefined {
  return rules.find((r) => (Array.isArray(r.tag) ? r.tag.includes(tag) : r.tag === tag))?.color;
}

describe("syntaxRules", () => {
  const rules = syntaxRules(custom);

  it("puts each of the 8 ytheme colours on its tags (spec §3.2)", () => {
    expect(colorOf(rules, tags.keyword)).toBe("#111111");
    expect(colorOf(rules, tags.controlKeyword)).toBe("#111111");
    expect(colorOf(rules, tags.string)).toBe("#222222");
    expect(colorOf(rules, tags.comment)).toBe("#333333");
    expect(colorOf(rules, tags.number)).toBe("#444444");
    expect(colorOf(rules, tags.bool)).toBe("#444444");
    expect(colorOf(rules, tags.typeName)).toBe("#666666");
    expect(colorOf(rules, tags.className)).toBe("#666666");
    expect(colorOf(rules, tags.variableName)).toBe("#777777");
    expect(colorOf(rules, tags.propertyName)).toBe("#777777");
    expect(colorOf(rules, tags.operator)).toBe("#888888");
    expect(colorOf(rules, tags.bracket)).toBe("#888888");
  });

  it("covers every SyntaxColors field exactly once", () => {
    expect(rules.map((r) => r.field).sort()).toEqual(Object.keys(custom).sort());
  });

  it("falls back per field on a malformed or missing colour instead of throwing", () => {
    const broken = syntaxRules({ ...custom, keyword: "purple", string: "#12345", comment: undefined });
    expect(colorOf(broken, tags.keyword)).toBe(DEFAULT_SYNTAX.keyword);
    expect(colorOf(broken, tags.string)).toBe(DEFAULT_SYNTAX.string);
    expect(colorOf(broken, tags.comment)).toBe(DEFAULT_SYNTAX.comment);
    expect(colorOf(broken, tags.number)).toBe("#444444");
  });

  it("uses the defaults when no theme loaded", () => {
    expect(colorOf(syntaxRules(null), tags.keyword)).toBe(DEFAULT_SYNTAX.keyword);
  });
});

describe("safeHex", () => {
  it("accepts #rrggbb only", () => {
    expect(safeHex("#A1b2C3", "#000000")).toBe("#A1b2C3");
    expect(safeHex(" #a1b2c3 ", "#000000")).toBe("#a1b2c3");
    expect(safeHex("#abc", "#000000")).toBe("#000000");
    expect(safeHex(7, "#000000")).toBe("#000000");
  });
});

describe("chromeSpec", () => {
  it("draws from the app's CSS tokens, not CodeMirror's defaults", () => {
    const spec = chromeSpec(14);
    expect(spec["&"].backgroundColor).toBe("var(--bg)");
    expect(spec["&"].fontSize).toBe("14px");
    expect(spec[".cm-gutters"].color).toBe("var(--fg-muted)");
  });
});
