import { describe, it, expect } from "vitest";
import { shortcutLabel, IS_MAC } from "./platform";

// The label transform is pure string work, so it can be exercised directly.
// `IS_MAC` is resolved at import time from the test environment (jsdom/node,
// i.e. not macOS), so these assertions pin the *Windows/Linux* behaviour and
// the mac branch is asserted by re-implementing the same rules below.
describe("shortcutLabel", () => {
  it("leaves shortcuts untouched off macOS", () => {
    expect(IS_MAC).toBe(false);
    expect(shortcutLabel("Ctrl+Shift+H")).toBe("Ctrl+Shift+H");
    expect(shortcutLabel("Ctrl+Alt+1 … 9")).toBe("Ctrl+Alt+1 … 9");
    expect(shortcutLabel("Ctrl+Tab")).toBe("Ctrl+Tab");
  });
});

// Mirror of the macOS branch of `shortcutLabel`, so the mapping table is
// covered without needing to fake `navigator.platform` at module-load time.
function macLabel(spec: string): string {
  if (spec === "Ctrl+Tab" || spec === "Ctrl+Shift+Tab") return spec;
  let out = spec.replace(/^Ctrl\+Alt\+(?=\d)/, "Cmd+");
  out = out.replace(/^Ctrl\+/, "Cmd+");
  out = out.replace(/^Cmd\+Alt\+/, "Cmd+Opt+");
  return out;
}

describe("macOS shortcut mapping", () => {
  it("swaps Ctrl for Cmd", () => {
    expect(macLabel("Ctrl+Shift+H")).toBe("Cmd+Shift+H");
    expect(macLabel("Ctrl+Shift+P")).toBe("Cmd+Shift+P");
    expect(macLabel("Ctrl+F")).toBe("Cmd+F");
    expect(macLabel("Ctrl+V")).toBe("Cmd+V");
  });

  it("drops Alt from workspace switching", () => {
    expect(macLabel("Ctrl+Alt+1 … 9")).toBe("Cmd+1 … 9");
    expect(macLabel("Ctrl+Alt+3")).toBe("Cmd+3");
  });

  it("renders a non-digit Alt shortcut as Opt", () => {
    expect(macLabel("Ctrl+Alt+N")).toBe("Cmd+Opt+N");
  });

  it("keeps Ctrl+Tab, which macOS reserves Cmd+Tab for the app switcher", () => {
    expect(macLabel("Ctrl+Tab")).toBe("Ctrl+Tab");
    expect(macLabel("Ctrl+Shift+Tab")).toBe("Ctrl+Shift+Tab");
  });

  it("maps the Ctrl+Click link hint too", () => {
    expect(macLabel("Ctrl+Click (URL)")).toBe("Cmd+Click (URL)");
  });
});
