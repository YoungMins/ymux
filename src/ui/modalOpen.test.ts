// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from "vitest";
import { isModalOverlayOpen } from "./modalOpen";

describe("isModalOverlayOpen", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("is false with no overlay or with hidden ones", () => {
    document.body.innerHTML = `<div class="notes-modal"></div><div class="palette"></div>`;
    expect(isModalOverlayOpen()).toBe(false);
  });

  it.each(["notes-modal--visible", "settings-modal--visible", "palette--visible", "queue-dialog", "help-modal--visible", "dialog-backdrop"])(
    "is true while %s is present",
    (cls) => {
      document.body.innerHTML = `<div class="${cls}"></div>`;
      expect(isModalOverlayOpen()).toBe(true);
    },
  );
});
