// Grammar loading, one dynamic import per language (spec §3.3), so opening a
// `.rs` file never downloads the Python grammar. The decision of *which*
// language a path is lives in `editorModel.languageForPath` (pure, tested);
// this file is only the loader table, and the only place a grammar package
// is imported.

import type { Extension } from "@codemirror/state";
import type { LangId } from "./editorModel";

/// The grammar extension for `id`. Rejects only if the chunk fails to load,
/// which the pane treats as "plain text".
export async function loadLanguage(id: LangId): Promise<Extension> {
  switch (id) {
    case "rust":
      return (await import("@codemirror/lang-rust")).rust();
    case "javascript":
      return (await import("@codemirror/lang-javascript")).javascript();
    case "jsx":
      return (await import("@codemirror/lang-javascript")).javascript({ jsx: true });
    case "typescript":
      return (await import("@codemirror/lang-javascript")).javascript({ typescript: true });
    case "tsx":
      return (await import("@codemirror/lang-javascript")).javascript({ typescript: true, jsx: true });
    case "json":
      return (await import("@codemirror/lang-json")).json();
    case "html":
      return (await import("@codemirror/lang-html")).html();
    case "css":
      return (await import("@codemirror/lang-css")).css();
    case "markdown":
      return (await import("@codemirror/lang-markdown")).markdown();
    case "python":
      return (await import("@codemirror/lang-python")).python();
    case "yaml":
      return (await import("@codemirror/lang-yaml")).yaml();
  }
}
