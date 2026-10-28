// Renders a recursive LayoutNode tree as a DOM subtree. Splits become flexbox
// containers with a draggable gutter; panes become mounted TerminalPane
// elements. The renderer is stateful per-workspace so individual xterm
// instances survive layout edits.

import type { LayoutNode, SplitDir, Uuid } from "../types";
import type { Pane } from "./Pane";
import type { PaneGroup } from "./PaneGroup";
import { t } from "../i18n/i18n";
import { fill } from "../git/worktreeFlow";
import { IS_MAC } from "../platform";
import { linkedGroup, snapTo, type GutterLine } from "./gutterSnap";

/// What a gutter's drag can do to another gutter it moves with.
interface GutterHandle {
  /// Put the divider's centre at `pos` (client px across the drag).
  moveTo(pos: number): void;
  commit(): void;
}
const handles = new WeakMap<HTMLElement, GutterHandle>();

export interface RenderContext {
  /// Pane instance cache. Key is pane id. Ownership of entries is shared with
  /// `WorkspaceManager`, so the renderer neither creates nor disposes entries;
  /// it only mounts/unmounts the existing `element`s.
  paneCache: Map<Uuid, Pane>;
  /// Tab-group chrome, keyed by `LayoutNode::Tabs.id`. Same ownership deal as
  /// `paneCache`: the renderer looks a group up, creates it through
  /// `makeGroup` on first sight, and never disposes one (the manager prunes
  /// groups that left the tree).
  groupCache: Map<Uuid, PaneGroup>;
  makeGroup: (groupId: Uuid) => PaneGroup;
  onRatioCommitted: (path: number[], ratio: number) => void;
}

/// Render `node` into `container`, reusing existing DOM where possible. The
/// renderer rebuilds the split structure but preserves pane elements so that
/// xterm instances keep their scrollback.
export function render(
  node: LayoutNode,
  container: HTMLElement,
  ctx: RenderContext,
): void {
  // Detach all children without disposing: pane elements are still held by
  // ctx.paneCache and will be re-inserted below.
  while (container.firstChild) container.removeChild(container.firstChild);
  // Where a gutter looks for the dividers it snaps to and moves with.
  container.dataset.layoutRoot = "";
  container.appendChild(build(node, [], ctx));
}

function build(
  node: LayoutNode,
  path: number[],
  ctx: RenderContext,
): HTMLElement {
  if (node.kind === "pane") {
    const pane = ctx.paneCache.get(node.id);
    if (!pane) {
      const placeholder = document.createElement("div");
      placeholder.className = "pane pane--missing";
      placeholder.textContent = fill(t("pane.missing"), { id: node.id });
      return placeholder;
    }
    // Re-parenting the existing element keeps xterm's DOM subtree intact.
    pane.scheduleFit();
    return pane.element;
  }

  if (node.kind === "split") {
    const wrapper = document.createElement("div");
    wrapper.className = `split split--${node.direction}`;
    wrapper.style.display = "flex";
    wrapper.style.flex = "1 1 auto";
    wrapper.style.flexDirection = node.direction === "horizontal" ? "row" : "column";
    wrapper.style.minWidth = "0";
    wrapper.style.minHeight = "0";

    const a = wrapWithFlex(build(node.a, [...path, 0], ctx), node.ratio);
    const b = wrapWithFlex(build(node.b, [...path, 1], ctx), 1 - node.ratio);
    const gutter = makeGutter(node.direction, wrapper, a, b, path, node.ratio, ctx);

    wrapper.appendChild(a);
    wrapper.appendChild(gutter);
    wrapper.appendChild(b);
    return wrapper;
  }

  if (node.kind === "tabs") {
    // Shared chrome only makes sense when every child is a pane. Nested tabs
    // are created by no command; if one ever appears, fall back to the plain
    // rendering below rather than dropping the subtree.
    if (node.children.every((c) => c.kind === "pane")) {
      let group = ctx.groupCache.get(node.id);
      if (!group) {
        group = ctx.makeGroup(node.id);
        ctx.groupCache.set(node.id, group);
      }
      group.update(node, ctx.paneCache);
      return group.element;
    }
    return buildNestedTabs(node, path, ctx);
  }

  const unknown = document.createElement("div");
  unknown.textContent = "unknown node";
  return unknown;
}

/// Pre-PaneGroup rendering, kept for the one shape the shared chrome cannot
/// serve: a `Tabs` node with a non-pane child. Nothing creates one.
function buildNestedTabs(
  node: LayoutNode & { kind: "tabs" },
  path: number[],
  ctx: RenderContext,
): HTMLElement {
  const wrapper = document.createElement("div");
  wrapper.className = "tabs";
  wrapper.style.display = "flex";
  wrapper.style.flexDirection = "column";
  wrapper.style.flex = "1 1 auto";
  wrapper.style.minWidth = "0";
  wrapper.style.minHeight = "0";

  const header = document.createElement("div");
  header.className = "tabs__header";
  wrapper.appendChild(header);

  const body = document.createElement("div");
  body.className = "tabs__body";
  body.style.flex = "1 1 auto";
  wrapper.appendChild(body);

  node.children.forEach((child, idx) => {
    const tab = document.createElement("button");
    tab.className = "tabs__tab";
    if (idx === node.active) tab.classList.add("tabs__tab--active");
    tab.textContent = fill(t("tab.fallbackName"), { n: idx + 1 });
    header.appendChild(tab);
    if (idx === node.active) {
      body.appendChild(build(child, [...path, idx], ctx));
    }
  });

  return wrapper;
}

function wrapWithFlex(inner: HTMLElement, flex: number): HTMLElement {
  const w = document.createElement("div");
  w.className = "split__child";
  w.style.flex = `${flex} ${flex} 0`;
  w.style.display = "flex";
  w.style.flexDirection = "column";
  w.style.minWidth = "0";
  w.style.minHeight = "0";
  w.appendChild(inner);
  return w;
}

function makeGutter(
  direction: SplitDir,
  parent: HTMLElement,
  a: HTMLElement,
  b: HTMLElement,
  path: number[],
  startRatio: number,
  ctx: RenderContext,
): HTMLElement {
  const gutter = document.createElement("div");
  gutter.className = `gutter gutter--${direction}`;
  gutter.style.flex = "0 0 4px";
  gutter.title = fill(t("layout.gutter.title"), { alt: IS_MAC ? "Option" : "Alt" });
  if (direction === "horizontal") {
    gutter.style.cursor = "col-resize";
  } else {
    gutter.style.cursor = "row-resize";
  }

  let dragging = false;
  let ratio = startRatio;
  const horizontal = direction === "horizontal";
  // Moves this divider's centre to `pos`; false when it has no size yet.
  const moveTo = (pos: number): boolean => {
    const rect = parent.getBoundingClientRect();
    const start = horizontal ? rect.left : rect.top;
    const size = horizontal ? rect.width : rect.height;
    if (size <= 0) return false;
    ratio = clamp((pos - start) / size, 0.05, 0.95);
    a.style.flex = `${ratio} ${ratio} 0`;
    b.style.flex = `${1 - ratio} ${1 - ratio} 0`;
    return true;
  };
  handles.set(gutter, {
    moveTo: (pos) => void moveTo(pos),
    commit: () => ctx.onRatioCommitted(path, ratio),
  });

  // The dividers of this direction in the workspace, as lines on screen.
  const peersNow = (): HTMLElement[] => {
    const root = gutter.closest<HTMLElement>("[data-layout-root]") ?? parent;
    return [...root.querySelectorAll<HTMLElement>(`.gutter--${direction}`)].filter((g) => g.getClientRects().length > 0);
  };
  const lineOf = (g: HTMLElement): GutterLine => {
    const r = g.getBoundingClientRect();
    return horizontal ? { at: r.left + r.width / 2, from: r.top, to: r.bottom } : { at: r.top + r.height / 2, from: r.left, to: r.right };
  };
  // The dividers this drag moves (this one, plus those on its line meeting
  // it end to end), fixed when the drag starts; Alt drags this one alone.
  let group: HTMLElement[] = [gutter];
  let alone = false;
  const setGroup = (members: HTMLElement[]) => {
    for (const g of group) g.classList.remove("gutter--linked");
    group = members;
    if (members.length > 1) for (const g of members) g.classList.add("gutter--linked");
  };

  const onMove = (ev: PointerEvent) => {
    if (!dragging) return;
    let pos = horizontal ? ev.clientX : ev.clientY;
    let snapped = false;
    if (!alone) {
      // Re-measured on every move: a divider nested in a moved split moves too.
      const peers = peersNow();
      const mine = group.map((g) => peers.indexOf(g)).filter((i) => i >= 0);
      const hit = snapTo(pos, peers.map(lineOf), mine);
      pos = hit.at;
      snapped = hit.snapped !== null;
    }
    gutter.classList.toggle("gutter--snapped", snapped);
    if (!moveTo(pos)) return;
    for (const g of group) if (g !== gutter) handles.get(g)?.moveTo(pos);
    // Refit every cached pane; cheap at small pane counts and avoids having
    // to walk the DOM to find which xterm instances live inside `a`/`b`.
    for (const pane of ctx.paneCache.values()) pane.scheduleFit();
  };

  // Every way a drag can end, not just `pointerup`: an Alt+Tab (window
  // `blur`) or an OS-cancelled pointer never delivers one, and a drag left
  // open would keep every iframe at `pointer-events: none`.
  const endDrag = () => {
    if (!dragging) return;
    dragging = false;
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", endDrag);
    window.removeEventListener("pointercancel", endDrag);
    window.removeEventListener("blur", endDrag);
    gutter.removeEventListener("lostpointercapture", endDrag);
    setIframePointerEvents("");
    gutter.classList.remove("gutter--snapped");
    for (const g of group) if (g !== gutter) handles.get(g)?.commit();
    setGroup([gutter]);
    ctx.onRatioCommitted(path, ratio);
  };

  gutter.addEventListener("pointerdown", (ev) => {
    ev.preventDefault();
    dragging = true;
    alone = ev.altKey;
    if (alone) setGroup([gutter]);
    else {
      const peers = peersNow();
      const self = peers.indexOf(gutter);
      setGroup(self < 0 ? [gutter] : linkedGroup(peers.map(lineOf), self).map((i) => peers[i]));
    }
    // Capture, so the up/cancel comes back here even when released over an
    // iframe or outside the window.
    try {
      gutter.setPointerCapture(ev.pointerId);
    } catch {
      // Pointer already gone; the window listeners below still end the drag.
    }
    setIframePointerEvents("none");
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", endDrag);
    window.addEventListener("pointercancel", endDrag);
    window.addEventListener("blur", endDrag);
    gutter.addEventListener("lostpointercapture", endDrag);
  });

  return gutter;
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

function setIframePointerEvents(value: string): void {
  for (const iframe of document.querySelectorAll<HTMLIFrameElement>("iframe")) {
    iframe.style.pointerEvents = value;
  }
}
