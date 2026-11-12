// Which panes' accounts are near / at their usage limit (with another account
// available), for the title-row chip. Runtime-only module state like the
// auto-continue mirror: `WorkspaceManager` computes the marks (quotaModel) and
// the chip (badge.ts) reads them; a click on the chip asks for the switch.

import type { Uuid } from "../types";

export interface QuotaMark {
  level: "near" | "limit";
  /// Used percent of the tightest window.
  used: number;
}

let marks: ReadonlyMap<Uuid, QuotaMark> = new Map();
let onSwitch: ((paneId: Uuid) => void) | null = null;

export function quotaMarkOf(paneId: Uuid): QuotaMark | undefined {
  return marks.get(paneId);
}

/// Replace all marks; true when something differs from before.
export function setQuotaMarks(next: ReadonlyMap<Uuid, QuotaMark>): boolean {
  let changed = next.size !== marks.size;
  if (!changed) {
    for (const [id, m] of next) {
      const old = marks.get(id);
      if (!old || old.level !== m.level || Math.round(old.used) !== Math.round(m.used)) {
        changed = true;
        break;
      }
    }
  }
  marks = next;
  return changed;
}

export function setQuotaSwitchHandler(fn: ((paneId: Uuid) => void) | null): void {
  onSwitch = fn;
}

export function requestQuotaSwitch(paneId: Uuid): void {
  onSwitch?.(paneId);
}
