// ytheme → CodeMirror (spec §3.2 reason 6), pure.
//
// Two sources, on purpose:
//  - **chrome** (background, gutter, selection, cursor, panels) comes from the
//    app's own CSS tokens (`--bg`, `--fg-muted`, `--accent`, … in style.css),
//    referenced as `var(...)`, so the editor is the same surface as the files
//    pane and the terminal beside it rather than CodeMirror's default white;
//  - **syntax** comes from the user's ytheme `SyntaxColors`
//    (`load_syntax_theme`) — the palette ycode used — mapped onto
//    `@lezer/highlight` tags one-to-one.
//
// Imports `@lezer/highlight` (a value), so only the lazy editor chunk may
// import this module. ycode's `hex()` panicked on a malformed colour; this
// falls back to the default instead — a typo in theme.toml must not take the
// editor down.

import { tags, type Tag } from "@lezer/highlight";
import type { SyntaxColors } from "../settings/types";
import { MONO_FONT_NAMES } from "../ui/fonts";

/// ytheme's defaults (crates/ytheme/src/lib.rs `SyntaxColors::default`).
export const DEFAULT_SYNTAX: SyntaxColors = {
  keyword: "#c792ea",
  string: "#ecc48d",
  comment: "#637777",
  number: "#f78c6c",
  function: "#82aaff",
  type_name: "#ffcb8b",
  variable: "#d6deeb",
  punctuation: "#7fdbca",
};

const HEX = /^#[0-9a-fA-F]{6}$/;

/// `value` if it is an `#rrggbb` colour, else `fallback`.
export function safeHex(value: unknown, fallback: string): string {
  return typeof value === "string" && HEX.test(value.trim()) ? value.trim() : fallback;
}

export interface SyntaxRule {
  field: keyof SyntaxColors;
  tag: Tag | readonly Tag[];
  color: string;
}

/// The eight ytheme syntax colours on their `@lezer/highlight` tags (the
/// table in spec §3.2). Missing or malformed colours fall back per field.
export function syntaxRules(syntax: Partial<SyntaxColors> | null | undefined): SyntaxRule[] {
  const s = syntax ?? {};
  const c = (f: keyof SyntaxColors) => safeHex(s[f], DEFAULT_SYNTAX[f]);
  return [
    { field: "keyword", tag: [tags.keyword, tags.controlKeyword, tags.moduleKeyword], color: c("keyword") },
    { field: "string", tag: [tags.string, tags.special(tags.string)], color: c("string") },
    { field: "comment", tag: tags.comment, color: c("comment") },
    { field: "number", tag: [tags.number, tags.bool, tags.null], color: c("number") },
    {
      field: "function",
      tag: [tags.function(tags.variableName), tags.function(tags.propertyName)],
      color: c("function"),
    },
    { field: "type_name", tag: [tags.typeName, tags.className, tags.namespace], color: c("type_name") },
    { field: "variable", tag: [tags.variableName, tags.propertyName], color: c("variable") },
    {
      field: "punctuation",
      tag: [tags.punctuation, tags.bracket, tags.operator],
      color: c("punctuation"),
    },
  ];
}

/// The editor chrome as a CodeMirror theme spec, built from the app's CSS
/// tokens. `fontSize` follows the terminal font size (Ctrl +/-/0 apply to
/// the editor too, spec §0.6).
export function chromeSpec(fontSize: number): Record<string, Record<string, string>> {
  // The shared stack, with Hangul-capable coding fonts ahead of the generics.
  const mono = `${MONO_FONT_NAMES}, "D2Coding", "Malgun Gothic", ui-monospace, "Courier New", monospace`;
  return {
    "&": {
      height: "100%",
      color: "var(--fg)",
      backgroundColor: "var(--bg)",
      fontSize: `${fontSize}px`,
    },
    "&.cm-focused": { outline: "none" },
    ".cm-scroller": { fontFamily: mono, lineHeight: "1.5" },
    ".cm-content": { caretColor: "var(--accent)", padding: "4px 0" },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "2px" },
    ".cm-gutters": {
      backgroundColor: "var(--bg)",
      color: "var(--fg-muted)",
      border: "none",
      borderRight: "1px solid var(--border)",
    },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 10px 0 12px", minWidth: "3ch" },
    ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--fg)" },
    ".cm-activeLine": { backgroundColor: "rgba(127, 219, 202, 0.045)" },
    "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, ::selection":
      { backgroundColor: "rgba(127, 219, 202, 0.2)" },
    ".cm-selectionMatch": { backgroundColor: "rgba(127, 219, 202, 0.1)" },
    ".cm-searchMatch": {
      backgroundColor: "rgba(229, 192, 123, 0.22)",
      outline: "1px solid rgba(229, 192, 123, 0.5)",
    },
    ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "rgba(229, 192, 123, 0.45)" },
    "&.cm-focused .cm-matchingBracket": {
      backgroundColor: "rgba(127, 219, 202, 0.18)",
      outline: "1px solid rgba(127, 219, 202, 0.45)",
    },
    ".cm-foldPlaceholder": {
      backgroundColor: "var(--bg-hover)",
      border: "1px solid var(--border)",
      color: "var(--fg-muted)",
    },
    ".cm-foldGutter .cm-gutterElement": { color: "var(--fg-muted)", cursor: "pointer" },
    ".cm-panels": {
      backgroundColor: "var(--bg-alt)",
      color: "var(--fg)",
    },
    ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
    ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--border)" },
    ".cm-panel input, .cm-panel button, .cm-panel label": { fontSize: "12px" },
    ".cm-textfield": {
      backgroundColor: "var(--bg)",
      color: "var(--fg)",
      border: "1px solid var(--border)",
      borderRadius: "4px",
      padding: "2px 6px",
    },
    ".cm-textfield:focus": { borderColor: "var(--accent)", outline: "none" },
    ".cm-button": {
      backgroundImage: "none",
      backgroundColor: "var(--bg)",
      color: "var(--fg-muted)",
      border: "1px solid var(--border)",
      borderRadius: "4px",
      padding: "2px 8px",
    },
    ".cm-button:hover": { color: "var(--accent)", borderColor: "var(--accent)" },
    ".cm-tooltip": {
      backgroundColor: "var(--bg-alt)",
      color: "var(--fg)",
      border: "1px solid var(--border)",
    },
  };
}
