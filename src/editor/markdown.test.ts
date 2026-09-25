// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { classifyLink, isMarkdownPath, renderMarkdown, resolveRelative, slugify } from "./markdown";

function dom(html: string): HTMLElement {
  const el = document.createElement("div");
  el.innerHTML = html;
  return el;
}

describe("isMarkdownPath", () => {
  it("matches .md and .markdown, any case", () => {
    expect(isMarkdownPath("C:\\r\\README.md")).toBe(true);
    expect(isMarkdownPath("/r/notes.MARKDOWN")).toBe(true);
    expect(isMarkdownPath("/r/main.ts")).toBe(false);
    expect(isMarkdownPath("/r/md")).toBe(false);
  });
});

describe("renderMarkdown sanitising", () => {
  it("removes script", () => {
    const el = dom(renderMarkdown("hi\n\n<script>alert(1)</script>"));
    expect(el.querySelector("script")).toBeNull();
    expect(el.innerHTML).not.toContain("alert");
  });

  it("drops event handlers from img", () => {
    const el = dom(renderMarkdown('<img src=x onerror="alert(1)">'));
    const img = el.querySelector("img");
    expect(img).not.toBeNull();
    expect(img!.hasAttribute("onerror")).toBe(false);
  });

  it("javascript: links lose their href or classify as ignore", () => {
    const el = dom(renderMarkdown("[x](javascript:alert(1))"));
    const a = el.querySelector("a");
    const href = a?.getAttribute("href");
    if (href != null) expect(classifyLink(href).kind).toBe("ignore");
    expect(el.innerHTML).not.toContain("javascript:");
  });

  it("forbids form, iframe, object, embed, style, link, meta, base, svg", () => {
    const src = [
      '<form action="x"><input name="a"></form>',
      '<iframe src="https://e.com"></iframe>',
      '<object data="x"></object>',
      '<embed src="x">',
      "<style>body{display:none}</style>",
      '<link rel="stylesheet" href="x">',
      '<meta http-equiv="refresh" content="0;url=https://e.com">',
      '<base href="https://e.com/">',
      '<svg><a href="javascript:alert(1)"><text>x</text></a></svg>',
    ].join("\n\n");
    const el = dom(renderMarkdown(src));
    for (const tag of ["form", "iframe", "object", "embed", "style", "link", "meta", "base", "svg"]) {
      expect(el.querySelector(tag), tag).toBeNull();
    }
  });

  it("drops target from links and style attributes", () => {
    const el = dom(renderMarkdown('<a href="https://e.com" target="_blank" style="color:red">x</a>'));
    const a = el.querySelector("a")!;
    expect(a.hasAttribute("target")).toBe(false);
    expect(a.hasAttribute("style")).toBe(false);
  });

  it("prefixes author ids so they cannot clobber globals", () => {
    const el = dom(renderMarkdown('<div id="location">x</div>'));
    expect(el.querySelector("div")!.id).toBe("user-content-location");
  });
});

describe("renderMarkdown GFM", () => {
  it("renders tables", () => {
    const el = dom(renderMarkdown("| a | b |\n|---|---|\n| 1 | 2 |"));
    expect(el.querySelector("table")).not.toBeNull();
    expect(el.querySelectorAll("td")).toHaveLength(2);
  });

  it("renders task lists with disabled checkboxes", () => {
    const el = dom(renderMarkdown("- [x] done\n- [ ] todo"));
    const boxes = el.querySelectorAll<HTMLInputElement>("input[type=checkbox]");
    expect(boxes).toHaveLength(2);
    for (const b of boxes) expect(b.disabled).toBe(true);
    expect(boxes[0].checked).toBe(true);
  });

  it("renders strikethrough", () => {
    expect(dom(renderMarkdown("~~gone~~")).querySelector("del")).not.toBeNull();
  });

  it("gives headings GitHub-style ids, deduplicated", () => {
    const el = dom(renderMarkdown("# Hello, **World**!\n\n## Hello World\n\n## Hello World"));
    const ids = [...el.querySelectorAll("h1, h2")].map((h) => h.id);
    expect(ids).toEqual(["user-content-hello-world", "user-content-hello-world-1", "user-content-hello-world-2"]);
  });

  it("slugifies non-ASCII headings", () => {
    expect(slugify("한글 제목")).toBe("한글-제목");
  });
});

describe("classifyLink", () => {
  it("external schemes", () => {
    expect(classifyLink("https://e.com/x")).toEqual({ kind: "external", url: "https://e.com/x" });
    expect(classifyLink("http://e.com")).toEqual({ kind: "external", url: "http://e.com/" });
    expect(classifyLink("HTTPS://E.COM")).toEqual({ kind: "external", url: "https://e.com/" });
    // open_url refuses anything but http(s), so nothing else is external.
    expect(classifyLink("mailto:a@b.c")).toEqual({ kind: "ignore" });
    expect(classifyLink("https://")).toEqual({ kind: "ignore" });
  });

  it("anchors, percent-decoded", () => {
    expect(classifyLink("#install")).toEqual({ kind: "anchor", id: "install" });
    expect(classifyLink("#%ED%95%9C")).toEqual({ kind: "anchor", id: "한" });
    expect(classifyLink("#")).toEqual({ kind: "ignore" });
  });

  it("relative markdown files, fragment and query stripped, decoded", () => {
    expect(classifyLink("docs/guide.md")).toEqual({ kind: "file", relPath: "docs/guide.md" });
    expect(classifyLink("../a%20b.markdown#sec")).toEqual({ kind: "file", relPath: "../a b.markdown" });
    expect(classifyLink("docs\\x.MD?raw=1")).toEqual({ kind: "file", relPath: "docs\\x.MD" });
  });

  it("ignores everything else", () => {
    for (const h of [
      "javascript:alert(1)",
      " JavaScript:alert(1)",
      "data:text/html,<script>",
      "file:///C:/x.md",
      "C:\\x\\y.md",
      "/etc/x.md",
      "//evil.com/x.md",
      "image.png",
      "src/main.ts",
      "bad%E0%A4%A.md",
      "",
    ]) {
      expect(classifyLink(h).kind, h).toBe("ignore");
    }
  });
});

describe("resolveRelative", () => {
  it("POSIX paths", () => {
    expect(resolveRelative("/home/a/repo/README.md", "docs/x.md")).toBe("/home/a/repo/docs/x.md");
    expect(resolveRelative("/home/a/repo/docs/x.md", "../README.md")).toBe("/home/a/repo/README.md");
    expect(resolveRelative("/home/a/repo/docs/x.md", "./y.md")).toBe("/home/a/repo/docs/y.md");
    expect(resolveRelative("/x.md", "../../y.md")).toBe("/y.md");
  });

  it("Windows paths, either separator in the link", () => {
    expect(resolveRelative("C:\\repo\\README.md", "docs/x.md")).toBe("C:\\repo\\docs\\x.md");
    expect(resolveRelative("C:\\repo\\docs\\x.md", "..\\README.md")).toBe("C:\\repo\\README.md");
    expect(resolveRelative("C:\\x.md", "../../y.md")).toBe("C:\\y.md");
    expect(resolveRelative("C:/repo/docs/x.md", "../y.md")).toBe("C:/repo/y.md");
  });

  it("UNC roots are not climbed above", () => {
    expect(resolveRelative("\\\\srv\\share\\d\\x.md", "../../../y.md")).toBe("\\\\srv\\share\\y.md");
  });
});
