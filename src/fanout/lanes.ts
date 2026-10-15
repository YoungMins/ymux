export interface Lane {
  agent: string;
  account: string;
}

export const MAX_LANES = 8;
export const MAX_PROMPT_BYTES = 32 * 1024;

export type LaneError = "lanes" | "prompt-empty" | "prompt-too-long";

/// One path segment of git-safe characters; `fallback` when nothing survives.
function seg(s: string, fallback: string): string {
  const out = s
    .replace(/[^A-Za-z0-9._-]+/g, "-")
    .replace(/\.{2,}/g, "-")
    .replace(/-{2,}/g, "-")
    .replace(/^[-.]+/, "")
    .replace(/(\.lock|[-.])+$/g, "");
  return out || fallback;
}

function prefixOf(prefix: string): string {
  const parts = prefix
    .split("/")
    .map((p) => seg(p, ""))
    .filter(Boolean);
  return parts.length ? parts.join("/") : "fanout";
}

/// The run's short tag in branch names: the last 6 alphanumerics of the run
/// id (minute + random part), so two fan-outs never share a branch name.
export function runShort(runId: string): string {
  return seg(runId.replace(/[^A-Za-z0-9]/g, "").slice(-6), "run");
}

/// Unique, valid branch names `<prefix>/<runShort>-<agent>-<n>` (n = 1-based
/// lane index). A name already in `existing` (local branches) is bumped with
/// a `-k` suffix until free.
export function laneBranches(
  prefix: string,
  runId: string,
  lanes: Lane[],
  existing: Iterable<string> = [],
): string[] {
  const pre = prefixOf(prefix);
  const run = runShort(runId);
  const used = new Set<string>(existing);
  return lanes.map((l, i) => {
    const base = `${pre}/${run}-${seg(l.agent, "agent")}-${i + 1}`;
    let name = base;
    for (let k = 2; used.has(name); k++) name = `${base}-${k}`;
    used.add(name);
    return name;
  });
}

/// `yyyymmdd-hhmm-xxxx` in local time; `rand` is a [0,1) source.
export function newRunId(now: Date, rand: () => number): string {
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  const date = `${p(now.getFullYear(), 4)}${p(now.getMonth() + 1)}${p(now.getDate())}`;
  const r = Math.max(0, Math.min(Math.floor(rand() * 36 ** 4), 36 ** 4 - 1));
  return `${date}-${p(now.getHours())}${p(now.getMinutes())}-${r.toString(36).padStart(4, "0")}`;
}

export function validateLanes(lanes: Lane[], prompt: string): LaneError | null {
  if (lanes.length < 1 || lanes.length > MAX_LANES) return "lanes";
  if (prompt.trim() === "") return "prompt-empty";
  if (new TextEncoder().encode(prompt).length > MAX_PROMPT_BYTES) return "prompt-too-long";
  return null;
}
