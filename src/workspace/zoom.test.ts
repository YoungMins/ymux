import { describe, it, expect } from "vitest";
import { zoomAction } from "./zoom";

const PANE = "pppppppp-pppp-4ppp-8ppp-pppppppppppp";
const GROUP = "gggggggg-gggg-4ggg-8ggg-gggggggggggg";

describe("zoomAction", () => {
  it("does nothing when nothing is zoomed", () => {
    expect(zoomAction(null, false, null)).toEqual({ kind: "none" });
    expect(zoomAction(null, true, GROUP)).toEqual({ kind: "none" });
  });

  it("re-applies the zoom to the group when the zoomed pane is a tab", () => {
    expect(zoomAction(PANE, true, GROUP)).toEqual({
      kind: "apply",
      paneId: PANE,
      groupId: GROUP,
    });
  });

  it("re-applies to the pane itself once its group has unwrapped", () => {
    // "Close other tabs" leaves one tab, which unwraps back to a plain pane:
    // the zoom must move from the group element to the pane element.
    expect(zoomAction(PANE, true, null)).toEqual({
      kind: "apply",
      paneId: PANE,
      groupId: null,
    });
  });

  it("clears the zoom when the zoomed pane is gone", () => {
    // Closing the zoomed pane must take `workspace--zoomed` off the
    // container, or every sibling stays `display: none` and the workspace
    // renders blank.
    expect(zoomAction(PANE, false, null)).toEqual({ kind: "clear" });
  });
});
