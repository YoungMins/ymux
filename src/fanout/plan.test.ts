import { describe, expect, it } from "vitest";
import type { DetectedAgent } from "../types";
import { laneAgents, laneSpec, plainAgentCommand } from "./plan";

const agent = (id: string, over: Partial<DetectedAgent> = {}): DetectedAgent => ({
  id,
  name: id,
  program: id,
  path: null,
  bypass_args: ["--dangerously-skip-permissions"],
  note: null,
  powershell_only: false,
  ...over,
});

describe("plainAgentCommand", () => {
  it("never carries bypass flags", () => {
    expect(plainAgentCommand(agent("claude"), "unknown")).toBe("claude");
  });
  it("quotes an off-PATH agent and keeps the null for unrunnable ones", () => {
    expect(plainAgentCommand(agent("codex", { powershell_only: true }), "cmd")).toBeNull();
  });
});

describe("laneAgents", () => {
  it("keeps only claude and codex, in that order", () => {
    const got = laneAgents([agent("opencode"), agent("codex"), agent("claude")]);
    expect(got.map((a) => a.id)).toEqual(["claude", "codex"]);
  });
});

describe("laneSpec", () => {
  it("is a terminal in the worktree running the command, with no hotkey/startup leakage", () => {
    const s = laneSpec({ shell: "pwsh", path: "C:/wt/a", command: "claude", account: "ab12cd34", title: "fanout/claude-1" });
    expect(s.pane_kind).toBe("terminal");
    expect(s.cwd).toBe("C:/wt/a");
    expect(s.worktree_path).toBe("C:/wt/a");
    expect(s.startup_cmd).toBe("claude");
    expect(s.account).toBe("ab12cd34");
    expect(s.title).toBe("fanout/claude-1");
  });
  it("leaves account unset when inheriting", () => {
    expect(laneSpec({ shell: "", path: "/x", command: "c", account: "", title: "t" }).account).toBeUndefined();
  });
});
