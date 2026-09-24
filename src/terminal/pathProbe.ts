// Decides which path candidates are worth asking the backend about, caches
// the answers per pane, and resolves the overlaps the matcher deliberately
// leaves behind.
//
// Pure with respect to the outside world: the actual existence check is an
// injected async function, so everything here is testable with a fake probe
// and a call counter. The real probe is one `resolve_paths` IPC call.
//
// Why a cache at all: xterm asks a link provider for links on every hover
// that lands on a new row, and a row of build output can hold a dozen
// path-shaped tokens. Without memoisation, dragging the pointer down a
// screenful of `cargo` output would fire hundreds of stat calls, several of
// them on network drives.

import type { ResolvedPath } from "../types";
import type { PathCandidate } from "./pathMatch";

export type { ResolvedPath };

/// `null` means "checked, and it is not a file or directory we will link".
export type ProbeResult = ResolvedPath | null;

/// Resolve several candidate strings against `cwd` in one round trip.
export type ProbeFn = (
  texts: readonly string[],
  cwd: string | null,
) => Promise<ProbeResult[]>;

/// Entries retained per pane. Chosen so a tall pane's worth of distinct
/// candidates (a 60-row window with a handful of paths per row) stays
/// resident while a `cat` of a large file cannot grow the map without
/// bound. At roughly 100 bytes an entry this is ~25 KB per pane.
export const PROBE_CACHE_LIMIT = 256;

/// Candidates the backend should never be asked about — the answer is known
/// without a syscall, and asking would turn hovering over terminal output
/// into a filesystem oracle driven by whatever the pane printed.
///
/// Existence is the main filter, but it is deliberately not the only one:
/// a control character must not reach the opener even if some filesystem
/// somewhere accepts it in a name.
export function isProbeable(text: string): boolean {
  if (!text) return false;
  if (text.length > 4096) return false;
  if (/[\u0000-\u001f\u007f]/.test(text)) return false;
  return true;
}

/// Keep the most informative non-overlapping subset of `candidates`.
///
/// The matcher emits overlapping readings on purpose — a quoted region
/// yields both the whole quoted string and the tokens inside it — and xterm
/// silently drops links that intersect, so the choice has to be made here
/// rather than left to it. Longest existing candidate wins, which is what
/// picks `C:\Program Files\Git` over the `C:\Program` prefix inside it.
export function resolveOverlaps<T extends { start: number; end: number }>(
  candidates: readonly T[],
  exists: (c: T) => boolean,
): T[] {
  const kept: T[] = [];
  const ranked = candidates
    .filter(exists)
    .slice()
    .sort((a, b) => b.end - b.start - (a.end - a.start) || a.start - b.start);
  for (const c of ranked) {
    if (kept.some((k) => c.start < k.end && c.end > k.start)) continue;
    kept.push(c);
  }
  return kept.sort((a, b) => a.start - b.start);
}

/// Per-pane memo of existence answers.
///
/// Eviction is least-recently-used: a hit re-inserts the key so the entries
/// the user is actually hovering stay resident. Negative answers are cached
/// too — prose like `and/or` is by far the commonest candidate, and not
/// remembering that it is not a file is how the cache would fail to do its
/// one job.
export class PathProbeCache {
  private cwd: string | null = null;
  private readonly entries = new Map<string, ProbeResult>();
  /// In-flight probes, so two hovers over the same row (or two candidates
  /// resolving to the same text) share one request instead of racing.
  private readonly inflight = new Map<string, Promise<ProbeResult>>();

  constructor(
    private readonly probe: ProbeFn,
    private readonly limit: number = PROBE_CACHE_LIMIT,
  ) {}

  /// The pane's live cwd, which is what relative candidates resolve
  /// against. Changing it invalidates everything: `src/main.ts` means a
  /// different file after a `cd`, and an absolute entry kept across the
  /// change would be a needless special case for no measurable gain.
  setCwd(cwd: string | null): void {
    if (cwd === this.cwd) return;
    this.cwd = cwd;
    this.entries.clear();
    this.inflight.clear();
  }

  get currentCwd(): string | null {
    return this.cwd;
  }

  get size(): number {
    return this.entries.size;
  }

  /// The cached answer, or `undefined` when this text has not been probed.
  peek(text: string): ProbeResult | undefined {
    if (!this.entries.has(text)) return undefined;
    const value = this.entries.get(text)!;
    // Re-insert to mark it as most recently used.
    this.entries.delete(text);
    this.entries.set(text, value);
    return value;
  }

  /// Is every one of `candidates` already answered? When true, the caller
  /// can build its links synchronously, which keeps a re-hover of a row the
  /// user just left from flickering.
  allCached(candidates: readonly PathCandidate[]): boolean {
    return candidates.every(
      (c) => !isProbeable(c.text) || this.entries.has(c.text),
    );
  }

  /// Resolve every candidate, consulting the cache first and issuing at
  /// most one probe call for the rest.
  async resolve(
    candidates: readonly PathCandidate[],
  ): Promise<Map<string, ResolvedPath>> {
    const out = new Map<string, ResolvedPath>();
    const wanted: string[] = [];
    const pending: Array<Promise<ProbeResult>> = [];
    const pendingKeys: string[] = [];

    for (const c of candidates) {
      if (!isProbeable(c.text)) continue;
      const hit = this.peek(c.text);
      if (hit !== undefined) {
        if (hit) out.set(c.text, hit);
        continue;
      }
      const running = this.inflight.get(c.text);
      if (running) {
        if (!pendingKeys.includes(c.text)) {
          pendingKeys.push(c.text);
          pending.push(running);
        }
        continue;
      }
      if (!wanted.includes(c.text)) wanted.push(c.text);
    }

    if (wanted.length > 0) {
      const cwd = this.cwd;
      // A rejected probe (IPC error, backend timeout, every backend probe
      // worker busy) is "no link", not a thrown error: the caller must
      // always get an answer, or xterm's hover never resolves and the row is
      // stuck without links. But it is also *not* an answer, so nothing from
      // it is cached — the next hover asks again.
      const batch = this.probe(wanted, cwd).then(
        (results) => ({ ok: true, results }),
        () => ({ ok: false, results: [] as ProbeResult[] }),
      );
      for (const [i, text] of wanted.entries()) {
        const one: Promise<ProbeResult> = batch.then(({ ok, results }) => {
          const r = results[i] ?? null;
          // Only retract the in-flight marker if it is still ours; a cwd
          // change clears the map and a later probe may hold the slot.
          if (this.inflight.get(text) === one) this.inflight.delete(text);
          // An answer computed against a cwd the pane has since left is
          // worthless — and caching it would outlive the invalidation that
          // was supposed to throw it away.
          if (ok && this.cwd === cwd) this.store(text, r);
          return r;
        });
        this.inflight.set(text, one);
        pendingKeys.push(text);
        pending.push(one);
      }
    }

    const settled = await Promise.all(pending);
    settled.forEach((r, i) => {
      if (r) out.set(pendingKeys[i]!, r);
    });
    return out;
  }

  private store(text: string, result: ProbeResult): void {
    if (this.entries.has(text)) this.entries.delete(text);
    this.entries.set(text, result);
    while (this.entries.size > this.limit) {
      const oldest = this.entries.keys().next();
      if (oldest.done) break;
      this.entries.delete(oldest.value);
    }
  }
}
