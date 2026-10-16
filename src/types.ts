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
  /// Env vars applied to the PTY before the pane's own `env`. Used by the
  /// macOS zsh profile to point ZDOTDIR at ymux's shell-integration shim.
  env?: [string, string][];
}

/// `files` is a GUI file manager with no PTY; its directory is `cwd`.
/// `editor` is a GUI text editor with no PTY; its file is `file_path`.
/// `git` is a GUI log / branch / worktree view with no PTY; its repo is found from `cwd`.
export type PaneKind =
  | "terminal"
  | "browser"
  | "native_browser"
  | "embedded_browser"
  | "files"
  | "editor"
  | "git"
  | "token_usage";

export interface HotKeyDef {
  label: string;
  command: string;
  batch?: boolean;
}

export interface PaneSpec {
  id: Uuid;
  title?: string | null;
  /// A ShellProfile name. "" is a sentinel ("the default shell") that the
  /// backend pins to a concrete name at boot (`Config::pin_pane_shells` in
  /// load_bootstrap), so a restored terminal pane keeps its shell even if
  /// `default_shell` changes later.
  shell: string;
  cwd?: string | null;
  startup_cmd?: string | null;
  env: [string, string][];
  /// Renamed from `kind` on the Rust side to avoid colliding with the
  /// `LayoutNode` tagged-enum discriminator (also called `kind`).
  pane_kind?: PaneKind;
  url?: string | null;
  hotkeys?: HotKeyDef[];
  bg_color?: string;
  worktree_path?: string;
  /// The file an `editor` pane has open. Empty = untitled. A plain string,
  /// not `Option`, for the TOML reason rule 3 gives (see model.rs).
  file_path?: string;
  /// An `AccountProfile.id`. "" = inherit the workspace's account; "none" =
  /// no account even if the workspace has one. Plain string for the TOML
  /// reason rule 3 gives.
  account?: string;
}

/// One AI-CLI account: where Claude Code (`CLAUDE_CONFIG_DIR`) and Codex
/// (`CODEX_HOME`) keep their login. An empty dir leaves that tool on its
/// default. Mirrors `AccountProfile` in src-tauri/src/config/model.rs.
export interface AccountProfile {
  id: string;
  name: string;
  claude_config_dir: string;
  codex_home: string;
  /// "#rrggbb" or "".
  color: string;
}

export type SplitDir = "horizontal" | "vertical";

export type LayoutNode =
  | ({ kind: "pane" } & PaneSpec)
  | { kind: "split"; direction: SplitDir; ratio: number; a: LayoutNode; b: LayoutNode }
  /// `id` mirrors `LayoutNode::Tabs.id` in src-tauri/src/config/model.rs: the
  /// stable handle the renderer caches a `PaneGroup` under and the file dock
  /// registers its viewer tab against.
  | { kind: "tabs"; id: Uuid; active: number; children: LayoutNode[] };

export interface Workspace {
  id: number;
  name: string;
  root: LayoutNode;
  /// Default `AccountProfile.id` for the workspace's panes; "" = none.
  account?: string;
}

export interface Config {
  version: number;
  active_workspace: number;
  shells: ShellProfile[];
  workspaces: Workspace[];
  notify_on_bell: boolean;
  persist_scrollback: boolean;
  /// Draw short terminal content against the pane's bottom edge (the prompt
  /// sits on the last row). See `src/terminal/bottomAnchor.ts`.
  bottom_anchor: boolean;
  paste_image_retention_hours: number;
  worktree_base_dir: string;
  /// Terminal font size in CSS pixels, shared by every pane.
  font_size?: number;
  /// `ShellProfile.name` used for new panes and workspaces. Empty (or a name
  /// that no longer exists) means "the first detected shell".
  default_shell: string;
  /// Claude Code hooks installed in ~/.claude/settings.json (agent tree).
  /// Optional: absent in configs written before the setting existed.
  agent_tracking?: boolean;
  /// Account profiles panes and workspaces can pick. Absent in older configs.
  accounts?: AccountProfile[];
  /// Activity log settings; absent in older configs (backend defaults apply).
  activity?: ActivityConfig;
  /// Turn-summary provider settings; absent in older configs.
  summarizer?: SummarizerConfig;
  /// Webhook destinations; absent in older configs. URL and secret are never here.
  webhooks?: WebhookDestination[];
}

/// Mirror of `ActivityConfig` in `config/model.rs`.
export interface ActivityConfig {
  enabled: boolean;
  retention_days: number;
  max_total_mb: number;
  record_prompts: boolean;
  report_dir: string;
  /// UI language code; the backend uses it for the rule-based summary.
  language: string;
}

/// One line of the activity log (`activity::store::Record`). `kind` is
/// "prompt" | "tool" | "permission" | "turn_end" | "summary"; `data` depends on it.
export interface ActivityRecord {
  v: number;
  id: string;
  /// Unix milliseconds.
  ts: number;
  ws: number;
  pane: Uuid;
  agent: string;
  account: string;
  session: string;
  turn: string;
  cwd: string;
  branch: string;
  kind: string;
  data: Record<string, unknown>;
}

/// Mirror of `webhooks::WebhookDestination`. `id` is a uuidv4 made by the
/// frontend (the backend drops anything else). `url_hint` (scheme+host) is
/// computed by the backend; whatever the frontend sends is overwritten.
export interface WebhookDestination {
  id: string;
  name: string;
  enabled: boolean;
  /// "json" | "slack" | "discord" | "teams".
  format: string;
  url_hint: string;
  /// "turn_end" | "attention".
  events: string[];
  /// "summary" | "full".
  detail: string;
  include_prompts: boolean;
  debounce_secs: number;
  attach: boolean;
  attach_types: string[];
  max_file_mb: number;
  max_total_mb: number;
  max_files: number;
}

/// Mirror of `delivery::DestStatus`: kinds and counts only.
export interface WebhookDestStatus {
  queued: number;
  last_ok_at: number | null;
  last_error: string | null;
  dropped: number;
}

/// Mirror of `runtime::WebhookPreview`. `payload` is the JSON body that would
/// be sent (no URL, secret or signature).
export interface WebhookPreview {
  format: string;
  payload: unknown;
  attachments: { name: string; size: number; mime: string }[];
  listed: { name: string; size: number; reason: string }[];
}

/// Mirror of `SummarizerConfig`. `provider`: "rule" | "ollama" | "openai".
export interface SummarizerConfig {
  provider: string;
  endpoint: string;
  model: string;
  timeout_secs: number;
  max_input_chars: number;
  allow_remote: boolean;
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

/// Agent-tree snapshot — mirror of `src-tauri/src/agents.rs` (serde
/// lowercase enums, snake_case fields).
export type AgentStatus = "working" | "waiting" | "done" | "idle";
export type AgentSource = "hook" | "process";

export interface Agent {
  kind: string;
  status: AgentStatus;
  source: AgentSource;
  tool: string | null;
}

export interface Subagent {
  id: string;
  agent_type: string;
  status: AgentStatus;
}

export interface PaneAgents {
  lead: Agent | null;
  subagents: Subagent[];
}

/// Pane id → agents. Panes with no agents are absent.
export type AgentSnapshot = Record<Uuid, PaneAgents>;

/// A path the backend resolved and confirmed exists (mirrors
/// `src-tauri/src/fspath.rs::ResolvedPath`). Produced by `resolve_paths`
/// for the terminal's path linkifier.
export interface ResolvedPath {
  /// Absolute, with Windows' verbatim `\?\` prefix stripped.
  absolute: string;
  /// Directories open in the OS file manager rather than an application.
  is_dir: boolean;
}

/// An installed agent CLI for the top bar's "+" launcher (mirrors
/// `src-tauri/src/agent_launch.rs::DetectedAgent`, returned by `detect_agents`).
export interface DetectedAgent {
  id: string;
  name: string;
  /// Bare executable name, typed as-is when the agent is on PATH.
  program: string;
  /// Absolute path, set only when the agent is not on PATH.
  path: string | null;
  /// Empty when the agent has no bypass flag (opencode).
  bypass_args: string[];
  /// An extra tooltip caveat, as an i18n key.
  note: string | null;
  /// Only a `.ps1` shim was found: runnable from PowerShell only.
  powershell_only: boolean;
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

/// A Claude config dir whose hooks could not be written or removed by
/// `set_agent_tracking`. Mirrors `HookDirIssue` in src-tauri/src/agent_hooks.rs.
export interface HookDirIssue {
  dir: string;
  error: string;
}
