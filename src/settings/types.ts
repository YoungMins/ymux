// TypeScript mirror of `ytheme::Theme` (Rust). Every color is an
// `#rrggbb` hex string. Optional syntax-color section drives yCode's
// highlighter; everything else is reserved for future yMux theming.

export interface SyntaxColors {
  keyword: string;
  string: string;
  comment: string;
  number: string;
  function: string;
  type_name: string;
  variable: string;
  punctuation: string;
}

export interface YTheme {
  bg: string;
  bg_alt: string;
  bg_hover: string;
  fg: string;
  fg_muted: string;
  accent: string;
  border: string;
  status_ok: string;
  status_warn: string;
  status_critical: string;
  syntax: SyntaxColors;
}

/// Which known path the `open_config_path` command should open. Matches
/// `settings::ConfigPathKind` on the Rust side (serde snake_case).
export type ConfigPathKind = "theme" | "folder";

/// Sections of the settings panel — the left nav identifier.
export type SettingsSection =
  | "general"
  | "syntax"
  | "shortcuts"
  | "tools"
  | "config";
