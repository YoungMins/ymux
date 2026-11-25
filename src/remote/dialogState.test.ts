import { describe, it, expect } from "vitest";
import { DialogDebounce, screenShowsDialog } from "./dialogState";

const MENU = [
  "Do you want to proceed?",
  "❯ 1. Yes",
  "  2. Yes, and don't ask again",
  "  3. No",
];

describe("screenShowsDialog", () => {
  it("accepts a real menu: pointer row plus more numbered rows", () => {
    expect(screenShowsDialog(MENU)).toBe(true);
    expect(screenShowsDialog(["│ ❯ 1. Allow", "│   2. Deny"])).toBe(true);
  });

  it("ignores the user's own numbered prompt echoed back", () => {
    expect(screenShowsDialog(["› 1. do x", "  ... working"])).toBe(false);
    expect(screenShowsDialog(["❯ 1. fix the bug"])).toBe(false);
    expect(screenShowsDialog(["output", "› 1. do x", "  2. then y"], "1. do x\n2. then y")).toBe(false);
  });

  it("ignores a numbered list without a pointer", () => {
    expect(screenShowsDialog(["1. one", "2. two", "3. three"])).toBe(false);
  });

  it("finds a known dialog only in the last rows", () => {
    expect(screenShowsDialog(["welcome", "Do you trust the files in this folder?"])).toBe(true);
    const old = ["Do you trust this folder?", ...Array.from({ length: 10 }, (_, i) => `line ${i}`)];
    expect(screenShowsDialog(old)).toBe(false);
    expect(screenShowsDialog(["The docs say: yes, proceed", ...Array.from({ length: 8 }, () => "x")])).toBe(false);
  });

  it("ignores a menu that scrolled far up the screen", () => {
    const rows = [...MENU, ...Array.from({ length: 25 }, (_, i) => `output ${i}`)];
    expect(screenShowsDialog(rows)).toBe(false);
  });
});

// Layouts as the agents draw them (inferred from the repo's fixtures and the
// agents' documented screens, not captured from a live run).
describe("a menu counts only while it is the last thing on the screen", () => {
  const claudeBox = [
    "● Bash(rm -rf build)",
    "╭──────────────────────────────────────────────╮",
    "│ Bash command                                 │",
    "│ Do you want to proceed?                      │",
    "│ ❯ 1. Yes                                     │",
    "│   2. Yes, and don't ask again                │",
    "│   3. No, and tell Claude what to do          │",
    "╰──────────────────────────────────────────────╯",
  ];
  const claudeAsk = [
    "Which approach do you prefer?",
    "❯ 1. Option A",
    "     the quick one",
    "  2. Option B",
    "     the careful one",
    "  3. Other",
    "  4. Chat about this",
    "",
    "  Enter to select · ↑/↓ to navigate · Esc to cancel",
  ];
  const codexApproval = [
    "Would you like to run the following command?",
    "  $ rm -rf build",
    "› 1. Yes, proceed (y)",
    "  2. Yes, and don't ask again (a)",
    "  3. No, and tell Codex what to do differently (esc)",
    "",
    "  Press enter to confirm or esc to cancel",
  ];

  it("pending menus pass: Claude permission, Claude question with hints, Codex approval", () => {
    expect(screenShowsDialog(claudeBox)).toBe(true);
    expect(screenShowsDialog(claudeAsk)).toBe(true);
    expect(screenShowsDialog(codexApproval)).toBe(true);
    expect(screenShowsDialog([...codexApproval, "  gpt-5 · 100% context left"])).toBe(true);
  });

  it("the same menus followed by the agent's output do not", () => {
    expect(screenShowsDialog([...claudeBox, "", "● Ran rm -rf build", "  ⎿ done"])).toBe(false);
    expect(screenShowsDialog([...claudeAsk, "", "● Going with option A."])).toBe(false);
    expect(screenShowsDialog([...codexApproval, "", "• Ran rm -rf build"])).toBe(false);
    expect(screenShowsDialog([...codexApproval, "", "• Working (2s • esc to interrupt)"])).toBe(false);
    expect(screenShowsDialog([...claudeBox, "╭────╮", "│ > │", "╰────╯", "  ? for shortcuts"])).toBe(false);
  });

  it("a numbered prompt echo followed by the working indicator or the input box does not", () => {
    // Claude Code
    expect(
      screenShowsDialog(["> 1. a", "  2. b", "", "✻ Thinking…", "╭────╮", "│ > │", "╰────╯", "  ? for shortcuts"]),
    ).toBe(false);
    // Codex
    expect(
      screenShowsDialog(["› 1. a", "  2. b", "", "• Working (0s • esc to interrupt)", "", "› Ask Codex to do anything", "  gpt-5 · 100% context left"]),
    ).toBe(false);
    expect(screenShowsDialog(["› 1. a", "  2. b", "", "• I'll start with a.", "  gpt-5 · 100% context left"])).toBe(false);
  });

  it("Claude's own '>' message echo with nothing after it yet is not a dialog; with a hint it is", () => {
    expect(screenShowsDialog(["> 1. a", "  2. b"])).toBe(false);
    expect(screenShowsDialog(["> 1. Yes", "  2. No", "  Esc to cancel"])).toBe(true);
  });

  it("the supplied prompt echo is still dropped first", () => {
    expect(screenShowsDialog(["› 1. a", "  2. b"], "1. a\n2. b")).toBe(false);
  });

  it("a prose row mentioning a key is not a hint", () => {
    expect(screenShowsDialog([...claudeBox, "I will return a value to the caller"])).toBe(false);
  });
});

describe("DialogDebounce", () => {
  it("believes a dialog only on the second sighting 1.5 s or more after the first", () => {
    const d = new DialogDebounce();
    d.observe("a", true, 0);
    expect(d.stable("a")).toBe(false);
    d.observe("a", true, 1000);
    expect(d.stable("a")).toBe(false);
    d.observe("a", true, 1500);
    expect(d.stable("a")).toBe(true);
  });

  it("believes it is gone only on two sightings of nothing, as far apart", () => {
    const d = new DialogDebounce();
    d.observe("a", true, 0);
    d.observe("a", true, 2000);
    expect(d.stable("a")).toBe(true);
    d.observe("a", false, 4000);
    expect(d.stable("a")).toBe(true);
    d.observe("a", false, 6000);
    expect(d.stable("a")).toBe(false);
  });

  it("is not moved by flapping input", () => {
    const d = new DialogDebounce();
    for (let t = 0; t < 20_000; t += 2000) d.observe("a", (t / 2000) % 2 === 0, t);
    expect(d.stable("a")).toBe(false);
    d.observe("a", true, 30_000);
    d.observe("a", true, 32_000);
    expect(d.stable("a")).toBe(true);
    for (let t = 34_000; t < 54_000; t += 2000) d.observe("a", (t / 2000) % 2 === 0, t);
    expect(d.stable("a")).toBe(true);
  });

  it("starts over after forget, and forgets what is no longer listed", () => {
    const d = new DialogDebounce();
    d.observe("a", true, 0);
    d.observe("a", true, 2000);
    d.observe("b", true, 0);
    d.observe("b", true, 2000);
    d.retainOnly(["b"]);
    expect(d.stable("a")).toBe(false);
    expect(d.stable("b")).toBe(true);
    d.observe("a", true, 2500);
    d.observe("a", true, 4500);
    expect(d.stable("a")).toBe(true);
    d.forget("a");
    expect(d.stable("a")).toBe(false);
    d.observe("a", true, 3000);
    expect(d.stable("a")).toBe(false);
  });
});
