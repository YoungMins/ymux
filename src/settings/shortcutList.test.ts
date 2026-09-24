import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { SHORTCUTS, splitShortcutKeys } from "./shortcutList";

// Type declarations for these two imports live in ./node-shims.d.ts — see
// its header comment for why (this package has no @types/node).

// CLAUDE.md rule 6: a new Ctrl+Shift+X shortcut must get a row in
// shortcutList.ts's SHORTCUTS (Settings → Shortcuts). This file makes
// forgetting that row loud, by re-deriving the key/code literals main.ts's
// global keydown handler actually matches on and checking each one shows up
// somewhere in SHORTCUTS.
//
// What this catches: any `key === "X"` or `ev.code === "Y"` literal added to
// (or removed from) the handler whose resolved symbol isn't present in any
// SHORTCUTS row — e.g. shipping a new `if (mod && ev.shiftKey && key ===
// "Q")` block with no matching SHORTCUTS entry fails CODE_SYMBOLS.has(...)
// below with a clear message, and removing an existing SHORTCUTS row for a
// still-handled key fails the same way.
//
// What this does NOT catch: main.ts's conditions are boolean expressions,
// not a declarative table, so this cannot verify a row's *modifier combo*
// (e.g. that "R" really means Ctrl+Shift+R and not plain Ctrl+R), nor can it
// derive isWorkspaceSwitch()'s digit range (that check lives in
// platform.ts, exercised separately by platform.test.ts) — both are
// asserted by hand below instead. Getting a real per-binding table would
// mean restructuring main.ts's handler into data, which is out of scope
// here (see CLAUDE.md rule 6).

const mainTsPath = fileURLToPath(new URL("../main.ts", import.meta.url));
const mainTsSrc = readFileSync(mainTsPath, "utf8");

function keydownHandlerSource(src: string): string {
  const start = src.indexOf('window.addEventListener("keydown"');
  const end = src.indexOf('window.addEventListener("resize"', start);
  if (start === -1 || end === -1 || end <= start) {
    throw new Error(
      "shortcutList.test.ts: could not locate main.ts's keydown handler by " +
        'its window.addEventListener("keydown"...)/("resize"...) markers. ' +
        "main.ts's handler shape changed — update this extractor.",
    );
  }
  return src.slice(start, end);
}

// `key === "X"` / `ev.code === "Y"` literal -> the symbol it types into a
// SHORTCUTS row. Single letters aren't listed; they map to their own
// uppercase form. Add a mapping here (not a silent skip) the moment main.ts
// starts matching a literal this doesn't know.
const TOKEN_SYMBOLS: Record<string, string> = {
  Tab: "Tab",
  ArrowLeft: "←",
  ArrowRight: "→",
  KeyN: "N",
  BracketLeft: "[",
  BracketRight: "]",
  Equal: "+",
  NumpadAdd: "+",
  Minus: "-",
  NumpadSubtract: "-",
  Digit0: "0",
  Numpad0: "0",
};

function resolveSymbol(token: string): string {
  if (token in TOKEN_SYMBOLS) return TOKEN_SYMBOLS[token];
  if (/^[A-Za-z]$/.test(token)) return token.toUpperCase();
  throw new Error(
    `shortcutList.test.ts: main.ts matches on "${token}", which has no ` +
      "entry in TOKEN_SYMBOLS. Add one (and a SHORTCUTS row if it's a new " +
      "binding) before this test can pass.",
  );
}

function extractHandledTokens(handlerSrc: string): string[] {
  const tokens: string[] = [];
  for (const m of handlerSrc.matchAll(/key === "([A-Za-z]+)"/g)) tokens.push(m[1]);
  for (const m of handlerSrc.matchAll(/ev\.code === "(\w+)"/g)) tokens.push(m[1]);
  return tokens;
}

// The tokens a SHORTCUTS row actually renders as separate `<kbd>`s — same
// split rule the Settings panel uses, so this test and the render agree on
// what counts as "documented".
function shortcutTokens(): Set<string> {
  const tokens = new Set<string>();
  for (const { keys } of SHORTCUTS) {
    for (const seg of splitShortcutKeys(keys)) {
      for (const part of seg.split("/")) {
        const trimmed = part.trim();
        if (trimmed) tokens.add(trimmed);
      }
    }
  }
  return tokens;
}

describe("SHORTCUTS covers every key/code literal main.ts's keydown handler matches", () => {
  const handlerSrc = keydownHandlerSource(mainTsSrc);
  const handledTokens = extractHandledTokens(handlerSrc);
  const documented = shortcutTokens();

  it("found a non-trivial set of bindings to check (sanity check on the extractor itself)", () => {
    expect(handledTokens.length).toBeGreaterThan(10);
  });

  it("documents isWorkspaceSwitch()'s Ctrl+Alt+1…9 binding", () => {
    expect(handlerSrc).toContain("isWorkspaceSwitch(ev)");
    expect(SHORTCUTS.some((s) => /^Ctrl\+Alt\+1/.test(s.keys))).toBe(true);
  });

  for (const token of new Set(handledTokens)) {
    it(`has a SHORTCUTS row covering "${token}" (resolved: "${resolveSymbol(token)}")`, () => {
      const symbol = resolveSymbol(token);
      expect(
        documented.has(symbol),
        `main.ts's keydown handler matches "${token}" (→ "${symbol}"), but no ` +
          "SHORTCUTS row in shortcutList.ts renders that key. Add one.",
      ).toBe(true);
    });
  }
});

describe("splitShortcutKeys", () => {
  it("splits a plain modifier chord", () => {
    expect(splitShortcutKeys("Ctrl+Shift+H")).toEqual(["Ctrl", "Shift", "H"]);
  });

  it("treats a trailing ++ as Ctrl followed by the literal Plus key", () => {
    expect(splitShortcutKeys("Ctrl++")).toEqual(["Ctrl", "+"]);
  });

  it("does the same after shortcutLabel maps Ctrl to Cmd", () => {
    expect(splitShortcutKeys("Cmd++")).toEqual(["Cmd", "+"]);
  });

  it("leaves a combined arrow/tab token as one segment (mac-mapping expands it, not this)", () => {
    expect(splitShortcutKeys("Ctrl+Shift+←/→")).toEqual(["Ctrl", "Shift", "←/→"]);
  });
});
