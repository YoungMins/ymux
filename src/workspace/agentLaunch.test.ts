import { describe, it, expect } from "vitest";
import { agentCommand, launchableAgents } from "./agentLaunch";
import type { DetectedAgent } from "../types";

const onPath = (over: Partial<DetectedAgent> = {}): DetectedAgent => ({
  id: "claude",
  name: "Claude Code",
  program: "claude",
  path: null,
  bypass_args: ["--dangerously-skip-permissions"],
  note: null,
  powershell_only: false,
  ...over,
});

describe("agentCommand", () => {
  it("types the bare name plus the bypass flag when the agent is on PATH", () => {
    for (const family of ["posix", "fish", "powershell", "cmd", "unknown"] as const) {
      expect(agentCommand(onPath(), family)).toBe("claude --dangerously-skip-permissions");
    }
  });

  it("types just the name for an agent without a bypass flag", () => {
    expect(agentCommand(onPath({ id: "opencode", program: "opencode", bypass_args: [] }), "cmd")).toBe(
      "opencode",
    );
  });

  it("quotes an off-PATH absolute path for the shell family", () => {
    const win = onPath({ path: "C:\\Users\\me\\.local\\bin\\claude.exe" });
    expect(agentCommand(win, "cmd")).toBe(
      '"C:\\Users\\me\\.local\\bin\\claude.exe" --dangerously-skip-permissions',
    );
    const mac = onPath({ path: "/opt/homebrew/bin/claude" });
    expect(agentCommand(mac, "posix")).toBe("'/opt/homebrew/bin/claude' --dangerously-skip-permissions");
    expect(agentCommand(mac, "fish")).toBe("'/opt/homebrew/bin/claude' --dangerously-skip-permissions");
  });

  it("invokes a quoted path with & in PowerShell, where a bare string is not a call", () => {
    const win = onPath({ path: "C:\\Users\\O'Brien\\npm\\codex.cmd", program: "codex", bypass_args: ["--x"] });
    expect(agentCommand(win, "powershell")).toBe("& 'C:\\Users\\O''Brien\\npm\\codex.cmd' --x");
  });

  it("offers a .ps1-only shim to PowerShell alone", () => {
    const bare = onPath({ program: "codex", bypass_args: ["--x"], powershell_only: true });
    expect(agentCommand(bare, "powershell")).toBe("codex --x");
    const abs = { ...bare, path: "C:\\npm\\codex.ps1" };
    expect(agentCommand(abs, "powershell")).toBe("& 'C:\\npm\\codex.ps1' --x");
    for (const family of ["cmd", "posix", "fish", "unknown"] as const) {
      expect(agentCommand(bare, family)).toBeNull();
      expect(agentCommand(abs, family)).toBeNull();
    }
  });

  it("refuses a path the shell cannot quote safely", () => {
    // A WSL / unknown shell refuses any backslashed path.
    expect(agentCommand(onPath({ path: "C:\\x\\claude.exe" }), "unknown")).toBeNull();
    expect(agentCommand(onPath({ path: "/a\nb/claude" }), "posix")).toBeNull();
  });
});

describe("launchableAgents", () => {
  it("keeps table order and drops agents whose command cannot be typed", () => {
    const agents = [
      onPath(),
      onPath({ id: "kimi", program: "kimi", path: "C:\\h\\.kimi-code\\bin\\kimi.exe", bypass_args: ["--yolo"] }),
      onPath({ id: "gemini", program: "gemini", bypass_args: ["--yolo"] }),
    ];
    const got = launchableAgents(agents, "unknown").map((a) => [a.agent.id, a.command]);
    expect(got).toEqual([
      ["claude", "claude --dangerously-skip-permissions"],
      ["gemini", "gemini --yolo"],
    ]);
  });
});
