// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { paintPaneFlags } from "./paneFlags";

describe("paintPaneFlags", () => {
  it("adds, updates and removes chips", () => {
    const el = document.createElement("div");
    paintPaneFlags(el, { queuedText: "2 queued", broadcastText: null });
    expect(el.querySelectorAll(".pane-flag").length).toBe(1);
    expect(el.querySelector(".pane-flag--queue")?.textContent).toBe("2 queued");
    expect(el.classList.contains("pane--broadcast")).toBe(false);

    paintPaneFlags(el, { queuedText: "3 queued", broadcastText: "BROADCAST" });
    expect(el.querySelectorAll(".pane-flags").length).toBe(1);
    expect(el.querySelector(".pane-flag--queue")?.textContent).toBe("3 queued");
    expect(el.querySelector(".pane-flag--broadcast")?.textContent).toBe("BROADCAST");
    expect(el.classList.contains("pane--broadcast")).toBe(true);

    paintPaneFlags(el, { queuedText: null, broadcastText: "BROADCAST" });
    expect(el.querySelector(".pane-flag--queue")).toBeNull();

    paintPaneFlags(el, { queuedText: null, broadcastText: null });
    expect(el.querySelector(".pane-flags")).toBeNull();
    expect(el.classList.contains("pane--broadcast")).toBe(false);
  });
});
