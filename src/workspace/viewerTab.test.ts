import { describe, it, expect } from "vitest";
import { viewerTabAction } from "./viewerTab";

describe("viewerTabAction", () => {
  it("creates a viewer tab the first time", () => {
    expect(viewerTabAction(undefined, ["a", "b"])).toEqual({ kind: "create" });
  });

  it("reuses the registered tab while it is still a member of the group", () => {
    expect(viewerTabAction("v", ["a", "v", "b"])).toEqual({
      kind: "reuse",
      paneId: "v",
    });
  });

  it("creates again once the registered tab has been closed", () => {
    expect(viewerTabAction("v", ["a", "b"])).toEqual({ kind: "create" });
  });

  it("creates for an empty group", () => {
    expect(viewerTabAction("v", [])).toEqual({ kind: "create" });
  });
});
