import { describe, expect, it } from "vitest";
import { detectBlockingScreen, dropEchoRows } from "./blockingScreen";

const claudeTrust = [
  "╭──────────────────────────────────────────────╮",
  "│ Do you trust the files in this folder?       │",
  "│                                              │",
  "│ C:\Users\me\.ymux-worktrees\repo\fanout-1    │",
  "│ ❯ 1. Yes, proceed                            │",
  "│   2. No, exit                                │",
  "╰──────────────────────────────────────────────╯",
];

describe("detectBlockingScreen", () => {
  it("matches Claude Code's folder trust dialog", () => {
    expect(detectBlockingScreen(claudeTrust)).toBe("trust");
    expect(detectBlockingScreen(["Quick safety check: Is this a project you created or one you trust?"])).toBe("trust");
  });
  it("matches Codex's trust dialog", () => {
    expect(
      detectBlockingScreen([
        "> You are running Codex in /tmp/repo",
        "  Do you trust the contents of this directory? Working with untrusted contents",
        "› 1. Yes, continue",
        "  2. No, quit",
      ]),
    ).toBe("trust");
    expect(detectBlockingScreen(["Allow Codex to work in this folder without asking for approval"])).toBe("trust");
  });
  it("is case-insensitive", () => {
    expect(detectBlockingScreen(["TRUST THIS FOLDER"])).toBe("trust");
  });
  it("matches a generic numbered select menu", () => {
    expect(detectBlockingScreen(["Pick one", "❯ 1. Light", "  2. Dark"])).toBe("select");
    expect(detectBlockingScreen(["› 2) Allow once", "  3) Deny"])).toBe("select");
  });
  it("ignores a normal prompt box", () => {
    expect(detectBlockingScreen(["╭────╮", "│ ❯ fix the failing test │", "╰────╯", "  ? for shortcuts"])).toBeNull();
    expect(detectBlockingScreen(["› Ask Codex to do anything"])).toBeNull();
    expect(detectBlockingScreen([])).toBeNull();
  });
  it("does not match the weak word trust or approve in prose", () => {
    expect(detectBlockingScreen(["I do not trust this parser; it should approve inputs", "const trusted = true;"])).toBeNull();
    expect(detectBlockingScreen(["1. first step", "2. second step"])).toBeNull();
  });
  it("only looks at the last 30 rows", () => {
    const old = [...claudeTrust, ...Array.from({ length: 30 }, () => "output")];
    expect(detectBlockingScreen(old)).toBeNull();
  });
});

describe("bordered select menus", () => {
  it("matches a boxed permission menu", () => {
    expect(
      detectBlockingScreen([
        "╭──────────────────────────────────────────────╮",
        "│ Bash command                                 │",
        "│ Do you want to proceed?                      │",
        "│ ❯ 1. Yes                                     │",
        "│   2. Yes, and don't ask again                │",
        "│   3. No, and tell Claude what to do          │",
        "╰──────────────────────────────────────────────╯",
      ]),
    ).toBe("select");
  });
  it("matches a boxed login method menu", () => {
    expect(
      detectBlockingScreen([
        "│ Select login method:                         │",
        "│ ┃ ❯ 1. Claude account with subscription      │",
        "│   2. Anthropic Console account               │",
      ]),
    ).toBe("select");
  });
  it("matches a boxed theme menu with an ASCII border", () => {
    expect(detectBlockingScreen(["| > Choose the text style", "| › 1) Dark mode", "|   2) Light mode"])).toBe("select");
  });
  it("still ignores a boxed prompt that is not a numbered option", () => {
    expect(detectBlockingScreen(["│ ❯ 1 file changed │"])).toBeNull();
    expect(detectBlockingScreen(["│ ❯ fix the 2. bug │"])).toBeNull();
  });
});

describe("echo of our own pasted prompt", () => {
  const echo = ["› 1. do x", "  2. then y", "  gpt-5 · 100% context left"];
  it("is a false menu without echo handling", () => {
    expect(detectBlockingScreen(echo)).toBe("select");
  });
  it("is ignored when the rows echo the pasted text", () => {
    expect(detectBlockingScreen(echo, "1. do x\n2. then y")).toBeNull();
  });
  it("does not hide a real dialog that merely shares the screen", () => {
    expect(detectBlockingScreen(["› 1. do x", "Do you trust the files in this folder?"], "1. do x")).toBe("trust");
    expect(detectBlockingScreen(["› 1. do x", "❯ 1. Yes", "  2. No"], "1. do x")).toBe("select");
  });
  it("matches on the first ~40 chars of the first non-empty line", () => {
    const long = "1. " + "a".repeat(80);
    expect(detectBlockingScreen(["› " + long.slice(0, 60)], long + "\nmore")).toBeNull();
    expect(dropEchoRows(["", "x"], "\n\n  1. do x  ")).toEqual(["", "x"]);
  });
  it("a too-short first line drops nothing", () => {
    expect(dropEchoRows(["❯ 1. Yes"], "1")).toEqual(["❯ 1. Yes"]);
    expect(dropEchoRows(["a"], "   ")).toEqual(["a"]);
  });
});
