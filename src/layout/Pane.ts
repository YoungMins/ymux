// Shared interface for anything that renders inside a LayoutNode leaf. The
// WorkspaceManager / SplitContainer operate on this interface so they don't
// have to distinguish between terminal and browser panes — each implementation
// owns its own DOM element and lifecycle.

import type { Uuid } from "../types";

export interface Pane {
  readonly id: Uuid;
  readonly element: HTMLElement;
  focus(): void;
  /// Recompute size from the container. For terminals this fits xterm; for
  /// browser panes it's a no-op. Safe to call repeatedly.
  scheduleFit(): void;
  /// Bring the underlying resource online (spawn PTY / navigate iframe). Must
  /// be idempotent: calling twice should not double-spawn.
  spawn(): Promise<void>;
  /// Release every resource. DOM element is removed.
  dispose(): void;
}
