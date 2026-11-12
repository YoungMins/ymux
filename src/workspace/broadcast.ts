// Broadcast input (tmux synchronize-panes): while a workspace has it on, what
// the user types or pastes into its focused terminal pane is also written to
// the other terminal panes shown in that workspace. Pure decisions; the hooks
// live in TerminalPane (user input only) and WorkspaceManager (wiring).

import type { Uuid } from "../types";

export interface BroadcastPane {
  id: Uuid;
  /// A terminal pane (editor, files, git, browser, ... never receive).
  terminal: boolean;
  /// Its tab is the shown one in its group (or it is not in a group).
  shown: boolean;
}

/// While a workspace is zoomed only the zoomed pane's group is on screen:
/// every other pane counts as not shown. `zoomed` = the ids of the zoomed
/// group's tabs (or just the zoomed pane); null = nothing zoomed.
export function applyZoom(panes: readonly BroadcastPane[], zoomed: ReadonlySet<Uuid> | null): BroadcastPane[] {
  if (!zoomed) return [...panes];
  return panes.map((p) => (zoomed.has(p.id) ? p : { ...p, shown: false }));
}

/// Panes that take part while broadcast is on: every terminal whose tab is
/// currently shown. Hidden tabs are not targets (decision: what the user
/// cannot see should not receive keystrokes).
export function broadcastParticipants(panes: readonly BroadcastPane[]): Uuid[] {
  return panes.filter((p) => p.terminal && p.shown).map((p) => p.id);
}

/// Where input typed in `source` is mirrored: the participants other than the
/// source. Empty when the source itself is not a participating terminal.
export function broadcastTargets(source: Uuid, panes: readonly BroadcastPane[]): Uuid[] {
  const all = broadcastParticipants(panes);
  if (!all.includes(source)) return [];
  return all.filter((id) => id !== source);
}

// Terminal replies that xterm emits through onData without a user action:
// focus reports, cursor position / device status / attribute reports, mode
// reports, mouse reports, and OSC/DCS answers. Mirroring them would feed one
// pane's answers to its siblings.
const AUTO_REPLY = [
  /^\x1b\[[IO]$/, // focus in / out
  /^\x1b\[\d+;\d+R$/, // cursor position
  /^\x1b\[\??\d*(;\d+)*n$/, // device status
  /^\x1b\[[?>=]?[\d;]*c$/, // device attributes
  /^\x1b\[\??[\d;]*\$y$/, // mode report
  /^\x1b\[<\d+;\d+;\d+[Mm]$/, // SGR mouse
  /^\x1b\[M[\s\S]{3}$/, // X10 mouse
  /^\x1b[\]P_^]/, // OSC / DCS / APC / PM replies
];

/// Whether `data` from xterm's onData is something the user did (typed text,
/// a control key, an arrow key) rather than the terminal answering a query.
/// Escape sequences are matched as a whole; a batch of several is not mirrored.
export function mirrorableInput(data: string): boolean {
  if (data.length === 0) return false;
  return !AUTO_REPLY.some((re) => re.test(data));
}

/// The per-workspace switch. Runtime-only: a fresh instance is all off.
export class BroadcastSwitch {
  private on = new Set<number>();

  isOn(wsId: number): boolean {
    return this.on.has(wsId);
  }

  /// Flip it; returns the new state.
  toggle(wsId: number): boolean {
    if (this.on.delete(wsId)) return false;
    this.on.add(wsId);
    return true;
  }

  off(wsId: number): void {
    this.on.delete(wsId);
  }
}
