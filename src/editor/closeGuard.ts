// The unsaved-changes close guard's decisions (spec §3.5), pure.
//
// `Pane.dispose()` has no veto, so every path that closes a pane asks the
// pane first (`Pane.canClose`). This module is the part of that which can
// be wrong in a way a test catches: *whether* to ask, *what* to offer, and
// what the answer means. The dialogs and the saves are the pane's and the
// manager's; they only carry these results out.
//
// The rule every function here keeps: anything but an explicit Discard, or a
// Save that actually completed, means **do not close**. Esc, a click
// outside the dialog, Cancel, and a save that failed or hit a conflict the
// user then cancelled all leave the pane where it is.

export type CloseChoice = "save" | "discard" | "cancel";

/// One pane's close question. `null` = close without asking.
///
/// A dirty buffer with no file (untitled) cannot be saved from a close
/// prompt — there is nowhere to save it — so it is offered Discard/Cancel.
export function closeDecision(dirty: boolean, hasPath: boolean): CloseChoice[] | null {
  if (!dirty) return null;
  return hasPath ? ["save", "discard", "cancel"] : ["discard", "cancel"];
}

export interface Closable {
  /// What the prompt lists (the file name).
  name: string;
  dirty: boolean;
  hasPath: boolean;
}

export type ClosePlan =
  | { kind: "close" }
  | { kind: "ask"; names: string[]; choices: CloseChoice[] };

/// Closing several panes at once (a workspace, the window): **one** prompt
/// listing every dirty file, not one per pane. "Save" is offered only when
/// every dirty buffer has somewhere to go; otherwise the choice is between
/// discarding all and cancelling.
export function closePlan(items: readonly Closable[]): ClosePlan {
  const dirty = items.filter((i) => i.dirty);
  if (dirty.length === 0) return { kind: "close" };
  const canSave = dirty.every((i) => i.hasPath);
  return {
    kind: "ask",
    names: dirty.map((i) => i.name),
    choices: canSave ? ["save", "discard", "cancel"] : ["discard", "cancel"],
  };
}

/// What the user's answer means. `answer` is `null` for Esc / click-outside.
/// `saved` holds the outcome of each save the pane(s) attempted for a
/// "save" answer; the close proceeds only if there was at least one and
/// every one completed.
export function closeResult(answer: CloseChoice | null, saved: readonly boolean[] = []): boolean {
  if (answer === "discard") return true;
  if (answer === "save") return saved.length > 0 && saved.every(Boolean);
  return false;
}
