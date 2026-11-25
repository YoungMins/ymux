// Remote server: pushing the pane list when its content changes. A change
// source calls `poke()` as often as it likes; the list is recomputed at most
// once per `delayMs` window and sent only when its JSON differs from the last
// one sent. Pure (timers injectable) so the debounce is unit-tested.

import type { RemotePane } from "../types";

export interface PublisherDeps {
  compute: () => RemotePane[];
  send: (panes: RemotePane[]) => Promise<void>;
  /// False while the remote connection is off or unavailable: nothing is
  /// computed or sent.
  active: () => boolean;
  delayMs?: number;
  now?: () => number;
  setTimer?: (fn: () => void, ms: number) => unknown;
  clearTimer?: (t: unknown) => void;
}

export const PUBLISH_DEBOUNCE_MS = 500;

export interface Publisher {
  poke(): void;
  /// Like `poke`, but when nothing was computed for `delayMs` the list is
  /// computed right here, without a timer: a hidden window's timers are
  /// throttled, its events are not. Inside the window it is a `poke`.
  pokeNow(): void;
  /// Forget what was sent (a new connection has not seen it) and poke.
  invalidate(): void;
  dispose(): void;
}

export function createPublisher(deps: PublisherDeps): Publisher {
  const delay = deps.delayMs ?? PUBLISH_DEBOUNCE_MS;
  const setT = deps.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
  const clearT = deps.clearTimer ?? ((t) => clearTimeout(t as ReturnType<typeof setTimeout>));
  const now = deps.now ?? Date.now;
  let timer: unknown = null;
  let last: string | null = null;
  let disposed = false;
  let firedAt = Number.NEGATIVE_INFINITY;

  const fire = (): void => {
    timer = null;
    if (disposed || !deps.active()) return;
    firedAt = now();
    let panes: RemotePane[];
    try {
      panes = deps.compute();
    } catch (e) {
      console.warn("remote pane list failed:", e);
      return;
    }
    const json = JSON.stringify(panes);
    if (json === last) return;
    last = json;
    deps.send(panes).catch(() => {
      // Not delivered: the next change (or reconnect) sends it again.
      if (last === json) last = null;
    });
  };

  const poke = (): void => {
    if (disposed || timer !== null) return;
    timer = setT(fire, delay);
  };

  const pokeNow = (): void => {
    if (disposed) return;
    if (now() - firedAt >= delay) {
      // A timer still pending past its time was throttled: do its work now.
      if (timer !== null) clearT(timer);
      fire();
    } else poke();
  };

  return {
    poke,
    pokeNow,
    invalidate(): void {
      last = null;
      poke();
    },
    dispose(): void {
      disposed = true;
      if (timer !== null) clearT(timer);
      timer = null;
    },
  };
}
