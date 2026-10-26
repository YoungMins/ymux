// Inline SVG for the office view: one small front-facing character per agent
// and a prop per room kind. Plain markup strings with no external assets, so
// they pass the app's CSP; parts carry classes the CSS animates (legs swing
// while walking, arms type, one arm goes up when the agent needs the user).

import type { PaneKind } from "../types";

interface Palette {
  shirt: string;
  trim: string;
  hair: string;
}

const PALETTES: Record<string, Palette> = {
  claude: { shirt: "#d97757", trim: "#f4c7b3", hair: "#5b3a29" },
  codex: { shirt: "#24292f", trim: "#10a37f", hair: "#1b1f23" },
  gemini: { shirt: "#4285f4", trim: "#c6dafc", hair: "#2b2b2b" },
  opencode: { shirt: "#6d5dfc", trim: "#d6d1ff", hair: "#30264f" },
  aider: { shirt: "#14b8a6", trim: "#ccfbf1", hair: "#3f3f46" },
  copilot: { shirt: "#8957e5", trim: "#e2d5fb", hair: "#2d2d2d" },
  manager: { shirt: "#3a6ea5", trim: "#e8eef6", hair: "#3b2f2f" },
};
const FALLBACK: Palette = { shirt: "#7d8794", trim: "#d9dee4", hair: "#3a3a3a" };

export function paletteOf(kind: string): Palette {
  return PALETTES[kind] ?? FALLBACK;
}

/// A character, feet at the bottom centre of a 32×44 box.
/// `role` "sub" draws a cap and a name tag; "manager" draws a tie.
export function characterSvg(kind: string, role: "lead" | "sub" | "manager"): string {
  const p = role === "manager" ? PALETTES.manager : paletteOf(kind);
  const cap =
    role === "sub"
      ? `<path d="M8 11 Q16 2 24 11 Z" fill="${p.shirt}"/><rect x="7" y="10" width="18" height="2.4" rx="1" fill="${p.trim}"/>`
      : `<path d="M8.5 12 Q9 4.5 16 4.5 Q23 4.5 23.5 12 Q20 8.5 16 8.8 Q12 8.5 8.5 12 Z" fill="${p.hair}"/>`;
  const tie =
    role === "manager"
      ? `<path d="M16 22 L14.6 24 L16 31 L17.4 24 Z" fill="#c0392b"/>`
      : `<rect x="12" y="22.5" width="8" height="2" rx="1" fill="${p.trim}" opacity="0.9"/>`;
  return `<svg class="office-sprite" viewBox="0 0 32 44" width="32" height="44" aria-hidden="true">
  <ellipse class="o-shadow" cx="16" cy="42.5" rx="9" ry="2" fill="rgba(0,0,0,0.28)"/>
  <g class="o-leg o-leg--l"><rect x="11" y="32" width="4" height="10" rx="1.6" fill="#33404f"/><rect x="10" y="40" width="6" height="2.6" rx="1.2" fill="#1d242d"/></g>
  <g class="o-leg o-leg--r"><rect x="17" y="32" width="4" height="10" rx="1.6" fill="#33404f"/><rect x="16" y="40" width="6" height="2.6" rx="1.2" fill="#1d242d"/></g>
  <g class="o-body">
    <rect x="8.5" y="20" width="15" height="14" rx="5" fill="${p.shirt}"/>
    ${tie}
    <g class="o-arm o-arm--l"><rect x="5" y="21" width="4" height="10" rx="2" fill="${p.shirt}"/><circle cx="7" cy="31.5" r="2" fill="#f2c9a5"/></g>
    <g class="o-arm o-arm--r"><rect x="23" y="21" width="4" height="10" rx="2" fill="${p.shirt}"/><circle cx="25" cy="31.5" r="2" fill="#f2c9a5"/></g>
    <g class="o-mug"><rect x="23.5" y="27" width="5" height="5" rx="1" fill="#f5f0e6"/><path d="M28.5 28.2 q2 1.3 0 2.6" stroke="#f5f0e6" stroke-width="1" fill="none"/></g>
    <g class="o-head">
      <circle cx="16" cy="13" r="8" fill="#f2c9a5"/>
      ${cap}
      <g class="o-eyes"><rect x="12.4" y="13" width="2" height="2.6" rx="1" fill="#2a2a2a"/><rect x="17.6" y="13" width="2" height="2.6" rx="1" fill="#2a2a2a"/></g>
      <path class="o-eyes-shut" d="M12 14.5 h3 M17.2 14.5 h3" stroke="#2a2a2a" stroke-width="1.1" stroke-linecap="round"/>
      <path d="M14.2 17.6 q1.8 1.3 3.6 0" stroke="#9b5b45" stroke-width="1" fill="none" stroke-linecap="round"/>
    </g>
  </g>
</svg>`;
}

/// A desk with a monitor; the screen's class is driven by the desk's status.
export function deskSvg(): string {
  return `<svg class="office-desk__svg" viewBox="0 0 56 34" width="56" height="34" aria-hidden="true">
  <rect class="office-desk__screen-frame" x="17" y="1" width="22" height="15" rx="2" fill="#20262e"/>
  <rect class="office-desk__screen" x="19" y="3" width="18" height="11" rx="1"/>
  <rect x="26" y="16" width="4" height="4" fill="#20262e"/>
  <rect class="office-desk__top" x="1" y="19" width="54" height="6" rx="2"/>
  <rect class="office-desk__front" x="4" y="25" width="48" height="9" rx="1.5"/>
  <rect x="21" y="20" width="14" height="2" rx="1" fill="#3a4350"/>
</svg>`;
}

/// One decoration per room kind, drawn in the room's top-right corner.
export function propSvg(kind: PaneKind): string {
  switch (kind) {
    case "files":
      return `<svg viewBox="0 0 34 30" width="34" height="30" aria-hidden="true"><rect x="1" y="1" width="32" height="28" rx="2" fill="#6b4f3a"/><rect x="3" y="3" width="28" height="7" fill="#3f2f22"/><rect x="3" y="12" width="28" height="7" fill="#3f2f22"/><rect x="3" y="21" width="28" height="6" fill="#3f2f22"/><rect x="5" y="4" width="3" height="6" fill="#e0b84f"/><rect x="9" y="5" width="3" height="5" fill="#5aa0d8"/><rect x="13" y="4" width="2" height="6" fill="#d9675a"/><rect x="6" y="13" width="4" height="6" fill="#7cc47f"/><rect x="11" y="14" width="3" height="5" fill="#e0b84f"/><rect x="18" y="22" width="5" height="5" fill="#5aa0d8"/></svg>`;
    case "git":
      return `<svg viewBox="0 0 26 34" width="26" height="34" aria-hidden="true"><rect x="1" y="1" width="24" height="32" rx="2" fill="#2b323c"/><rect x="4" y="5" width="18" height="4" fill="#3d4652"/><rect x="4" y="12" width="18" height="4" fill="#3d4652"/><rect x="4" y="19" width="18" height="4" fill="#3d4652"/><circle class="office-blink" cx="19" cy="7" r="1.2" fill="#4cd964"/><circle cx="19" cy="14" r="1.2" fill="#4cd964"/><circle class="office-blink office-blink--slow" cx="19" cy="21" r="1.2" fill="#f5a623"/><path d="M8 27 v4 M8 29 q5 0 5 -3" stroke="#f05133" stroke-width="1.6" fill="none"/></svg>`;
    case "browser":
    case "native_browser":
    case "embedded_browser":
      return `<svg viewBox="0 0 40 28" width="40" height="28" aria-hidden="true"><rect x="1" y="1" width="38" height="26" rx="2" fill="#9cc9ee"/><path d="M1 20 q10 -8 20 0 t18 -2 v9 h-38 z" fill="#79b46a"/><circle cx="31" cy="8" r="3.5" fill="#ffe08a"/><rect x="19" y="1" width="2" height="26" fill="#e9eef3"/><rect x="1" y="13" width="38" height="2" fill="#e9eef3"/><rect x="0" y="0" width="40" height="28" rx="2" fill="none" stroke="#e9eef3" stroke-width="2"/></svg>`;
    case "editor":
      return `<svg viewBox="0 0 30 30" width="30" height="30" aria-hidden="true"><rect x="3" y="2" width="22" height="26" rx="1.5" fill="#f4f1e8" transform="rotate(-6 14 15)"/><rect x="6" y="3" width="22" height="26" rx="1.5" fill="#fffdf6"/><path d="M9 9 h15 M9 13 h12 M9 17 h15 M9 21 h9" stroke="#8a94a3" stroke-width="1.3"/></svg>`;
    case "token_usage":
      return `<svg viewBox="0 0 34 28" width="34" height="28" aria-hidden="true"><rect x="1" y="1" width="32" height="26" rx="2" fill="#f7f7f2"/><rect x="6" y="15" width="4" height="8" fill="#5aa0d8"/><rect x="12" y="10" width="4" height="13" fill="#d97757"/><rect x="18" y="6" width="4" height="17" fill="#7cc47f"/><rect x="24" y="12" width="4" height="11" fill="#e0b84f"/><path d="M4 23 h26" stroke="#9aa3ad" stroke-width="1"/></svg>`;
    case "terminal":
    default:
      return `<svg viewBox="0 0 22 30" width="22" height="30" aria-hidden="true"><rect x="5" y="20" width="12" height="9" rx="2" fill="#a0673f"/><path d="M11 21 C3 14 4 6 11 3 C18 6 19 14 11 21 Z" fill="#4f9a5a"/><path d="M11 21 C8 15 9 9 11 5" stroke="#3a7a44" stroke-width="1.2" fill="none"/></svg>`;
  }
}
