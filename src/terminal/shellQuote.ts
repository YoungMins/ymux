/// Quote a filesystem path so that typing it into a pane's shell hands the
/// program exactly that path, and nothing in it is expanded.
///
/// The family comes from the pane's shell *profile*, not from whatever is
/// running in it right now: a cmd pane where the user started bash still gets
/// cmd quoting.
///
/// Mirrors `ShellFamily` in `src-tauri/src/agent_sessions.rs` (same
/// classification, same quoting per family), plus a fish family the backend
/// folds into Unknown. That side may refuse to quote and fall back to
/// something else; here a refusal (`null`) means nothing is typed.
///
/// - **posix** (bash, zsh, sh, dash, ksh): `'…'`, a `'` becomes `'\''`.
///   Inside single quotes nothing — `$`, `` ` ``, `\`, `!` — is special.
/// - **fish**: `'…'`, but fish's single quotes are *not* POSIX ones: `\\`
///   and `\'` are escapes inside them. So `\` becomes `\\` first, then `'`
///   becomes `\'`. POSIX quoting would let `x\';touch pwned;#` break out.
/// - **powershell** (Windows PowerShell, pwsh): `'…'`, a quote is doubled.
///   PowerShell also reads U+2018–U+201B as single quotes, so those are
///   doubled too; nothing else expands inside single quotes.
/// - **cmd**: `"…"`. cmd has no `$` expansion, and Windows forbids `"` in
///   filenames. `%NAME%` still expands inside quotes on an interactive
///   command line and cannot be escaped there, so a path containing
///   `%PATH%` is typed as-is: it reaches the program expanded only when that
///   variable exists. That is a garbled path, never an executed command —
///   cmd runs nothing for a `%` expansion.
/// - **unknown** (`wsl.exe` whose inner shell is unknown, nu, csh/tcsh, any
///   custom profile): plain `'…'` only when the path holds nothing any of
///   those shells might treat specially inside single quotes (`'`, `\`, `!`,
///   the curly single quotes); otherwise refused. On Windows that refuses
///   every backslashed path — a WSL shell could not open it anyway.
///
/// Every path is quoted, even ones that need none, so the typed text looks
/// the same for every drop. A path with a control character (a newline, say)
/// is refused outright: typed into a terminal it would act as Enter.

export type ShellFamily = "posix" | "fish" | "powershell" | "cmd" | "unknown";

/// Classify a shell profile by its executable's file stem, like the backend's
/// `ShellFamily::from_executable` (plus `fish`).
export function shellFamilyFromExecutable(
  executable: string | null | undefined,
): ShellFamily {
  if (!executable) return "unknown";
  const base = executable.split(/[\\/]/).pop()!.toLowerCase();
  const stem = base.endsWith(".exe") ? base.slice(0, -4) : base;
  switch (stem) {
    case "cmd":
      return "cmd";
    case "powershell":
    case "pwsh":
      return "powershell";
    case "bash":
    case "zsh":
    case "sh":
    case "dash":
    case "ksh":
      return "posix";
    case "fish":
      return "fish";
    default:
      return "unknown";
  }
}

// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;
/// `'` plus the curly single quotes PowerShell also accepts as quotes.
const SINGLE_QUOTES = /[\u0027\u2018-\u201b]/;
/// What an unknown shell might still interpret inside single quotes.
const UNKNOWN_UNSAFE = /[\u0027\u2018-\u201b\\!]/;
/// What bash/zsh/fish interpret inside double quotes (`!` is history
/// expansion in interactive bash and zsh).
const POSIX_DQ_UNSAFE = /[$`\\"!]/;
/// What PowerShell interprets inside double quotes (U+201C–U+201E are
/// double quotes to it).
const POWERSHELL_DQ_UNSAFE = /[$`"\u201c-\u201e]/;

/// `path` quoted for `family`, or `null` when it cannot be typed safely.
export function quotePathForShell(path: string, family: ShellFamily): string | null {
  if (!path || CONTROL.test(path)) return null;
  switch (family) {
    case "posix":
      return `'${path.replace(/'/g, "'\\''")}'`;
    case "fish":
      return `'${path.replace(/\\/g, "\\\\").replace(/'/g, "\\'")}'`;
    case "powershell":
      return `'${path.replace(/[\u0027\u2018-\u201b]/g, "$&$&")}'`;
    case "cmd":
      return `"${path}"`;
    case "unknown":
      return UNKNOWN_UNSAFE.test(path) ? null : `'${path}'`;
  }
}

/// `path` quoted for a pasted image, or `null` to type nothing.
///
/// The pasted path is usually read by Claude Code, which strips one outer
/// pair of `'…'` or `"…"` from a pasted path but does not un-escape anything
/// inside it. So a path whose shell quoting needs an inner escape (`''`,
/// `'\''`, `\'` — a profile directory like `C:\Users\O'Brien\…`) must use
/// the other quote style instead, and only where that is still safe:
///
/// - nothing in the path the family's form would escape: that form, as for
///   a drop (cmd and unknown never escape anything inside their quotes);
/// - PowerShell (a `'` or curly single quote): `"…"` unless the path holds
///   `$`, `` ` `` or a double quote, which would expand or end the string;
/// - posix (a `'`) / fish (a `'` or `\`): `"…"` unless the path holds `$`,
///   `` ` ``, `\`, `"` or `!`;
/// - otherwise refused.
export function quotePathForPaste(path: string, family: ShellFamily): string | null {
  if (!path || CONTROL.test(path)) return null;
  switch (family) {
    case "cmd":
    case "unknown":
      return quotePathForShell(path, family);
    case "powershell":
      if (!SINGLE_QUOTES.test(path)) return quotePathForShell(path, family);
      return POWERSHELL_DQ_UNSAFE.test(path) ? null : `"${path}"`;
    case "posix":
    case "fish": {
      const escaped = family === "fish" ? /['\\]/ : /'/;
      if (!escaped.test(path)) return quotePathForShell(path, family);
      return POSIX_DQ_UNSAFE.test(path) ? null : `"${path}"`;
    }
  }
}
