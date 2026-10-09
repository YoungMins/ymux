// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";

vi.mock("../files/FilesPane", () => ({
  FilesPane: class {
    readonly element = document.createElement("div");
    constructor() {
      this.element.className = "files-pane";
    }
    navigate = vi.fn();
    follow = vi.fn();
    focus = vi.fn();
    scheduleFit = vi.fn();
    spawn = vi.fn().mockResolvedValue(undefined);
  },
}));

import { api } from "../ipc/bridge";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import { mountSidePanel, toggleFileDock, toggleTokenDock } from "./SidePanel";

afterEach(() => {
  document.body.replaceChildren();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

const empty = { collected_at: 0, providers: [], rows: [], warnings: [] };

function setup() {
  vi.useFakeTimers();
  const read = vi.spyOn(api, "getTokenUsage").mockResolvedValue(empty);
  vi.spyOn(api, "getPaneCwd").mockResolvedValue(null);
  // jsdom has no layout: a view counts as rendered when the panel is open
  // and the view is the active one.
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockImplementation(function (this: HTMLElement) {
    const shown =
      this.closest(".side-panel--open") !== null && this.closest(".side-panel__view--active") !== null;
    return { length: shown ? 1 : 0, item: () => null, [Symbol.iterator]: () => [].values() };
  });
  const manager = {
    focusActivePane: vi.fn(),
    refitActive: vi.fn(),
    activePaneId: () => null,
    onActivePaneChange: vi.fn(),
  };
  const body = document.createElement("div");
  document.body.append(body);
  mountSidePanel(body, manager as unknown as WorkspaceManager);
  const panel = body.querySelector<HTMLElement>(".side-panel")!;
  return { read, manager, body, panel };
}

it("hosts both views in one panel; usage polls only while it is the visible view", async () => {
  const { read, body, panel } = setup();
  await vi.advanceTimersByTimeAsync(0);
  expect(body.querySelectorAll(".side-panel")).toHaveLength(1);
  expect(panel.querySelectorAll(".files-pane")).toHaveLength(1);
  expect(read).not.toHaveBeenCalled();

  toggleTokenDock();
  await vi.advanceTimersByTimeAsync(20_000);
  expect(read).toHaveBeenCalledTimes(3);
  expect(panel.classList.contains("side-panel--open")).toBe(true);
  expect(JSON.parse(localStorage.getItem("ymux.sidePanel") ?? "null")).toMatchObject({
    open: true,
    view: "usage",
  });

  // Switching to files hides the usage view: its polling pauses.
  toggleFileDock();
  await vi.advanceTimersByTimeAsync(30_000);
  expect(read).toHaveBeenCalledTimes(3);
  const tabs = panel.querySelectorAll<HTMLElement>('[role="tab"]');
  expect(tabs[0].getAttribute("aria-selected")).toBe("true");
  expect(tabs[1].getAttribute("aria-selected")).toBe("false");
});

it("toggles closed on the showing view and returns focus to the active pane", async () => {
  const { manager, panel } = setup();
  await vi.advanceTimersByTimeAsync(0);
  toggleTokenDock();
  panel.querySelector<HTMLElement>(".side-panel__view--active")!.tabIndex = 0;
  panel.querySelector<HTMLElement>(".side-panel__view--active")!.focus();
  toggleTokenDock();
  expect(panel.classList.contains("side-panel--open")).toBe(false);
  expect(manager.focusActivePane).toHaveBeenCalledOnce();
});

it("clicking the active tab keeps the panel open; the close button closes it", async () => {
  const { panel } = setup();
  await vi.advanceTimersByTimeAsync(0);
  toggleFileDock();
  const tabs = panel.querySelectorAll<HTMLElement>('[role="tab"]');
  tabs[0].click();
  expect(panel.classList.contains("side-panel--open")).toBe(true);
  tabs[1].click();
  expect(tabs[1].getAttribute("aria-selected")).toBe("true");
  panel.querySelector<HTMLElement>(".side-panel__close")!.click();
  expect(panel.classList.contains("side-panel--open")).toBe(false);
});

it("seeds the first run from the legacy token dock state", async () => {
  localStorage.setItem("ymux.tokenDock", JSON.stringify({ open: true, width: 400, v: 2 }));
  const { panel } = setup();
  await vi.advanceTimersByTimeAsync(0);
  expect(panel.classList.contains("side-panel--open")).toBe(true);
  expect(panel.querySelector('[role="tab"][aria-selected="true"]')).toBe(
    panel.querySelectorAll('[role="tab"]')[1],
  );
});

it("arrow keys, Home and End move between tabs with wrap-around and never close", async () => {
  const { panel } = setup();
  await vi.advanceTimersByTimeAsync(0);
  toggleFileDock();
  const tablist = panel.querySelector<HTMLElement>('[role="tablist"]')!;
  expect(tablist.querySelector(".side-panel__close")).toBeNull();
  const tabs = panel.querySelectorAll<HTMLElement>('[role="tab"]');
  const press = (key: string) =>
    tablist.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  press("ArrowRight");
  expect(tabs[1].getAttribute("aria-selected")).toBe("true");
  expect(tabs[1].tabIndex).toBe(0);
  expect(tabs[0].tabIndex).toBe(-1);
  expect(document.activeElement).toBe(tabs[1]);
  press("ArrowRight");
  expect(tabs[0].getAttribute("aria-selected")).toBe("true");
  press("ArrowLeft");
  expect(tabs[1].getAttribute("aria-selected")).toBe("true");
  press("Home");
  expect(tabs[0].getAttribute("aria-selected")).toBe("true");
  press("End");
  expect(tabs[1].getAttribute("aria-selected")).toBe("true");
  expect(panel.classList.contains("side-panel--open")).toBe(true);
  const view = panel.querySelector(`#${tabs[1].getAttribute("aria-controls")}`)!;
  expect(view.getAttribute("role")).toBe("tabpanel");
  expect(view.getAttribute("aria-labelledby")).toBe(tabs[1].id);
});

it("refits the workspace only when the panel opens or closes, not on view switches", async () => {
  const { manager, panel } = setup();
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => (cb(0), 0));
  await vi.advanceTimersByTimeAsync(0);
  manager.refitActive.mockClear();
  toggleFileDock();
  expect(manager.refitActive).toHaveBeenCalledTimes(1);
  panel.querySelectorAll<HTMLElement>('[role="tab"]')[1].click();
  expect(manager.refitActive).toHaveBeenCalledTimes(1);
  panel.querySelector<HTMLElement>(".side-panel__close")!.click();
  expect(manager.refitActive).toHaveBeenCalledTimes(2);
  vi.unstubAllGlobals();
});
