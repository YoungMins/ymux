// Switching auto-continue for a pane: the backend decides, the model mirrors.

import { api, onAutoAnswer } from "../ipc/bridge";
import type { Uuid } from "../types";
import { applyAutoContinue, isAutoContinue, recordAutoAnswer } from "./model";

export async function setAutoContinue(id: Uuid, enabled: boolean): Promise<void> {
  await api.setAutoContinue(id, enabled);
  applyAutoContinue(id, enabled);
}

export function toggleAutoContinue(id: Uuid): Promise<void> {
  return setAutoContinue(id, !isAutoContinue(id));
}

/// Feed the backend's answer events into the model. Call once at startup.
export function listenAutoAnswers(): Promise<() => void> {
  return onAutoAnswer(recordAutoAnswer);
}
