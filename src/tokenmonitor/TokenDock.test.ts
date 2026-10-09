// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { api } from "../ipc/bridge";
import { mountTokenDock, toggleTokenDock } from "./TokenDock";

afterEach(() => { document.body.replaceChildren(); localStorage.clear(); vi.restoreAllMocks(); vi.useRealTimers(); });

it("keeps one dock outside workspace panes, persists opening and polls every ten seconds", async () => {
  // Given an app body with a workspace host and a native IPC boundary.
  vi.useFakeTimers();
  const read = vi.spyOn(api, "getTokenUsage").mockResolvedValue({ collected_at: 0, providers: [], rows: [], warnings: [] });
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockImplementation(function (this: HTMLElement) {
    return { length: this.closest(".token-dock")?.classList.contains("token-dock--open") ? 1 : 0, item: () => null, [Symbol.iterator]: () => [].values() };
  });
  const body = document.createElement("div"); const workspace = document.createElement("div");
  body.append(workspace); document.body.append(body);
  const manager = { focusActivePane: vi.fn(), refitActive: vi.fn() };
  const dock = mountTokenDock(body, manager);
  await Promise.resolve(); expect(read).not.toHaveBeenCalled();
  // When the sidebar opens and another workspace replaces the workspace content.
  toggleTokenDock(); workspace.replaceChildren(document.createElement("article"));
  await vi.advanceTimersByTimeAsync(20_000);
  // Then it stays app-wide and performs the initial read plus two polls.
  expect(body.querySelectorAll(".token-dock")).toHaveLength(1);
  expect(workspace.querySelector(".token-dock")).toBeNull();
  expect(read).toHaveBeenCalledTimes(3);
  expect(JSON.parse(localStorage.getItem("ymux.tokenDock") ?? "null")).toMatchObject({ open: true });
  dock.dispose();
  const restored = mountTokenDock(body, manager);
  expect(restored.element.classList.contains("token-dock--open")).toBe(true);
  restored.dispose();
});

it("returns focus to the active pane and stops reads when the dock closes", async () => {
  vi.useFakeTimers();
  const read = vi.spyOn(api, "getTokenUsage").mockResolvedValue({ collected_at: 0, providers: [], rows: [], warnings: [] });
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockImplementation(function (this: HTMLElement) {
    return { length: this.closest(".token-dock")?.classList.contains("token-dock--open") ? 1 : 0, item: () => null, [Symbol.iterator]: () => [].values() };
  });
  const manager = { focusActivePane: vi.fn(), refitActive: vi.fn() };
  const dock = mountTokenDock(document.body, manager);
  toggleTokenDock(); await Promise.resolve();
  toggleTokenDock(); await vi.advanceTimersByTimeAsync(30_000);
  expect(manager.focusActivePane).toHaveBeenCalledOnce();
  expect(read).toHaveBeenCalledOnce();
  dock.dispose();
});
