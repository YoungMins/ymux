import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { MONO_FONT_STACK } from "./fonts";

const css = readFileSync(fileURLToPath(new URL("../style.css", import.meta.url)), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  "",
);

describe("monospace font stack", () => {
  it("puts a macOS font before the Courier fallbacks, Windows fonts first", () => {
    const fonts = MONO_FONT_STACK.split(",").map((f) => f.trim().replace(/"/g, ""));
    const idx = (f: string) => fonts.indexOf(f);
    expect(idx("Cascadia Code")).toBe(0);
    expect(idx("Menlo")).toBeGreaterThan(idx("Consolas"));
    expect(idx("SF Mono")).toBeGreaterThan(idx("Consolas"));
    expect(idx("Menlo")).toBeLessThan(idx("Courier New"));
    expect(idx("SF Mono")).toBeLessThan(idx("Courier New"));
    expect(fonts[fonts.length - 1]).toBe("monospace");
  });

  it("style.css defines --font-mono as exactly the same stack", () => {
    const m = css.match(/--font-mono:\s*([^;]+);/);
    expect(m?.[1].trim()).toBe(MONO_FONT_STACK);
  });

  it("style.css has no hard-coded monospace stack left", () => {
    const decls = [...css.matchAll(/font-family:\s*([^;]+);/g)].map((d) => d[1]);
    for (const d of decls) {
      if (/monospace|Consolas|Cascadia|Courier/.test(d)) expect(d.trim()).toBe("var(--font-mono)");
    }
  });
});
