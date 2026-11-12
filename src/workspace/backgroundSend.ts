// Whether `sendToPane` should pull the user to the pane it sent to.
// A send aimed at another workspace than the one on screen must not yank the
// user away mid-typing; one inside the current workspace keeps focusing.

export function shouldFocusAfterSend(
  explicit: boolean | undefined,
  targetWs: number | null,
  activeWs: number,
): boolean {
  return explicit ?? (targetWs === null || targetWs === activeWs);
}
