import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// Guards the style.css rule that keeps macOS spaces as spaces.
//
// Keystrokes reach xterm through its hidden `.xterm-helper-textarea`, which
// xterm.css gives `white-space: nowrap`. In a whitespace-collapsing textarea
// WebKit (macOS WKWebView) turns a lone or trailing typed space into U+00A0,
// so `brew install ruby` reaches the shell as one word. `white-space: pre`
// keeps spaces literal (and still never wraps). Losing the rule is silent on
// Windows (WebView2 doesn't do this), hence a source-level check.

const cssPath = fileURLToPath(new URL("../style.css", import.meta.url));
const css = readFileSync(cssPath, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

function rulesFor(selectorPart: string): { selector: string; body: string }[] {
  const out: { selector: string; body: string }[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(css)) !== null) {
    const selector = m[1].trim();
    if (selector.includes(selectorPart)) out.push({ selector, body: m[2] });
  }
  return out;
}

describe("style.css helper textarea white-space", () => {
  it("sets white-space: pre on .xterm-helper-textarea, out-specifying xterm.css", () => {
    const rules = rulesFor(".xterm-helper-textarea").filter((r) =>
      /white-space\s*:\s*pre\s*(;|$)/.test(r.body),
    );
    expect(rules.length).toBeGreaterThan(0);
    // xterm.css uses `.xterm .xterm-helper-textarea` (two classes); ours
    // must carry more class selectors to win without !important.
    const beats = rules.some((r) =>
      r.selector.split(",").some((s) => (s.match(/\./g) ?? []).length > 2),
    );
    expect(beats).toBe(true);
  });
});
