// "Someone sent work to a session": raised by `WorkspaceManager.sendToPane`,
// the one funnel every cross-pane send goes through (fan-out prompts, diff
// review comments, the fan-out compare's commit ask). The office view turns
// each into a character walking over to deliver it. Nothing else listens.

import type { Uuid } from "../types";

export type OfficeMessageKind = "prompt" | "review" | "commit";

export interface OfficeMessage {
  /// The pane the work came from, or null when the user sent it (fan-out,
  /// a review typed in the diff overlay).
  from: Uuid | null;
  to: Uuid;
  kind: OfficeMessageKind;
}

type Listener = (m: OfficeMessage) => void;
const listeners = new Set<Listener>();

export function onOfficeMessage(cb: Listener): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

export function emitOfficeMessage(m: OfficeMessage): void {
  for (const cb of listeners) {
    try {
      cb(m);
    } catch (e) {
      console.warn("office message listener failed:", e);
    }
  }
}
