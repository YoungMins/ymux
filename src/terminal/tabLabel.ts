/// What a tab is called. Spec §4: "the pane's own title when the user set one
/// (double-click to rename), otherwise the running program — shell name by
/// default, `claude`, `codex`, `ycode: <file>` etc. from the existing per-pane
/// process scan".
///
/// Pure so the precedence is pinned by tests: the DOM side (`PaneGroup`, the
/// workspace tree) only supplies the three inputs.

export interface TabLabelInputs {
  /// `PaneSpec.title` — a name the user set, which always wins.
  title?: string | null;
  /// `PaneSpec.shell`, the resting label for a bare shell.
  shell: string;
  /// The deepest descendant of this pane's shell, as the backend's
  /// `panes:labels` snapshot reports it ("claude", "ycode: main.rs"), or null
  /// when the shell has no child process.
  process: string | null;
  /// Localised `terminal.defaultTitle`, for a pane with no shell name (a
  /// browser pane, or a spec written before shells were detected).
  fallback: string;
}

export function tabLabel(i: TabLabelInputs): string {
  const title = i.title?.trim();
  if (title) return title;
  if (i.process) return i.process;
  return i.shell || i.fallback;
}
