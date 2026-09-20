import { describe, it, expect } from "vitest";
import { tabLabel } from "./tabLabel";

const base = { title: null, shell: "pwsh", process: null, fallback: "Terminal" };

describe("tabLabel", () => {
  it("prefers a user-set title over everything", () => {
    expect(tabLabel({ ...base, title: "build", process: "claude" })).toBe("build");
  });

  it("ignores a whitespace-only title", () => {
    expect(tabLabel({ ...base, title: "   ", process: "claude" })).toBe("claude");
  });

  it("trims a user-set title", () => {
    expect(tabLabel({ ...base, title: "  build  " })).toBe("build");
  });

  it("uses the running program when there is no title", () => {
    expect(tabLabel({ ...base, process: "ycode: main.rs" })).toBe("ycode: main.rs");
  });

  it("falls back to the shell name when nothing is running under it", () => {
    expect(tabLabel(base)).toBe("pwsh");
  });

  it("falls back to the localised default when there is no shell either", () => {
    expect(tabLabel({ ...base, shell: "" })).toBe("Terminal");
  });

  it("treats an undefined title like a missing one", () => {
    expect(tabLabel({ shell: "cmd", process: null, fallback: "Terminal" })).toBe("cmd");
  });
});
