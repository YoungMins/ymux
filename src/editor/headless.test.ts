// CodeMirror's state layer, headless in node (spec §3.8 / §7.1): the
// behaviours the editor pane relies on, checked without a DOM.

import { describe, expect, it } from "vitest";
import { EditorState, Transaction, type TransactionSpec } from "@codemirror/state";
import { history, undo, redo, undoDepth } from "@codemirror/commands";
import { SearchCursor, SearchQuery } from "@codemirror/search";
import { isDocDirty } from "./editorModel";

function run(
  state: EditorState,
  cmd: (t: { state: EditorState; dispatch: (tr: Transaction) => void }) => boolean,
): EditorState {
  let next = state;
  cmd({ state, dispatch: (tr) => (next = tr.state) });
  return next;
}

function typeAt(state: EditorState, text: string, time: number): EditorState {
  const pos = state.selection.main.head;
  const spec: TransactionSpec = {
    changes: { from: pos, insert: text },
    selection: { anchor: pos + text.length },
    userEvent: "input.type",
    annotations: Transaction.time.of(time),
  };
  return state.update(spec).state;
}

describe("undo history", () => {
  it("coalesces a typing burst into one undo step", () => {
    let s = EditorState.create({ doc: "", extensions: [history()] });
    s = typeAt(s, "안", 1000);
    s = typeAt(s, "녕", 1050);
    s = typeAt(s, "!", 1100);
    expect(s.doc.toString()).toBe("안녕!");
    expect(undoDepth(s)).toBe(1);
    s = run(s, undo);
    expect(s.doc.toString()).toBe("");
  });

  it("starts a new step after a pause", () => {
    let s = EditorState.create({ doc: "", extensions: [history()] });
    s = typeAt(s, "a", 1000);
    s = typeAt(s, "b", 5000);
    expect(undoDepth(s)).toBe(2);
    s = run(s, undo);
    expect(s.doc.toString()).toBe("a");
  });

  it("undoing back to the saved text is clean again; redo makes it dirty", () => {
    let s = EditorState.create({ doc: "saved\n", extensions: [history()] });
    const saved = s.doc;
    s = typeAt(s, "x", 1000);
    expect(isDocDirty(saved, s.doc)).toBe(true);
    s = run(s, undo);
    expect(isDocDirty(saved, s.doc)).toBe(false);
    s = run(s, redo);
    expect(isDocDirty(saved, s.doc)).toBe(true);
  });

  it("an edit reverted by hand is clean too (content, not history)", () => {
    let s = EditorState.create({ doc: "abc", extensions: [history()] });
    const saved = s.doc;
    s = s.update({ changes: { from: 1, to: 2, insert: "Z" } }).state;
    s = s.update({ changes: { from: 1, to: 2, insert: "b" } }).state;
    expect(isDocDirty(saved, s.doc)).toBe(false);
  });
});

describe("search over Hangul (ycode mixed byte and char offsets)", () => {
  const doc = "let 이름 = \"한글 문자열\";\n// 한글 주석 한글\n";

  it("finds every match at the right character offsets", () => {
    const state = EditorState.create({ doc });
    const cur = new SearchCursor(state.doc, "한글");
    const hits: number[] = [];
    while (!cur.next().done) hits.push(cur.value.from);
    const expected: number[] = [];
    for (let i = doc.indexOf("한글"); i >= 0; i = doc.indexOf("한글", i + 1)) expected.push(i);
    expect(hits).toEqual(expected);
    expect(hits).toHaveLength(3);
  });

  it("a replace-all over Hangul lands on the matches, not beside them", () => {
    const state = EditorState.create({ doc });
    const query = new SearchQuery({ search: "한글", replace: "Hangul" });
    const cursor = query.getCursor(state);
    const changes: { from: number; to: number; insert: string }[] = [];
    for (let r = cursor.next(); !r.done; r = cursor.next()) {
      changes.push({ from: r.value.from, to: r.value.to, insert: "Hangul" });
    }
    const out = state.update({ changes }).state.doc.toString();
    expect(out).toBe(doc.split("한글").join("Hangul"));
  });
});
