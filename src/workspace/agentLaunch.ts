// What the top bar's "+" launcher types into a new tab to start an agent.
//
// The agent table and detection live in the backend
// (`src-tauri/src/agent_launch.rs`); this only turns one detected agent into
// the command line for the pane's shell. It becomes the tab's `startup_cmd`,
// so the process scan, the agent tree and resume treat it like any other
// startup command.

import type { DetectedAgent } from "../types";
import { quotePathForShell, type ShellFamily } from "../terminal/shellQuote";

/// The command to type, or `null` when the agent sits off `PATH` at a path
/// this shell cannot be handed safely (the launcher then leaves it out).
///
/// An agent on `PATH` (the app's own, or on macOS/Linux the login shell's,
/// which is what the pane's login shell will have) is typed by bare name. One
/// found only in a known install directory is typed by its quoted absolute path, since the shell's
/// `PATH` may lack that directory too. PowerShell needs `& ` in front of a
/// quoted path: on its own a quoted string is an expression, not a call.
///
/// A `.ps1`-only agent is offered to PowerShell alone: cmd does not resolve
/// a `.ps1` by bare name, and a typed `.ps1` path opens through its file
/// association and exits without running anything.
export function agentCommand(
  agent: DetectedAgent,
  family: ShellFamily,
  withBypass = true,
): string | null {
  if (agent.powershell_only && family !== "powershell") return null;
  let program = agent.program;
  if (agent.path) {
    const quoted = quotePathForShell(agent.path, family);
    if (quoted === null) return null;
    program = family === "powershell" ? `& ${quoted}` : quoted;
  }
  return [program, ...(withBypass ? agent.bypass_args : [])].join(" ");
}

/// The agents the launcher can offer for `family`, in the backend's order.
export function launchableAgents(
  agents: readonly DetectedAgent[],
  family: ShellFamily,
): { agent: DetectedAgent; command: string }[] {
  const out: { agent: DetectedAgent; command: string }[] = [];
  for (const agent of agents) {
    const command = agentCommand(agent, family);
    if (command !== null) out.push({ agent, command });
  }
  return out;
}

/// Whether a pane's saved startup command already carries all of `agent`'s
/// permission-bypass arguments, i.e. the user (or the "+" launcher) started it
/// that way.
export function startedWithBypass(agent: DetectedAgent, startupCmd: string | null | undefined): boolean {
  if (!startupCmd || agent.bypass_args.length === 0) return false;
  const words = startupCmd.split(/\s+/);
  return agent.bypass_args.every((a) => words.includes(a));
}

/// The command "Switch account" types into the respawned shell: the bare agent
/// unless the pane's own startup command shows it ran with the bypass
/// arguments. A plain `claude` must never come back in bypass mode.
export function switchAccountCommand(
  agent: DetectedAgent,
  family: ShellFamily,
  startupCmd: string | null | undefined,
): string | null {
  return agentCommand(agent, family, startedWithBypass(agent, startupCmd));
}
