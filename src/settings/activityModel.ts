// Pure helpers behind Settings -> "Activity & notifications": provider
// defaults, the attachment type groups, destination defaults and the
// "recent turns" projection. No DOM, no IPC, so vitest covers it directly.

import type {
  ActivityConfig,
  ActivityRecord,
  SummarizerConfig,
  WebhookDestination,
} from "../types";
import { uuidv4 } from "../types";

/// Attachment caps the backend clamps to (webhooks/mod.rs MAX_ATTACH_*);
/// webhookLimits.test.ts keeps them equal.
export const ATTACH_LIMITS = { maxFileMb: 25, maxTotalMb: 25, maxFiles: 10 } as const;

export const SUMMARIZER_PROVIDERS = ["rule", "ollama", "openai"] as const;
export type SummarizerProvider = (typeof SUMMARIZER_PROVIDERS)[number];

export const OLLAMA_ENDPOINT = "http://127.0.0.1:11434";
/// LM Studio's default local server; also a sane base for most
/// OpenAI-compatible local runtimes.
export const OPENAI_LOCAL_ENDPOINT = "http://127.0.0.1:1234/v1";

/// The endpoint a provider starts with. "rule" never calls out, so it keeps
/// the Ollama default (the same value the backend config defaults to).
export function defaultEndpoint(provider: string): string {
  return provider === "openai" ? OPENAI_LOCAL_ENDPOINT : OLLAMA_ENDPOINT;
}

/// Endpoint after the provider select changes: a blank field or one still
/// holding the previous provider's default follows the new provider; anything
/// the user typed is kept.
export function endpointOnProviderChange(
  prevProvider: string,
  prevEndpoint: string,
  nextProvider: string,
): string {
  const cur = prevEndpoint.trim();
  if (cur === "" || cur === defaultEndpoint(prevProvider)) return defaultEndpoint(nextProvider);
  return prevEndpoint;
}

export const WEBHOOK_FORMATS = ["json", "slack", "discord", "teams"] as const;
export const WEBHOOK_EVENTS = ["turn_end", "attention"] as const;

export interface AttachGroup {
  id: string;
  /// Lowercase extensions without the dot; the vocabulary of
  /// `WebhookDestination.attach_types` (see `attach::type_of` in Rust).
  exts: readonly string[];
}

export const ATTACH_GROUPS: readonly AttachGroup[] = [
  { id: "images", exts: ["png", "jpg", "jpeg", "gif", "webp"] },
  { id: "pdf", exts: ["pdf"] },
  { id: "audio", exts: ["mp3", "wav", "ogg", "oga", "opus", "m4a"] },
  { id: "video", exts: ["mp4", "webm"] },
  { id: "text", exts: ["md", "txt", "csv"] },
  { id: "office", exts: ["docx", "xlsx", "pptx"] },
];

/// A group counts as on only when every one of its extensions is allowed.
export function groupEnabled(types: readonly string[], group: AttachGroup): boolean {
  return group.exts.every((e) => types.includes(e));
}

/// Turn a whole group on or off, leaving other extensions (including ones
/// this UI does not know) untouched and keeping the list free of duplicates.
export function setGroup(types: readonly string[], group: AttachGroup, on: boolean): string[] {
  const rest = types.filter((e) => !group.exts.includes(e));
  return on ? [...rest, ...group.exts] : rest;
}

export function defaultActivity(language: string): ActivityConfig {
  return {
    enabled: false,
    retention_days: 14,
    max_total_mb: 200,
    record_prompts: false,
    report_dir: ".ymux/report",
    language,
  };
}

export function defaultSummarizer(): SummarizerConfig {
  return {
    provider: "rule",
    endpoint: OLLAMA_ENDPOINT,
    model: "",
    timeout_secs: 20,
    max_input_chars: 8000,
    allow_remote: false,
  };
}

/// A fresh destination. Disabled until the user has given it a URL.
export function newDestination(name: string): WebhookDestination {
  return {
    id: uuidv4(),
    name,
    enabled: false,
    format: "json",
    url_hint: "",
    events: ["turn_end", "attention"],
    detail: "summary",
    include_prompts: false,
    debounce_secs: 10,
    attach: false,
    attach_types: [],
    max_file_mb: 8,
    max_total_mb: 8,
    max_files: 4,
  };
}

/// Integer in [min, max]; non-numeric input falls back to `fallback`.
export function clampInt(raw: string | number, min: number, max: number, fallback: number): number {
  const n = typeof raw === "number" ? raw : Number.parseInt(raw, 10);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, Math.trunc(n)));
}

export interface RecentTurn {
  id: string;
  ts: number;
  ws: number;
  pane: string;
  agent: string;
  headline: string;
}

/// The newest `limit` `turn_end` records, newest first. `read()` returns
/// oldest-first and mixes every record kind.
export function recentTurns(records: readonly ActivityRecord[], limit: number): RecentTurn[] {
  const out: RecentTurn[] = [];
  for (let i = records.length - 1; i >= 0 && out.length < limit; i--) {
    const r = records[i];
    if (r.kind !== "turn_end") continue;
    const summary = (r.data as { summary?: { headline?: unknown } } | null)?.summary;
    const headline = typeof summary?.headline === "string" ? summary.headline : "";
    out.push({ id: r.id, ts: r.ts, ws: r.ws, pane: r.pane, agent: r.agent, headline });
  }
  return out;
}

/// "12.3 KB" style size for the preview's attachment list.
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "?";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/// A report folder must stay inside the repository: relative, no drive or
/// root prefix, no `..` segment. Empty is fine (the caller substitutes the default).
export function isRelativeReportDir(raw: string): boolean {
  const p = raw.trim().replaceAll("\\", "/");
  if (p === "") return true;
  if (p.startsWith("/") || p.startsWith("~") || /^[a-zA-Z]:/.test(p)) return false;
  return !p.split("/").some((seg) => seg === "..");
}
