// TypeScript mirror of the Rust serde model in src-tauri/src/config/model.rs.
// Keep in sync when Rust structs change. Types use snake_case fields to match
// the TOML schema, except BootstrapPayload which uses camelCase because
// commands.rs renames it via #[serde(rename_all = "camelCase")].

export type Uuid = string;

export interface ShellProfile {
  name: string;
  executable: string;
  args: string[];
  icon?: string | null;
  color?: string | null;
}

export type PaneKind = "terminal" | "browser" | "native_browser";

export interface HotKeyDef {
  label: string;
  command: string;
  batch?: boolean;
}

export interface PaneSpec {
  id: Uuid;
  title?: string | null;
  shell: string;
  cwd?: string | null;
  startup_cmd?: string | null;
  env: [string, string][];
  /// Renamed from `kind` on the Rust side to avoid colliding with the
  /// `LayoutNode` tagged-enum discriminator (also called `kind`).
  pane_kind?: PaneKind;
  url?: string | null;
  hotkeys?: HotKeyDef[];
  bg_color?: string | null;
}

export type SplitDir = "horizontal" | "vertical";

export type LayoutNode =
  | ({ kind: "pane" } & PaneSpec)
  | { kind: "split"; direction: SplitDir; ratio: number; a: LayoutNode; b: LayoutNode }
  | { kind: "tabs"; active: number; children: LayoutNode[] };

export interface Workspace {
  id: number;
  name: string;
  root: LayoutNode;
}

export interface Config {
  version: number;
  active_workspace: number;
  shells: ShellProfile[];
  workspaces: Workspace[];
}

export interface BootstrapPayload {
  config: Config;
  shells: ShellProfile[];
  configPath: string;
}

export interface SpawnedPane {
  id: Uuid;
  shell: string;
}

/// UUID v4 generator that doesn't need a `crypto` subtle fallback polyfill.
export function uuidv4(): Uuid {
  // Prefer `crypto.randomUUID` — available in WebView2 and all modern browsers.
  const c = (globalThis as unknown as { crypto?: Crypto }).crypto;
  if (c && typeof c.randomUUID === "function") {
    return c.randomUUID();
  }
  // Fallback: RFC 4122 v4 from crypto.getRandomValues.
  const bytes = new Uint8Array(16);
  if (c && typeof c.getRandomValues === "function") {
    c.getRandomValues(bytes);
  } else {
    for (let i = 0; i < 16; i++) bytes[i] = Math.floor(Math.random() * 256);
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0"));
  return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex.slice(6, 8).join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10, 16).join("")}`;
}
