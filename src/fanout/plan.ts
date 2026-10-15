import type { DetectedAgent, PaneSpec } from "../types";
import { newPane } from "../layout/LayoutTree";
import { agentCommand } from "../workspace/agentLaunch";
import type { ShellFamily } from "../terminal/shellQuote";

/// Agent CLIs a lane may run.
export const LANE_AGENTS = ["claude", "codex"] as const;

/// The agent's command line with no bypass/permission flags (owner decision:
/// fan-out lanes run the plain CLI). `null` when this shell cannot run it.
export function plainAgentCommand(agent: DetectedAgent, family: ShellFamily): string | null {
  return agentCommand({ ...agent, bypass_args: [] }, family);
}

/// Detected agents a lane can use, in `LANE_AGENTS` order.
export function laneAgents(agents: readonly DetectedAgent[]): DetectedAgent[] {
  const out: DetectedAgent[] = [];
  for (const id of LANE_AGENTS) {
    const a = agents.find((x) => x.id === id);
    if (a) out.push(a);
  }
  return out;
}

/// One lane's terminal pane in its worktree. `account` "" = inherit.
export function laneSpec(opts: {
  shell: string;
  path: string;
  command: string;
  account: string;
  title: string;
}): PaneSpec {
  const spec = newPane(opts.shell, opts.path);
  spec.worktree_path = opts.path;
  spec.startup_cmd = opts.command;
  spec.title = opts.title;
  if (opts.account) spec.account = opts.account;
  return spec;
}
