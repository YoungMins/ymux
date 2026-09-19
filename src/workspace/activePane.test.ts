import { describe, it, expect } from "vitest";
import { pickActivePaneId } from "./activePane";

describe("pickActivePaneId", () => {
  it("is the focused pane when it belongs to the active workspace", () => {
    expect(pickActivePaneId(["a", "b"], "b")).toBe("b");
  });

  it("falls back to the first pane when focus still points into the previous workspace", () => {
    // After a workspace switch `focusedPaneId` keeps the old id until
    // something is clicked; the dock must follow the new workspace anyway.
    expect(pickActivePaneId(["a", "b"], "other-ws-pane")).toBe("a");
    expect(pickActivePaneId(["a", "b"], null)).toBe("a");
  });

  it("is null for a workspace with no panes", () => {
    expect(pickActivePaneId([], "a")).toBeNull();
  });
});
