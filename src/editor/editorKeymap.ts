// The editor pane's assembled keymap: CodeMirror's stock keymaps run through
// `filterKeymap` (ymux globals always win, spec §0.6) plus the few bindings
// ymux adds or moves. Imports CM6 values, so only the lazily loaded editor
// chunk (`cmSetup.ts`) and tests may import this file.
//
// The settled table (step 3's keymap task; see keymap.test.ts):
//
//   Ctrl/Cmd+S        save                         (new, pane-local)
//   Ctrl/Cmd+G        go to line                   (ycode's binding; CM6's
//                                                   find-next moves to F3 /
//                                                   Enter in the search box)
//   Ctrl/Cmd+Y        redo, on macOS too           (Cmd+Shift+Z is ymux zoom)
//   Ctrl/Cmd+F        NOT bound here: main.ts owns it and routes it to
//                     EditorPane.toggleSearch(), which opens CM6's panel —
//                     the one deliberate "editor wins" chord, delivered
//                     through ymux rather than around it
//   everything under Ctrl/Cmd+Shift and Ctrl/Cmd+Alt — dropped (fold,
//                     delete-line, select-all-matches, add-cursor, …): that
//                     namespace is ymux's, as in the files pane

import type { KeyBinding, Command } from "@codemirror/view";
import { defaultKeymap, historyKeymap, indentWithTab, redo } from "@codemirror/commands";
import { searchKeymap, gotoLine } from "@codemirror/search";
import { filterKeymap } from "./keymap";

export interface EditorKeyActions {
  save: Command;
}

/// The keymap the editor view installs on this platform.
export function editorKeymap(isMac: boolean, actions: EditorKeyActions): KeyBinding[] {
  const own: KeyBinding[] = [
    { key: "Mod-s", run: actions.save, preventDefault: true },
    { key: "Mod-g", run: gotoLine, preventDefault: true },
  ];
  // historyKeymap puts redo on Cmd+Shift+Z on macOS only, which the filter
  // drops (ymux zoom); Mod-y is already redo everywhere else.
  if (isMac) own.push({ key: "Mod-y", run: redo, preventDefault: true });
  const search = searchKeymap.filter((b) => b.key !== "Mod-f" && b.key !== "Mod-g");
  const stock = filterKeymap<Command>(
    [...search, ...defaultKeymap, ...historyKeymap, indentWithTab],
    isMac,
  ) as KeyBinding[];
  return [...own, ...stock];
}
