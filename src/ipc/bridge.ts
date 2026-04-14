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

/// Call a Tauri command and surface its error as a plain `Error`.
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return (await tauriInvoke(cmd, args)) as T;
  } catch (e) {
    const msg = typeof e === "string" ? e : (e as Error)?.message ?? String(e);
    throw new Error(`${cmd}: ${msg}`);
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
export async function onPaneData(
  id: Uuid,
  handler: (data: Uint8Array) => void,
): Promise<UnlistenFn> {
  return tauriListen<number[]>(`pty:data:${id}`, (ev) => {
    handler(Uint8Array.from(ev.payload));
  });
}

/// Subscribe to the child exit event for a single pane.
export async function onPaneExit(
  id: Uuid,
  handler: (code: number) => void,
): Promise<UnlistenFn> {
  return tauriListen<number>(`pty:exit:${id}`, (ev) => handler(ev.payload));
}
