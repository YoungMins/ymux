// The xterm link provider that turns filesystem paths printed in a pane into
// clickable links, alongside the URL links `WebLinksAddon` already provides.
//
// This file is the wiring: the three interesting decisions live in pure
// modules next to it and are unit-tested there — `pathMatch.ts` (what looks
// like a path), `pathRow.ts` (where it sits in the buffer), `pathProbe.ts`
// (does it exist, and which of the overlapping readings to keep).
//
// Registration order is load-bearing. xterm queries link providers in the
// order they were registered and silently drops a link that intersects one
// from an earlier provider, so this must be registered *after*
// `WebLinksAddon` — that is what makes `https://host/a/b` stay a URL rather
// than turning its path half into a file link.

import type { IDisposable, ILink, ILinkProvider, Terminal } from "@xterm/xterm";

import type { ResolvedPath } from "../types";
import { hasMod } from "../platform";
import { findPathCandidates, type PathCandidate } from "./pathMatch";
import { PathProbeCache, resolveOverlaps, type ProbeFn } from "./pathProbe";
import { flattenWrappedRow, spanToRange, type FlatRow, type RowBuffer } from "./pathRow";

export interface PathLinksOptions {
  /// The pane's live working directory, read fresh on every hover. Relative
  /// candidates mean nothing without it, and it changes under us on `cd`.
  cwd: () => string | null;
  /// Ask the backend which candidates exist. Injected so this class can be
  /// exercised without the IPC layer.
  probe: ProbeFn;
  /// Hand a confirmed path to the OS default handler.
  open: (resolved: ResolvedPath) => void;
}

/// Look up an already-probed candidate. `undefined` means "not a link".
type Lookup = (text: string) => ResolvedPath | undefined;

export class PathLinks implements ILinkProvider {
  private readonly cache: PathProbeCache;
  private registration: IDisposable | null = null;
  /// The hover tooltip. Parented to `document.body`, not to the pane, so
  /// neither the bottom anchor's transform on `.xterm-screen` nor any
  /// `overflow: hidden` in the layout can clip or displace it.
  private tip: HTMLElement | null = null;

  constructor(
    private readonly term: Terminal,
    private readonly opts: PathLinksOptions,
  ) {
    this.cache = new PathProbeCache(opts.probe);
  }

  /// Register with xterm. Call this *after* `WebLinksAddon` is loaded.
  install(): void {
    this.registration ??= this.term.registerLinkProvider(this);
  }

  /// xterm's `ILinkProvider`. `bufferLineNumber` is 1-based.
  ///
  /// The callback must fire exactly once on every path, including the error
  /// and empty ones: xterm waits for a reply from every provider before it
  /// decides what is under the pointer, so a swallowed callback leaves the
  /// row without links until the pointer leaves and comes back.
  provideLinks(
    bufferLineNumber: number,
    callback: (links: ILink[] | undefined) => void,
  ): void {
    let flat: FlatRow;
    try {
      flat = flattenWrappedRow(
        this.term.buffer.active satisfies RowBuffer,
        bufferLineNumber - 1,
      );
    } catch {
      callback(undefined);
      return;
    }
    if (!flat.text) {
      callback(undefined);
      return;
    }
    const candidates = findPathCandidates(flat.text);
    if (candidates.length === 0) {
      callback(undefined);
      return;
    }

    // A `cd` invalidates every relative answer, so the cwd is re-read here
    // rather than snapshotted at construction.
    this.cache.setCwd(this.opts.cwd());

    if (this.cache.allCached(candidates)) {
      // Answer in the same tick. Re-hovering a row the pointer just left is
      // the common case, and going through a promise for it makes the
      // underline blink off and back on.
      callback(this.build(flat, candidates, (t) => this.cache.peek(t) ?? undefined));
      return;
    }
    void this.cache
      .resolve(candidates)
      .then((found) => callback(this.build(flat, candidates, (t) => found.get(t))))
      .catch(() => callback(undefined));
  }

  private build(
    flat: FlatRow,
    candidates: readonly PathCandidate[],
    lookup: Lookup,
  ): ILink[] | undefined {
    const links: ILink[] = [];
    // The matcher emits overlapping readings on purpose; xterm would drop
    // the intersections itself, but arbitrarily, so the choice is made here.
    for (const c of resolveOverlaps(candidates, (x) => lookup(x.text) !== undefined)) {
      const resolved = lookup(c.text);
      const range = spanToRange(flat, c.start, c.end);
      if (!resolved || !range) continue;
      links.push({
        range,
        text: c.text,
        activate: (ev) => this.activate(ev, resolved),
        hover: (ev) => this.showTip(ev, resolved.absolute),
        leave: () => this.hideTip(),
      });
    }
    return links.length > 0 ? links : undefined;
  }

  /// Open the path. Gated on the primary button plus the platform modifier
  /// — Cmd on macOS, Ctrl elsewhere — which is exactly what the URL links in
  /// this same pane require, and what leaves a plain click free to place the
  /// cursor and start a selection.
  private activate(ev: MouseEvent, resolved: ResolvedPath): void {
    // xterm activates a link on mouseup of *any* button, so without this a
    // Ctrl+right-click would open the file and raise the context menu.
    if (ev.button !== 0 || !hasMod(ev)) return;
    // A modifier-held drag that happens to end on a link is a selection, not
    // a click. A plain click has already cleared any prior selection by the
    // time this runs, so this only ever catches the drag.
    if (this.term.hasSelection()) return;
    ev.preventDefault();
    this.hideTip();
    this.opts.open(resolved);
  }

  private showTip(ev: MouseEvent, text: string): void {
    if (!this.tip) {
      this.tip = document.createElement("div");
      this.tip.className = "path-link-tip";
      document.body.appendChild(this.tip);
    }
    const tip = this.tip;
    tip.textContent = text;
    tip.classList.add("path-link-tip--visible");
    // Offset below-right of the pointer so the tooltip never sits on the
    // link it describes, then pulled back inside the window.
    const margin = 8;
    const width = tip.offsetWidth;
    const height = tip.offsetHeight;
    const left = Math.max(
      margin,
      Math.min(ev.clientX + 12, window.innerWidth - width - margin),
    );
    const below = ev.clientY + 18;
    const top =
      below + height + margin > window.innerHeight
        ? Math.max(margin, ev.clientY - height - 10)
        : below;
    tip.style.left = `${left}px`;
    tip.style.top = `${top}px`;
  }

  private hideTip(): void {
    this.tip?.classList.remove("path-link-tip--visible");
  }

  dispose(): void {
    this.registration?.dispose();
    this.registration = null;
    this.tip?.remove();
    this.tip = null;
  }
}
