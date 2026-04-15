// Typed wrappers around Tauri's `invoke` + `listen` so the rest of the app
// never touches the raw IPC surface. This also makes it trivial to swap in a
// mock during browser-only development.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  BootstrapPayload,
  Config,
  ShellProfile,
  SpawnedPane,
  Uuid,
} from "../types";

export interface SpawnArgs {
  id: Uuid;
  shell: string;
  cwd?: string | null;
  rows: number;
  cols: number;
}

export interface ResizeArgs {
  id: Uuid;
  rows: number;
  cols: number;
  pixelWidth: number;
  pixelHeight: number;
}

/// Best-effort conversion of any thrown / rejected value into a human
/// readable string. Tauri can reject with strings, plain objects, Errors,
/// or `undefined` (the last one happens when a permission is denied without a
/// payload). Always returning *something* keeps the on-screen error from
/// turning into the literal text "undefined".
export function describeError(e: unknown): string {
  if (e == null) return "unknown error (no payload)";
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message || e.name || "Error";
  if (typeof e === "object") {
    const obj = e as Record<string, unknown>;
    if (typeof obj.message === "string") return obj.message;
    if (typeof obj.kind === "string" && typeof obj.detail === "string") {
      return `${obj.kind}: ${obj.detail}`;
    }
    try {
      return JSON.stringify(e);
    } catch {
      return Object.prototype.toString.call(e);
    }
  }
  return String(e);
}

/// Call a Tauri command and surface its error as a plain `Error`.
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return (await tauriInvoke(cmd, args)) as T;
  } catch (e) {
    throw new Error(`${cmd}: ${describeError(e)}`);
  }
}

/// Wrap a `tauriListen` call so that listen failures (typically capability /
/// permission denials in Tauri 2) surface as proper Errors instead of bare
/// undefined rejections.
async function safeListen<T>(
  channel: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  try {
    return await tauriListen<T>(channel, (ev) => handler(ev.payload));
  } catch (e) {
    throw new Error(`listen ${channel}: ${describeError(e)}`);
  }
}

export const api = {
  loadBootstrap: (): Promise<BootstrapPayload> => call("load_bootstrap"),

  detectShells: (): Promise<ShellProfile[]> => call("detect_shells_cmd"),

  saveConfig: (config: Config): Promise<void> =>
    call("save_config", { config }),

  spawnPane: (args: SpawnArgs): Promise<SpawnedPane> =>
    call("spawn_pane", { args }),

  writePane: (id: Uuid, data: Uint8Array): Promise<void> =>
    call("write_pane", { args: { id, data: Array.from(data) } }),

  resizePane: (args: ResizeArgs): Promise<void> =>
    call("resize_pane", { args }),

  killPane: (id: Uuid): Promise<void> => call("kill_pane", { id }),

  setActiveWorkspace: (id: number): Promise<void> =>
    call("set_active_workspace", { id }),
};

/// Subscribe to PTY stdout for a single pane. Returns an unlisten handle.
export function onPaneData(
  id: Uuid,
  handler: (data: Uint8Array) => void,
): Promise<UnlistenFn> {
  return safeListen<number[]>(`pty:data:${id}`, (payload) => {
    handler(Uint8Array.from(payload));
  });
}

/// Subscribe to the child exit event for a single pane.
export function onPaneExit(
  id: Uuid,
  handler: (code: number) => void,
): Promise<UnlistenFn> {
  return safeListen<number>(`pty:exit:${id}`, handler);
}
