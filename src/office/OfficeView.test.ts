// @vitest-environment jsdom
// The office overlay on a stub workspace: rooms kept across renders, the
// keyboard queue and the help window.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { newPane, paneNode } from "../layout/LayoutTree";
import type { AgentSnapshot, LayoutNode } from "../types";
import { OfficeView, type OfficeSource } from "./OfficeView";

const pane = (id: string): LayoutNode => ({ ...paneNode(newPane("bash")), id }) as LayoutNode;

function stub(): { source: OfficeSource; opened: string[]; refresh: () => void } {
  const root: LayoutNode = { kind: "split", direction: "horizontal", ratio: 0.5, a: pane("A"), b: pane("B") } as LayoutNode;
  const agents: AgentSnapshot = {
    A: { lead: { kind: "claude", status: "working", source: "hook", tool: "Bash" }, subagents: [] },
    B: { lead: { kind: "codex", status: "waiting", source: "process", tool: null }, subagents: [] },
  } as AgentSnapshot;
  const status: Record<string, string> = { A: "running", B: "attention" };
  let cb = (): void => {};
  const opened: string[] = [];
  const source: OfficeSource = {
    input: () => ({ root, agents, statusOf: (id: string) => (status[id] ?? "idle") as never, labelOf: (id: string) => id, focused: "A", mailOf: () => [] }) as never,
    workspaceName: () => "1 · test",
    floors: () => [{ id: 1, label: "1", position: 1, waiting: 1, working: 1 }],
    activeFloor: () => 1,
    floorOf: () => 1,
    waiting: () => [{ paneId: "B", ws: 1, label: "B", since: Date.now() - 5000 }],
    outputAt: () => null,
    toolSince: () => null,
    statusSince: () => null,
    toolCalls: () => 0,
    review: async () => "ok",
    launchers: async () => [],
    launch: () => {},
    setMini: () => {},
    openPane: (id) => opened.push(id),
    close: () => {},
    subscribe: (f) => {
      cb = f;
      return () => {};
    },
    log: () => [],
    accountOf: () => "",
    accounts: () => [],
    tokenUsage: async () => ({ collected_at: 0, providers: [], hourly: [], warnings: [], rows: [] }) as never,
    screen: () => ["Run tests?", "❯ 1. Yes", "  2. No"],
    send: async () => "sent",
    key: () => {},
    type: () => {},
    cursorKeys: () => false,
  };
  return { source, opened, refresh: () => cb() };
}

describe("OfficeView", () => {
  let view: OfficeView | null = null;
  beforeEach(() => {
    localStorage.clear();
    vi.stubGlobal("requestAnimationFrame", (f: FrameRequestCallback) => {
      f(0);
      return 0;
    });
  });
  afterEach(() => {
    view?.dispose();
    view = null;
    vi.unstubAllGlobals();
  });

  it("keeps the same room elements when nothing about the rooms changed", () => {
    const s = stub();
    view = new OfficeView(s.source);
    document.body.appendChild(view.element);
    const before = [...view.element.querySelectorAll(".office-room")];
    expect(before.length).toBe(2);
    s.refresh();
    const after = [...view.element.querySelectorAll(".office-room")];
    expect(after).toEqual(before);
  });

  it("N opens the next agent waiting", () => {
    const s = stub();
    view = new OfficeView(s.source);
    document.body.appendChild(view.element);
    view.element.dispatchEvent(new KeyboardEvent("keydown", { key: "n", bubbles: true }));
    const peek = view.element.querySelector<HTMLElement>(".office-peek");
    expect(peek?.hidden).toBe(false);
  });

  it("? toggles the help window and Esc closes it", () => {
    const s = stub();
    view = new OfficeView(s.source);
    document.body.appendChild(view.element);
    const help = view.element.querySelector<HTMLElement>(".office-help");
    expect(help?.hidden).toBe(true);
    view.element.dispatchEvent(new KeyboardEvent("keydown", { key: "?", bubbles: true }));
    expect(help?.hidden).toBe(false);
    view.element.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(help?.hidden).toBe(true);
  });
});
