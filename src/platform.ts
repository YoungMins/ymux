/// Platform detection and the "primary modifier" every ymux shortcut hangs off.
///
/// ymux was built Windows-first, so its shortcuts are written in the canonical
/// `Ctrl+Shift+H` form throughout the code and the docs. On macOS that combo
/// is wrong twice over: users expect `Cmd`, and `Ctrl` is load-bearing inside
/// the terminal itself (`Ctrl+C`, `Ctrl+D`, `Ctrl+F` as forward-char). So on
/// macOS the primary modifier becomes `Cmd`, which leaves every `Ctrl`
/// sequence free to reach the shell untouched.

/// True when running on macOS.
///
/// `navigator.platform` is deprecated but is the only signal available in
/// every webview ymux targets; `userAgentData` is Chromium-only and absent in
/// WKWebView. We check both and fall back to the user-agent string.
export const IS_MAC: boolean = detectMac();

function detectMac(): boolean {
  const nav = globalThis.navigator as
    | (Navigator & { userAgentData?: { platform?: string } })
    | undefined;
  if (!nav) return false;
  const uaPlatform = nav.userAgentData?.platform;
  if (uaPlatform) return uaPlatform.toLowerCase().includes("mac");
  const platform = nav.platform ?? "";
  if (platform) return /mac/i.test(platform);
  return /Mac OS X|Macintosh/i.test(nav.userAgent ?? "");
}

/// Does this event carry ymux's primary modifier — `Cmd` on macOS, `Ctrl`
/// everywhere else?
export function hasMod(ev: {
  ctrlKey: boolean;
  metaKey: boolean;
}): boolean {
  return IS_MAC ? ev.metaKey : ev.ctrlKey;
}

/// Shortcuts that keep `Ctrl` even on macOS.
///
/// `Cmd+Tab` is the macOS application switcher: the OS consumes it before any
/// webview sees a keydown, so pane cycling has to stay on `Ctrl+Tab`. It is
/// safe to leave there because a bare `Ctrl+Tab` means nothing to a shell.
const MAC_KEEPS_CTRL = new Set(["Ctrl+Tab", "Ctrl+Shift+Tab"]);

/// Render a shortcut — written in the canonical Windows form — for the
/// current platform. Used by the Help overlay and the command palette so the
/// hints match the keys that actually work.
export function shortcutLabel(spec: string): string {
  if (!IS_MAC) return spec;
  if (MAC_KEEPS_CTRL.has(spec)) return spec;
  // Workspace switching drops Alt on macOS: `Cmd+1…9` is the near-universal
  // mac idiom for "go to the Nth thing", and the Alt was only ever there to
  // dodge a Windows-level interception of Ctrl+Shift+digit.
  let out = spec.replace(/^Ctrl\+Alt\+(?=\d)/, "Cmd+");
  out = out.replace(/^Ctrl\+/, "Cmd+");
  out = out.replace(/^Cmd\+Alt\+/, "Cmd+Opt+");
  return out;
}

/// Does this event match "switch to workspace N"?
///
/// Split out because it is the one binding whose *shape* differs per platform
/// rather than just its modifier: `Ctrl+Alt+N` on Windows, plain `Cmd+N` on
/// macOS. Callers still read the digit from `ev.code` so non-QWERTY layouts
/// keep working.
export function isWorkspaceSwitch(ev: {
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  code: string;
}): boolean {
  if (ev.shiftKey || !/^Digit[1-9]$/.test(ev.code)) return false;
  return IS_MAC
    ? ev.metaKey && !ev.ctrlKey && !ev.altKey
    : ev.ctrlKey && ev.altKey;
}
