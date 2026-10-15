import { describe, expect, it } from "vitest";
import { composePrompt, PROMPT_WARN } from "./reviewPrompt";
import type { ReviewComment } from "./model-types";

const c = (over: Partial<ReviewComment> = {}): ReviewComment => ({
  id: "1", path: "src/a.ts", side: "new", line: 42, text: "rename this", lineText: "const x = 1;", ...over,
});

describe("composePrompt", () => {
  it("numbers entries with path:L and the comment", () => {
    const out = composePrompt("/repo", "main", [c(), c({ id: "2", path: "b.ts", line: 7, text: "why?" })]);
    expect(out).toContain("/repo");
    expect(out).toContain("main");
    expect(out).toContain("1. src/a.ts:L42");
    expect(out).toContain("2. b.ts:L7");
    expect(out).toContain("rename this");
    expect(out).toContain("const x = 1;");
  });

  it("shows a range", () => {
    expect(composePrompt("/r", "HEAD", [c({ endLine: 45 })])).toContain("src/a.ts:L42-45");
  });

  it("labels deleted lines", () => {
    expect(composePrompt("/r", "HEAD", [c({ side: "old", line: 12 })])).toContain("src/a.ts old L12 (deleted)");
  });

  it("fences longer than any backtick run in the excerpt", () => {
    const out = composePrompt("/r", "HEAD", [c({ lineText: "x ````` y" })]);
    expect(out).toContain("``````\nx ````` y\n``````");
  });

  it("uses a 3-backtick fence by default", () => {
    expect(composePrompt("/r", "HEAD", [c()])).toContain("```\nconst x = 1;\n```");
  });

  it("strips C0 controls except tab and newline", () => {
    const out = composePrompt("/r", "HEAD", [c({ text: "a\x1b[31mb\x00\x07\tc\nd\r", lineText: "\x1bz" })]);
    expect(out).not.toMatch(/[\x00-\x08\x0b-\x1f\x7f]/);
    expect(out).toContain("a[31mb\tc\nd");
  });

  it("strips C1 controls and line separators everywhere", () => {
    const out = composePrompt("/r\x9b", "b\x85", [
      c({ path: "a\x9b.ts", text: "x\x9b31my\u2028z", lineText: "q\x80", excerpt: [" \x9d"] }),
    ]);
    expect(out).not.toMatch(/[\x80-\x9f\u2028\u2029]/);
  });

  it("renders newlines and tabs in paths, root and base visibly", () => {
    const out = composePrompt("/r\nINJECT root", "b\r\nINJECT base", [
      c({ path: "a\nINJECT path\t.ts" }),
      c({ id: "2", path: "d\nINJECT old", side: "old" }),
    ]);
    expect(out).not.toMatch(/^INJECT/m);
    expect(out).toContain("/r\\nINJECT root");
    expect(out).toContain("b\\r\\nINJECT base");
    expect(out).toContain("1. a\\nINJECT path\\t.ts:L42");
    expect(out).toContain("d\\nINJECT old old L42 (deleted)");
  });

  it("flags stale comments", () => {
    expect(composePrompt("/r", "HEAD", [c({ stale: true })])).toMatch(/moved|stale/i);
  });

  it("exposes a 32 KiB warning threshold", () => {
    expect(PROMPT_WARN).toBe(32 * 1024);
  });

  it("prints the multi-line excerpt with prefixes when present", () => {
    const out = composePrompt("/r", "HEAD", [c({ excerpt: [" a", "-b", "+c"] })]);
    expect(out).toContain("```\n a\n-b\n+c\n```");
  });
});
