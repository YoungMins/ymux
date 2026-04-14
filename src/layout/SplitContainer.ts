// Renders a recursive LayoutNode tree as a DOM subtree. Splits become flexbox
// containers with a draggable gutter; panes become mounted TerminalPane
// elements. The renderer is stateful per-workspace so individual xterm
// instances survive layout edits.

import type { LayoutNode, SplitDir, Uuid } from "../types";
import type { TerminalPane } from "../terminal/TerminalPane";

export interface RenderContext {
  /// Pane instance cache. Key is pane id. Ownership of entries is shared with
  /// `WorkspaceManager`, so the renderer neither creates nor disposes entries;
  /// it only mounts/unmounts the existing `element`s.
  paneCache: Map<Uuid, TerminalPane>;
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
      placeholder.textContent = `Missing pane ${node.id}`;
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
      tab.textContent = `Tab ${idx + 1}`;
      header.appendChild(tab);
      if (idx === node.active) {
        body.appendChild(build(child, [...path, idx], ctx));
      }
    });

    return wrapper;
  }

  const unknown = document.createElement("div");
  unknown.textContent = "unknown node";
  return unknown;
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
  if (direction === "horizontal") {
    gutter.style.cursor = "col-resize";
  } else {
    gutter.style.cursor = "row-resize";
  }

  let dragging = false;
  let ratio = startRatio;
  const onMove = (ev: PointerEvent) => {
    if (!dragging) return;
    const rect = parent.getBoundingClientRect();
    if (direction === "horizontal") {
      if (rect.width <= 0) return;
      ratio = clamp((ev.clientX - rect.left) / rect.width, 0.05, 0.95);
    } else {
      if (rect.height <= 0) return;
      ratio = clamp((ev.clientY - rect.top) / rect.height, 0.05, 0.95);
    }
    a.style.flex = `${ratio} ${ratio} 0`;
    b.style.flex = `${1 - ratio} ${1 - ratio} 0`;
    // Refit every cached pane; cheap at small pane counts and avoids having
    // to walk the DOM to find which xterm instances live inside `a`/`b`.
    for (const pane of ctx.paneCache.values()) pane.scheduleFit();
  };

  const endDrag = () => {
    if (!dragging) return;
    dragging = false;
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", endDrag);
    ctx.onRatioCommitted(path, ratio);
  };

  gutter.addEventListener("pointerdown", (ev) => {
    ev.preventDefault();
    dragging = true;
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", endDrag);
  });

  return gutter;
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}
