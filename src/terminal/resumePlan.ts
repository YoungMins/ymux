/// The one decision a pane makes on spawn: resume the agent that was running
/// here, or replay the scrollback the way a shell pane always has.
///
/// Pure and separate from `TerminalPane` so it can be tested without a DOM, a
/// PTY or a Tauri bridge — the spec calls for exactly that (§7).

/// Mirrors `agent_sessions::ResumePlan` on the Rust side. `command` is already
/// merged with the pane's own `startup_cmd` and carries an explicit session
/// id, never a "most recent" selector.
export interface ResumePlan {
  agent: string;
  command: string;
  cwd: string;
  age_secs: number;
}

export type SpawnAction =
  | { kind: "resume"; plan: ResumePlan }
  | { kind: "restore" }
  | { kind: "fresh" };

/// What a pane should do as it comes up.
///
/// Resume wins outright when the backend offered a plan, and it *replaces*
/// the scrollback replay rather than layering on top of it (spec §4/§5). Two
/// reasons, both visible on screen: a resumed agent redraws its own history,
/// so replaying would park dead backlog above it; and ConPTY opens every
/// session with `\x1b[2J\x1b[H`, so the restored block has to be scrolled out
/// of the viewport to survive at all — work that is pointless for history the
/// agent is about to redraw itself.
///
/// Everything else is unchanged: a pane with persistence on and a saved blob
/// restores, and anything else starts clean.
export function spawnAction(params: {
  plan: ResumePlan | null | undefined;
  persistEnabled: boolean;
}): SpawnAction {
  if (params.plan) return { kind: "resume", plan: params.plan };
  if (params.persistEnabled) return { kind: "restore" };
  return { kind: "fresh" };
}

/// Whether this pane should save its scrollback at all.
///
/// A pane that resumes its agent neither restores nor saves (spec §5): the
/// blob it would write is a picture of a conversation that is being continued
/// for real, and keeping it would only feed the next launch something to
/// replay above the resumed session.
export function shouldPersistScrollback(params: {
  persistEnabled: boolean;
  resuming: boolean;
}): boolean {
  return params.persistEnabled && !params.resuming;
}

/// Coarse "3 hours ago" for the resume banner.
///
/// Deliberately coarse: the banner is one line of reassurance, not a
/// timestamp, and rounding down means it never claims more recency than it
/// has. Returns the unit and the count so the caller can hand them to i18n
/// rather than building an English string here (rule 7).
export function describeAge(ageSecs: number): {
  unit: "now" | "minutes" | "hours";
  count: number;
} {
  const secs = Math.max(0, Math.floor(ageSecs));
  if (secs < 60) return { unit: "now", count: 0 };
  const minutes = Math.floor(secs / 60);
  if (minutes < 60) return { unit: "minutes", count: minutes };
  return { unit: "hours", count: Math.floor(minutes / 60) };
}
