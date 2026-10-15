import type { ReviewComment } from "./model-types";
import type { DiffLine } from "./types";

/// Warn the user before sending a prompt larger than this many bytes.
export const PROMPT_WARN = 32 * 1024;

// C0 controls except \t and \n, DEL, C1 controls (8-bit terminals act on
// them) and the Unicode line separators: nothing a terminal or the agent
// could act on.
// eslint-disable-next-line no-control-regex
const CONTROLS = /[\x00-\x08\x0b-\x1f\x7f-\x9f\u2028\u2029]/g;

const clean = (s: string) => s.replace(CONTROLS, "");

/// For single-line fields (paths, root, base): newline, return and tab are
/// shown as `\n`, `\r`, `\t` so a file name cannot start a prompt line.
const inline = (s: string) =>
  clean(s.replace(/\n/g, "\\n").replace(/\r/g, "\\r").replace(/\t/g, "\\t"));

/// Diff lines as prefixed text for `ReviewComment.excerpt`.
export function prefixLines(lines: DiffLine[]): string[] {
  return lines
    .filter((l) => l.kind !== "noeol")
    .map((l) => (l.kind === "add" ? "+" : l.kind === "del" ? "-" : " ") + l.text);
}

function fenceFor(s: string): string {
  const longest = Math.max(0, ...(s.match(/`+/g) ?? []).map((r) => r.length));
  return "`".repeat(Math.max(3, longest + 1));
}

function where(c: ReviewComment): string {
  const range = c.endLine !== undefined && c.endLine !== c.line ? `${c.line}-${c.endLine}` : `${c.line}`;
  return c.side === "old" ? `${inline(c.path)} old L${range} (deleted)` : `${inline(c.path)}:L${range}`;
}

/// The text pasted into an agent: a short header, then one numbered entry per
/// comment with the commented line fenced and the reviewer's note under it.
export function composePrompt(root: string, baseLabel: string, comments: ReviewComment[]): string {
  const out = [
    `Code review comments on the changes in ${inline(root)} (compared with ${inline(baseLabel)}).`,
    "Address each one, then summarize what you changed.",
  ];
  comments.forEach((c, i) => {
    const code = c.excerpt?.length ? c.excerpt.map(clean).join("\n") : clean(c.lineText);
    const f = fenceFor(code);
    out.push("", `${i + 1}. ${where(c)}${c.stale ? " (line may have moved)" : ""}`, f, code, f, clean(c.text));
  });
  return out.join("\n") + "\n";
}
