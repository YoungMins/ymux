// Markdown preview for the editor pane: rendering, sanitising, and what a
// click on a link inside the preview is allowed to do.
//
// The preview lives in the main webview, which can `invoke` every ymux
// command (CLAUDE.md rule 16) — so script in a rendered README would be
// RCE. Everything marked produces goes through DOMPurify with the HTML
// profile only (no SVG/MathML) and a deny list on top, and the pane never
// lets a link navigate: `classifyLink` decides, the pane acts.
//
// EditorPane loads this module with a dynamic import, like cmSetup.ts, so
// the boot bundle does not carry marked + DOMPurify.

import { Marked } from "marked";
import DOMPurify from "dompurify";
import { languageForPath } from "./editorModel";

/// DOMPurify's `SANITIZE_NAMED_PROPS` puts this in front of every id (ours
/// and the author's), so a heading called "Location" cannot clobber a
/// global. Anchor lookups try it first.
export const ID_PREFIX = "user-content-";

export function isMarkdownPath(path: string): boolean {
  return languageForPath(path) === "markdown";
}

const ENTITY: Record<string, string> = { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": '"', "&#39;": "'" };

/// GitHub-style heading slug: lowercase, punctuation dropped, spaces to `-`.
export function slugify(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, "")
    .replace(/\s/g, "-");
}

// Duplicate-heading counter for the render in progress (renders are
// synchronous, so one module-level map is enough).
let slugsSeen = new Map<string, number>();

const md = new Marked({
  gfm: true,
  renderer: {
    heading({ tokens, depth }) {
      const inner = this.parser.parseInline(tokens);
      const plain = inner.replace(/<[^>]*>/g, "").replace(/&(amp|lt|gt|quot|#39);/g, (m) => ENTITY[m] ?? m);
      let slug = slugify(plain);
      const n = slugsSeen.get(slug) ?? 0;
      slugsSeen.set(slug, n + 1);
      if (n > 0) slug = `${slug}-${n}`;
      return `<h${depth} id="${slug}">${inner}</h${depth}>\n`;
    },
  },
});

const LANGUAGE_CLASS = /^language-[\w+-]+$/;

let purifier: ReturnType<typeof DOMPurify> | null = null;

/// A private DOMPurify instance, so its hooks touch nothing else.
function purify(): ReturnType<typeof DOMPurify> {
  if (purifier) return purifier;
  const p = DOMPurify(window);
  // No author classes: a README must not borrow ymux's own CSS to paint
  // fake UI. The one survivor is marked's `language-*` on `<code>`; any
  // other token in the same attribute is dropped, and an attribute left
  // empty goes entirely.
  p.addHook("uponSanitizeAttribute", (node, data) => {
    if (data.attrName !== "class") return;
    const kept = node.tagName === "CODE" ? data.attrValue.split(/\s+/).filter((c) => LANGUAGE_CLASS.test(c)) : [];
    if (kept.length === 0) data.keepAttr = false;
    else data.attrValue = kept.join(" ");
  });
  p.addHook("afterSanitizeAttributes", (node) => {
    // Task-list checkboxes are a view, not a form.
    if (node.tagName === "INPUT") node.setAttribute("disabled", "");
    // No new windows; the click handler decides where a link goes anyway.
    if (node.tagName === "A") node.removeAttribute("target");
  });
  purifier = p;
  return p;
}

const FORBID_TAGS = [
  "form", "iframe", "object", "embed", "style", "link", "meta", "base",
  "script", "frame", "frameset", "portal", "button", "textarea", "select",
];

const PURIFY_CONFIG = {
  USE_PROFILES: { html: true },
  FORBID_TAGS,
  FORBID_ATTR: ["style", "formaction", "srcset"],
  SANITIZE_NAMED_PROPS: true,
};

function parse(src: string): string {
  slugsSeen = new Map();
  return md.parse(src, { async: false }) as string;
}

/// Markdown source → sanitised HTML string (tests, inspection).
export function renderMarkdown(src: string): string {
  return purify().sanitize(parse(src), PURIFY_CONFIG);
}

/// Markdown source → sanitised DOM, for the pane. Handing over nodes rather
/// than a string skips the second HTML parse that mutation-XSS rides on.
export function renderMarkdownFragment(src: string): DocumentFragment {
  return purify().sanitize(parse(src), { ...PURIFY_CONFIG, RETURN_DOM_FRAGMENT: true });
}

export type LinkKind =
  | { kind: "external"; url: string }
  | { kind: "anchor"; id: string }
  | { kind: "file"; relPath: string }
  | { kind: "ignore" };

function decode(s: string): string | null {
  try {
    return decodeURIComponent(s);
  } catch {
    return null;
  }
}

/// What a click on `<a href>` in the preview may do. `href` is the raw
/// attribute (never `a.href`, which the page URL has already resolved).
export function classifyLink(href: string): LinkKind {
  const h = href.trim();
  if (!h) return { kind: "ignore" };
  if (h.startsWith("#")) {
    const id = decode(h.slice(1));
    return id ? { kind: "anchor", id } : { kind: "ignore" };
  }
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(h);
  if (scheme) {
    const s = scheme[1].toLowerCase();
    // A one-letter "scheme" is a drive letter: an absolute path, not ours.
    if (s !== "http" && s !== "https") return { kind: "ignore" };
    // Re-serialised, so `open_url` gets a canonical URL, not the raw text.
    try {
      return { kind: "external", url: new URL(h).href };
    } catch {
      return { kind: "ignore" };
    }
  }
  // Absolute and protocol-relative paths are not relative links.
  if (h.startsWith("/") || h.startsWith("\\")) return { kind: "ignore" };
  const cut = h.search(/[?#]/);
  const rel = decode(cut >= 0 ? h.slice(0, cut) : h);
  if (!rel || !/\.(md|markdown)$/i.test(rel)) return { kind: "ignore" };
  return { kind: "file", relPath: rel };
}

/// `relPath` resolved against the directory of `baseFilePath`. Accepts `/`
/// and `\` in both; joins with `\` when the base uses it. `..` never climbs
/// above a root (`/`, `C:`, `\\server\share`).
export function resolveRelative(baseFilePath: string, relPath: string): string {
  const sep = baseFilePath.includes("\\") ? "\\" : "/";
  const cut = Math.max(baseFilePath.lastIndexOf("/"), baseFilePath.lastIndexOf("\\"));
  const parts = cut >= 0 ? baseFilePath.slice(0, cut).split(/[\\/]/) : [];
  let min = 0;
  if (/^(\\\\|\/\/)/.test(baseFilePath)) min = 4;
  else if (parts.length > 0 && (parts[0] === "" || /^[A-Za-z]:$/.test(parts[0]))) min = 1;
  for (const seg of relPath.split(/[\\/]/)) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (parts.length > min) parts.pop();
      continue;
    }
    parts.push(seg);
  }
  // A POSIX root keeps its leading separator even when nothing follows it.
  if (parts.length === 1 && parts[0] === "") return sep;
  return parts.join(sep);
}
