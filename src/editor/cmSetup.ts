// The CodeMirror half of the editor pane — the one module that pulls the
// library in, loaded with a dynamic `import()` on the first editor pane so a
// user who never opens one pays nothing (spec §3.2, risk 7). EditorPane.ts
// may only `import type` from here.
//
// The pane talks to the editor through `EditorHandle`: load a document,
// read it back, a few commands. Everything that decides something (dirty,
// conflicts, the close guard) stays in the pure modules; this file only
// builds the view.

import { Compartment, EditorState, type Extension, type Text } from "@codemirror/state";
import {
  EditorView,
  crosshairCursor,
  drawSelection,
  dropCursor,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import { history, redo, undo } from "@codemirror/commands";
import {
  HighlightStyle,
  bracketMatching,
  foldGutter,
  indentOnInput,
  syntaxHighlighting,
} from "@codemirror/language";
import { highlightSelectionMatches, openSearchPanel, search } from "@codemirror/search";
import { tags } from "@lezer/highlight";
import type { SyntaxColors } from "../settings/types";
import { editorKeymap } from "./editorKeymap";
import { chromeSpec, syntaxRules } from "./theme";
import { loadLanguage } from "./languages";
import { cursorAfterReload, type LangId } from "./editorModel";

export interface EditorHandleOptions {
  isMac: boolean;
  fontSize: number;
  syntax: SyntaxColors | null;
  /// Any document change (typing, undo, a draft restore).
  onDocChange: () => void;
  /// Selection moved (status-bar cursor position).
  onSelection: () => void;
  /// Mod-s.
  onSave: () => void;
}

export interface EditorHandle {
  readonly view: EditorView;
  /// Replace the whole editor state: a new file, or a reload. Resets undo
  /// history — undoing past a reload into another version of the file is
  /// not something to offer.
  load(text: string, o: { readOnly: boolean; lang: LangId | null; cursor?: { line: number; col: number } }): void;
  /// Replace the document as one undoable edit (restoring a draft: undo
  /// goes back to what is on disk).
  replaceAll(text: string): void;
  doc(): Text;
  text(): string;
  cursor(): { line: number; col: number };
  lineLength(line: number): number;
  setFontSize(px: number): void;
  undo(): void;
  redo(): void;
  openSearch(): void;
  focus(): void;
  hasFocus(): boolean;
  /// Re-measure and put back the last scroll position the user left (a
  /// re-parent resets `.cm-scroller`'s scrollTop behind CM6's back — rule
  /// 14's hazard). Called one frame after the pane is shown.
  restoreScroll(): void;
  destroy(): void;
}

/// Colours beyond the 8 ytheme ones, for tags the mapping leaves out
/// (markdown headings, links, markup tags). Derived from the same palette so
/// a user's theme still reads as one.
function extraStyles(s: SyntaxColors | null): Parameters<typeof HighlightStyle.define>[0] {
  const rules = syntaxRules(s);
  const color = (f: keyof SyntaxColors) => rules.find((r) => r.field === f)!.color;
  return [
    { tag: tags.heading, color: color("keyword"), fontWeight: "600" },
    { tag: tags.strong, fontWeight: "600" },
    { tag: tags.emphasis, fontStyle: "italic" },
    { tag: tags.strikethrough, textDecoration: "line-through" },
    { tag: [tags.link, tags.url], color: color("function"), textDecoration: "underline" },
    { tag: [tags.tagName, tags.self, tags.atom], color: color("keyword") },
    { tag: [tags.attributeName, tags.labelName], color: color("type_name") },
    { tag: [tags.meta, tags.processingInstruction], color: color("comment") },
    { tag: [tags.regexp, tags.escape], color: color("number") },
    { tag: tags.invalid, color: "var(--status-critical)" },
  ];
}

export function createEditor(parent: HTMLElement, o: EditorHandleOptions): EditorHandle {
  const lang = new Compartment();
  const readOnly = new Compartment();
  const chrome = new Compartment();
  let fontSize = o.fontSize;
  let loadGen = 0;
  let scroll: ReturnType<EditorView["scrollSnapshot"]> | null = null;

  const style = HighlightStyle.define([
    ...syntaxRules(o.syntax).map((r) => ({ tag: r.tag as never, color: r.color })),
    ...extraStyles(o.syntax),
  ]);

  const save = () => {
    o.onSave();
    return true;
  };

  function extensions(ro: boolean): Extension[] {
    return [
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightSpecialChars(),
      history(),
      foldGutter({ openText: "▾", closedText: "▸" }),
      drawSelection(),
      dropCursor(),
      EditorState.allowMultipleSelections.of(true),
      indentOnInput(),
      syntaxHighlighting(style),
      bracketMatching(),
      rectangularSelection(),
      crosshairCursor(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      search({ top: true }),
      keymap.of(editorKeymap(o.isMac, { save })),
      EditorView.contentAttributes.of({
        spellcheck: "false",
        autocorrect: "off",
        autocapitalize: "off",
      }),
      chrome.of(EditorView.theme(chromeSpec(fontSize), { dark: true })),
      readOnly.of(EditorState.readOnly.of(ro)),
      lang.of([]),
      EditorView.updateListener.of((u) => {
        if (u.docChanged) o.onDocChange();
        if (u.docChanged || u.selectionSet) o.onSelection();
      }),
    ];
  }

  const view = new EditorView({
    parent,
    state: EditorState.create({ doc: "", extensions: extensions(false) }),
  });
  // The scroll position is remembered from the user's own scrolling. A jump
  // to the very top with no input behind it is the re-parent reset (rule 14)
  // — the browser zeroes `scrollTop` when SplitContainer / PaneGroup move the
  // element — and must not overwrite the real position.
  let lastInput = 0;
  const noteInput = () => {
    lastInput = Date.now();
  };
  for (const type of ["wheel", "pointerdown", "keydown", "touchstart"]) {
    view.dom.addEventListener(type, noteInput, { capture: true, passive: true });
  }
  view.scrollDOM.addEventListener("scroll", () => {
    if (!view.dom.isConnected || view.scrollDOM.clientHeight === 0) return;
    if (view.scrollDOM.scrollTop === 0 && Date.now() - lastInput > 500) return;
    scroll = view.scrollSnapshot();
  });

  function setLanguage(id: LangId | null): void {
    const gen = ++loadGen;
    if (!id) return;
    loadLanguage(id)
      .then((ext) => {
        if (gen === loadGen) view.dispatch({ effects: lang.reconfigure(ext) });
      })
      .catch((e) => console.warn(`editor: grammar ${id} failed to load`, e));
  }

  return {
    view,
    load(text, lo) {
      const state = EditorState.create({ doc: text, extensions: extensions(lo.readOnly) });
      view.setState(state);
      scroll = null;
      if (lo.cursor) {
        const doc = view.state.doc;
        const c = cursorAfterReload(lo.cursor.line, lo.cursor.col, doc.lines, (n) => doc.line(n).length);
        const pos = doc.line(c.line).from + c.col;
        view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
      }
      setLanguage(lo.lang);
    },
    replaceAll(text) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
    },
    doc: () => view.state.doc,
    text: () => view.state.doc.toString(),
    cursor() {
      const head = view.state.selection.main.head;
      const line = view.state.doc.lineAt(head);
      return { line: line.number, col: head - line.from };
    },
    lineLength: (n) => view.state.doc.line(n).length,
    setFontSize(px) {
      fontSize = px;
      view.dispatch({ effects: chrome.reconfigure(EditorView.theme(chromeSpec(fontSize), { dark: true })) });
    },
    undo: () => void undo(view),
    redo: () => void redo(view),
    openSearch: () => void openSearchPanel(view),
    focus: () => view.focus(),
    hasFocus: () => view.hasFocus,
    restoreScroll() {
      if (!view.dom.isConnected || view.scrollDOM.clientHeight === 0) return;
      view.requestMeasure();
      if (scroll) view.dispatch({ effects: scroll });
    },
    destroy: () => view.destroy(),
  };
}
