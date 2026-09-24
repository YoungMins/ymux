// Typed wrappers around Tauri's `invoke` + `listen` so the rest of the app
// never touches the raw IPC surface. This also makes it trivial to swap in a
// mock during browser-only development.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  AgentSnapshot,
  BootstrapPayload,
  Config,
  ResolvedPath,
  ShellProfile,
  SpawnedPane,
  Uuid,
} from "../types";
import type { YTheme, ConfigPathKind } from "../settings/types";
import type { ResumeOutcome } from "../terminal/resumePlan";
import type { FileEntry } from "../files/fileModel";

export interface SpawnArgs {
  id: Uuid;
  shell: string;
  cwd?: string | null;
  rows: number;
  cols: number;
  /// Run this program directly instead of `shell` (see `SpawnArgs.argv` in
  /// commands.rs). Used by the file dock for `ydir --dock <dir>`.
  argv?: string[];
}

export interface ResizeArgs {
  id: Uuid;
  rows: number;
  cols: number;
  pixelWidth: number;
  pixelHeight: number;
}

/// A single entry from `git worktree list --porcelain` (mirrors
/// `src-tauri/src/git/mod.rs::WorktreeEntry`).
export interface WorktreeEntry {
  path: string;
  branch: string;
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

/// A rejected fs/git command, keeping the backend's machine-readable `kind`
/// (`YmuxError::kind()` in src-tauri/src/error.rs: `not_found`,
/// `permission_denied`, `already_exists`, …). `call()` flattens errors to a
/// string, which is fine for a log line but not for a pane that has to tell
/// "already exists" (ask to overwrite) from "permission denied" (say so).
export class YmuxCallError extends Error {
  constructor(
    readonly cmd: string,
    readonly kind: string,
    message: string,
  ) {
    super(message);
    this.name = "YmuxCallError";
  }
}

/// The backend error kind of `e`, or `"other"` for anything that is not a
/// `YmuxCallError`.
export function errorKind(e: unknown): string {
  return e instanceof YmuxCallError ? e.kind : "other";
}

/// Like `call`, but rejects with a `YmuxCallError` carrying the error kind.
async function callKind<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return (await tauriInvoke(cmd, args)) as T;
  } catch (e) {
    const kind =
      e && typeof e === "object" && typeof (e as { kind?: unknown }).kind === "string"
        ? (e as { kind: string }).kind
        : "other";
    throw new YmuxCallError(cmd, kind, describeError(e));
  }
}

/// A bounded directory look for the preview (`fsops::DirPeek`).
export interface DirPeek {
  entries: { name: string; is_dir: boolean }[];
  more: boolean;
}

/// The guarded filesystem surface (src-tauri/src/fsops.rs). Every call
/// rejects with a `YmuxCallError`. Paths are raw strings: open them, never
/// compare them (rule 15).
export const fsApi = {
  listDir: (path: string, showHidden: boolean): Promise<FileEntry[]> =>
    callKind("fs_list_dir", { path, showHidden }),
  roots: (): Promise<string[]> => callKind("fs_roots"),
  homeDir: (): Promise<string> => callKind("fs_home_dir"),
  stat: (path: string): Promise<FileEntry> => callKind("fs_stat", { path }),
  createDir: (path: string): Promise<void> => callKind("fs_create_dir", { path }),
  createFile: (path: string): Promise<void> => callKind("fs_create_file", { path }),
  rename: (from: string, to: string): Promise<void> => callKind("fs_rename", { from, to }),
  copy: (from: string, to: string, overwrite: boolean): Promise<void> =>
    callKind("fs_copy", { from, to, overwrite }),
  move: (from: string, to: string, overwrite: boolean): Promise<void> =>
    callKind("fs_move", { from, to, overwrite }),
  delete: (paths: string[], toTrash: boolean): Promise<void> =>
    callKind("fs_delete", { paths, toTrash }),
  /// At most 64 KiB from the start of a file, as raw bytes.
  readHead: async (path: string, maxBytes: number): Promise<Uint8Array> => {
    const buf = await callKind<ArrayBuffer | number[]>("fs_read_head", { path, maxBytes });
    return buf instanceof ArrayBuffer ? new Uint8Array(buf) : Uint8Array.from(buf);
  },
  peekDir: (path: string, showHidden: boolean): Promise<DirPeek> =>
    callKind("fs_peek_dir", { path, showHidden }),
  reveal: (path: string): Promise<void> => callKind("fs_reveal", { path }),
  /// Open with the OS default app; executables are revealed, never run.
  openDefault: (path: string): Promise<void> => callKind("fs_open_default", { path }),
};

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

  /// Show an OS desktop notification with the given title and body.
  notify: (title: string, body: string): Promise<void> =>
    call("notify", { title, body }),

  /// Get the most recently reported working directory for a pane (via OSC 7).
  /// Returns `null` if the pane has not yet emitted a cwd sequence.
  getPaneCwd: (id: Uuid): Promise<string | null> =>
    call("get_pane_cwd", { id }),

  /// Open a URL in the system default browser. Only http/https are allowed.
  openUrl: (url: string): Promise<void> => call("open_url", { url }),

  /// Which of these terminal-output path candidates actually exist,
  /// resolved against `cwd`. Answers align with `paths` by index; `null`
  /// means "not a path we will link". One call per hovered row — see
  /// `src/terminal/pathProbe.ts` for the cache in front of it.
  resolvePaths: (
    paths: readonly string[],
    cwd: string | null,
  ): Promise<Array<ResolvedPath | null>> =>
    call("resolve_paths", { paths, cwd }),

  /// Open an absolute local path with the OS default handler. Directories
  /// open in the file manager; executables and scripts are revealed there
  /// rather than run. Rejected unless the path is absolute and exists.
  openPath: (path: string): Promise<void> => call("open_path", { path }),

  /// Create a native child webview window positioned over a layout placeholder.
  createWebview: (
    id: string,
    url: string,
    x: number,
    y: number,
    width: number,
    height: number,
    userAgent?: string,
  ): Promise<void> => call("create_webview", { id, url, x, y, width, height, user_agent: userAgent ?? null }),

  /// Destroy (close) a native child webview.
  destroyWebview: (id: string): Promise<void> =>
    call("destroy_webview", { id }),

  /// Navigate an existing native child webview to a new URL.
  navigateWebview: (id: string, url: string): Promise<void> =>
    call("navigate_webview", { id, url }),

  /// Set the CSS zoom level of a native child webview (1.0 = 100%).
  zoomWebview: (id: string, factor: number): Promise<void> =>
    call("zoom_webview", { id, factor }),

  /// Reposition and resize an existing native child webview.
  resizeWebview: (
    id: string,
    x: number,
    y: number,
    width: number,
    height: number,
  ): Promise<void> => call("resize_webview", { id, x, y, width, height }),

  /// Show or hide a native child webview (used to keep ymux popups visible
  /// over browser panes — the child window otherwise paints above all HTML).
  setWebviewVisible: (id: string, visible: boolean): Promise<void> =>
    call("set_webview_visible", { id, visible }),

  /// Create an embedded child webview (Window::add_child). Position and size
  /// are in physical pixels relative to the main window's content area.
  createEmbeddedBrowser: (
    id: string,
    url: string,
    x: number,
    y: number,
    width: number,
    height: number,
  ): Promise<void> => call("create_embedded_browser", { id, url, x, y, width, height }),

  /// Destroy (close) an embedded child webview.
  destroyEmbeddedBrowser: (id: string): Promise<void> =>
    call("destroy_embedded_browser", { id }),

  /// Navigate an embedded child webview to a new URL.
  navigateEmbeddedBrowser: (id: string, url: string): Promise<void> =>
    call("navigate_embedded_browser", { id, url }),

  /// Reposition and resize an embedded child webview. Coordinates are in
  /// physical pixels relative to the main window's content area.
  setEmbeddedBrowserBounds: (
    id: string,
    x: number,
    y: number,
    width: number,
    height: number,
  ): Promise<void> => call("set_embedded_browser_bounds", { id, x, y, width, height }),

  /// Show or hide an embedded child webview. `visible: false` moves it
  /// off-screen and shrinks to 1×1; `visible: true` is a no-op (the
  /// frontend must re-emit real bounds via setEmbeddedBrowserBounds).
  setEmbeddedBrowserVisible: (id: string, visible: boolean): Promise<void> =>
    call("set_embedded_browser_visible", { id, visible }),

  /// What a pane should do as it comes up: resume its agent, say that a
  /// recent session's transcript is gone, or nothing.
  /// Asked before `spawn()` decides whether to replay scrollback: a pane that
  /// resumes its agent skips the replay entirely (spec §4/§5). `startupCmd` is
  /// the pane's own saved startup command, so a selector it already carries
  /// can be stripped instead of fighting ours.
  getAgentSession: (
    id: Uuid,
    startupCmd?: string,
  ): Promise<ResumeOutcome | null> =>
    call("get_agent_session", { paneId: id, startupCmd: startupCmd ?? null }),

  /// Forget a pane's agent session. Called when the user closes a pane for
  /// good, mirroring `deleteScrollback`.
  clearAgentSession: (id: Uuid): Promise<void> =>
    call("clear_agent_session", { paneId: id }),

  /// Load the shared ymux/yCode color palette from `<config_dir>/theme.toml`.
  /// Returns the default Night Owl-inspired palette if no file exists yet.
  loadSyntaxTheme: (): Promise<YTheme> => call("load_syntax_theme"),

  /// Persist the palette back to `theme.toml`. Every y* TUI tool re-reads
  /// it on next launch — no live reload yet.
  saveSyntaxTheme: (theme: YTheme): Promise<void> =>
    call("save_syntax_theme", { theme }),

  /// Open the theme.toml file or the ymux config directory with the OS
  /// default app. Path is selected on the Rust side from a known set so
  /// the frontend can't smuggle in arbitrary paths.
  openConfigPath: (kind: ConfigPathKind): Promise<void> =>
    call("open_config_path", { kind }),

  /// Persist `blob` (serialized terminal scrollback) for a pane to disk.
  saveScrollback: (id: Uuid, blob: string): Promise<void> =>
    call("save_scrollback", { paneId: id, blob }),

  /// Load the persisted scrollback for a pane, or "" if none was saved.
  loadScrollback: (id: Uuid): Promise<string> =>
    call("load_scrollback", { paneId: id }),

  /// Delete the persisted scrollback for a pane, if any.
  deleteScrollback: (id: Uuid): Promise<void> =>
    call("delete_scrollback", { paneId: id }),

  /// If the OS clipboard holds an image, save it as a PNG and return that
  /// file's path; `null` means there is no image and the caller should paste
  /// text instead. Rejects when an image was found but could not be saved.
  ///
  /// Reading the clipboard in Rust rather than with `navigator.clipboard.read()`
  /// is the fix for pasted screenshots landing on disk as 0-byte PNGs: the
  /// webview's async image read handed us empty buffers.
  pasteClipboardImage: (): Promise<string | null> =>
    call("paste_clipboard_image"),

  /// Check whether `cwd` is inside a git repository.
  gitIsRepo: (cwd: string): Promise<boolean> => call("git_is_repo", { cwd }),

  /// Create a new git worktree for `branch` (based on `base`) under the
  /// repo rooted at `cwd`. Returns the absolute path of the new worktree.
  gitWorktreeAdd: (cwd: string, branch: string, base: string): Promise<string> =>
    call("git_worktree_add", { cwd, branch, base }),

  /// Remove a git worktree at `path`. `force` matches `git worktree remove --force`.
  gitWorktreeRemove: (path: string, force: boolean): Promise<void> =>
    call("git_worktree_remove", { path, force }),

  /// List all worktrees for the repo rooted at `cwd`.
  gitWorktreeList: (cwd: string): Promise<WorktreeEntry[]> =>
    call("git_worktree_list", { cwd }),

  /// Point the file dock's yDir at `path`. A no-op when it isn't running.
  fileDockChangeDir: (path: string): Promise<void> =>
    call("filedock_change_dir", { path }),

  /// Current agent-tree snapshot (pane id → agents).
  getAgents: (): Promise<AgentSnapshot> => call("get_agents"),

  /// Current tab labels (pane id → the deepest process running under it).
  getPaneLabels: (): Promise<Record<Uuid, string>> => call("get_pane_labels"),

  /// Install (true) or remove (false) ymux's Claude Code hooks in
  /// ~/.claude/settings.json and persist the setting.
  setAgentTracking: (enabled: boolean): Promise<void> =>
    call("set_agent_tracking", { enabled }),
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

/// Subscribe to a pane's working-directory changes. The backend emits these
/// only when the OSC 7 cwd actually changes.
export function onPaneCwd(
  id: Uuid,
  handler: (cwd: string) => void,
): Promise<UnlistenFn> {
  return safeListen<string>(`pty:cwd:${id}`, handler);
}

/// Subscribe to agent-tree snapshots pushed after every registry change.
export function onAgentsChanged(
  handler: (snapshot: AgentSnapshot) => void,
): Promise<UnlistenFn> {
  return safeListen<AgentSnapshot>("agents:changed", handler);
}

/// Subscribe to "open this file" requests from the file dock's yDir.
export function onOpenFile(handler: (path: string) => void): Promise<UnlistenFn> {
  return safeListen<string>("ymux:open-file", handler);
}

/// Subscribe to tab-label snapshots pushed by the 2 s process scan.
export function onPaneLabels(
  handler: (labels: Record<Uuid, string>) => void,
): Promise<UnlistenFn> {
  return safeListen<Record<Uuid, string>>("panes:labels", handler);
}
