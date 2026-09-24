//! Who may call which `#[tauri::command]`.
//!
//! ymux has no declarative ACL for its own commands: `build.rs` is a bare
//! `tauri_build::build()` with no `AppManifest`, and Tauri only consults the
//! capability files for an app command when one exists (`tauri-2.10.3`
//! `src/webview/mod.rs:1802`). Every page ymux loads can therefore reach every
//! command — an `eb-*` embedded-browser child webview, and on Windows even a
//! `browser` pane's `<iframe>` (see [`crate::fspath::origin_is_local`]). The
//! gate has to be the first line of each command, and there are exactly two:
//!
//!  - [`crate::fspath::guard_local`] — ymux's own document only: label `main`
//!    **and** an `Origin` derived from configuration. Every command except
//!    the two below.
//!  - `embedded_browser::guard_embedded_child` — a live `eb-<uuid>` child
//!    webview only, for the commands in [`EMBEDDED_CHILD_COMMANDS`].
//!
//! The test at the bottom of this file reads `main.rs`'s `generate_handler!`
//! and every command's body, and fails if a registered command does not start
//! with one of them. CLAUDE.md rule 16 points here.

use uuid::Uuid;

/// Commands an `eb-*` child page legitimately invokes — from the
/// initialization script in `embedded_browser::child_init_script`, and
/// nothing else. Each is callable by **any website** the user opens in an
/// embedded browser pane, so each must stay unable to do more than its name
/// says:
///
///  - `child_webview_focused` — tells the main window which pane was clicked.
///    The pane id is taken from the caller's own label, so a page can only
///    ever report *itself* as focused.
///  - `forward_keystroke` — replays a ymux global shortcut while the child
///    owns OS keyboard focus. Only the fixed table in
///    [`forwarded_shortcut_key`] is accepted and the key is derived from it,
///    so it cannot be used to type into the main window (let alone into a
///    terminal) or to trigger a destructive shortcut such as close-pane.
///
/// Adding to this list means a website can call the command. Don't.
pub const EMBEDDED_CHILD_COMMANDS: &[&str] = &["child_webview_focused", "forward_keystroke"];

/// Label prefix of an embedded-browser child webview (`eb-<pane uuid>`).
pub const EMBEDDED_CHILD_PREFIX: &str = "eb-";

/// The pane id an embedded-browser child's label encodes, or `None` if the
/// label is not exactly `eb-` followed by a UUID in its canonical lowercase
/// hyphenated form — which is the only form `create_embedded_browser` is ever
/// handed, because the frontend passes a `PaneSpec.id` (a Rust `Uuid`).
///
/// Requiring the canonical spelling means one pane has one label, so the
/// comparison against the registry and the id the frontend receives cannot
/// disagree over braces, case or a `urn:uuid:` prefix.
pub fn embedded_child_pane_id(label: &str) -> Option<Uuid> {
    let raw = label.strip_prefix(EMBEDDED_CHILD_PREFIX)?;
    let id = Uuid::parse_str(raw).ok()?;
    (id.hyphenated().to_string() == raw).then_some(id)
}

/// The exact shortcuts an embedded browser may forward, as
/// `(code, ctrl, shift, alt) -> key`. The `key` is **derived here**, never
/// taken from the page: `main.ts`'s keydown handler dispatches on `ev.key`
/// for most bindings, so trusting a page-supplied `key` would let
/// `{code: "Tab", key: "W"}` pass validation and act as a different
/// shortcut.
///
/// Deliberately absent: Ctrl+Shift+W (close pane) — closing a pane kills its
/// shell or agent and deletes its scrollback with no confirmation — and
/// Ctrl+Shift+H / V (split) and Ctrl+Shift+T (new tab), which each start a
/// new shell, so a page repeating them could spawn PTYs without limit.
/// Nothing that destroys or creates a PTY may be triggerable from a web
/// page; the user presses those after clicking back into ymux. Keep `isYmuxShortcut` in `embedded_browser::child_init_script` and
/// `src/browser/forwardedKeys.ts` in step with this table.
pub fn forwarded_shortcut_key(
    code: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Option<&'static str> {
    if !ctrl {
        return None;
    }
    match (shift, alt) {
        // Ctrl+Alt+1..9 switch workspace, Ctrl+Alt+N notes.
        (false, true) => match code {
            "Digit1" => Some("1"),
            "Digit2" => Some("2"),
            "Digit3" => Some("3"),
            "Digit4" => Some("4"),
            "Digit5" => Some("5"),
            "Digit6" => Some("6"),
            "Digit7" => Some("7"),
            "Digit8" => Some("8"),
            "Digit9" => Some("9"),
            "KeyN" => Some("n"),
            _ => None,
        },
        // Ctrl+Shift+Z/P/R/E, Ctrl+Shift+[ / ], Ctrl+Shift+Tab.
        (true, false) => match code {
            "KeyZ" => Some("Z"),
            "KeyP" => Some("P"),
            "KeyR" => Some("R"),
            "KeyE" => Some("E"),
            "BracketLeft" => Some("{"),
            "BracketRight" => Some("}"),
            "Tab" => Some("Tab"),
            _ => None,
        },
        // Ctrl+Tab.
        (false, false) => (code == "Tab").then_some("Tab"),
        (true, true) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    const PANE: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    #[test]
    fn embedded_child_label_must_be_eb_plus_canonical_uuid() {
        let id = embedded_child_pane_id(&format!("eb-{PANE}")).expect("canonical label");
        assert_eq!(id.to_string(), PANE);

        for bad in [
            "main".to_string(),
            "eb-".to_string(),
            "eb-main".to_string(),
            PANE.to_string(),
            format!("browser-{PANE}"),
            format!("EB-{PANE}"),
            format!("eb-{}", PANE.to_uppercase()),
            format!("eb-{{{PANE}}}"),
            format!("eb-urn:uuid:{PANE}"),
            format!("eb-{}", PANE.replace('-', "")),
            format!("eb-{PANE} "),
            format!("eb-{PANE}x"),
            format!("eb-eb-{PANE}"),
        ] {
            assert_eq!(embedded_child_pane_id(&bad), None, "{bad:?}");
        }
    }

    #[test]
    fn forwarded_shortcuts_derive_their_key_from_the_table() {
        for d in 1..=9 {
            let key = forwarded_shortcut_key(&format!("Digit{d}"), true, false, true);
            assert_eq!(key, Some(d.to_string()).as_deref());
        }
        assert_eq!(forwarded_shortcut_key("KeyN", true, false, true), Some("n"));
        for (code, key) in [
            ("KeyZ", "Z"),
            ("KeyP", "P"),
            ("KeyR", "R"),
            ("KeyE", "E"),
            ("BracketLeft", "{"),
            ("BracketRight", "}"),
            ("Tab", "Tab"),
        ] {
            assert_eq!(forwarded_shortcut_key(code, true, true, false), Some(key));
        }
        assert_eq!(
            forwarded_shortcut_key("Tab", true, false, false),
            Some("Tab")
        );
    }

    /// Close-pane is destructive (kills the shell, deletes scrollback) and
    /// must not be reachable by a website at all.
    #[test]
    fn close_pane_is_not_forwardable() {
        for (shift, alt) in [(true, false), (false, false), (false, true), (true, true)] {
            assert_eq!(forwarded_shortcut_key("KeyW", true, shift, alt), None);
        }
    }

    /// Split (H/V) and new tab (T) each spawn a shell; a page repeating them
    /// could create PTYs without limit.
    #[test]
    fn pty_spawning_shortcuts_are_not_forwardable() {
        for code in ["KeyH", "KeyV", "KeyT"] {
            for (shift, alt) in [(true, false), (false, false), (false, true), (true, true)] {
                assert_eq!(
                    forwarded_shortcut_key(code, true, shift, alt),
                    None,
                    "{code} shift={shift} alt={alt}"
                );
            }
        }
    }

    #[test]
    fn modifier_mismatches_and_other_keys_are_refused() {
        for (code, ctrl, shift, alt) in [
            ("KeyA", false, false, false),
            ("KeyH", false, true, false),
            ("KeyH", true, false, false),
            ("KeyH", true, true, true),
            ("KeyH", true, false, true),
            ("KeyA", true, true, false),
            ("KeyC", true, true, false),
            ("KeyF", true, false, false),
            ("Enter", true, false, false),
            ("Digit0", true, false, true),
            ("Digit1", true, true, true),
            ("Digit1", true, true, false),
            ("Digit1", true, false, false),
            ("Digit10", true, false, true),
            ("Digit", true, false, true),
            ("KeyN", true, true, true),
            ("Tab", false, false, false),
            ("Tab", true, false, true),
            ("Tab", true, true, true),
            ("", true, true, false),
            ("keyh", true, true, false),
        ] {
            assert_eq!(
                forwarded_shortcut_key(code, ctrl, shift, alt),
                None,
                "{code} ctrl={ctrl} shift={shift} alt={alt}"
            );
        }
    }

    // -----------------------------------------------------------------
    // Enforcement: every registered command starts with a guard.
    //
    // Command bodies are parsed with `syn`, so comments are not tokens and
    // cannot impersonate a guard (`/* guard_local(..) */ evil()?;`).
    // -----------------------------------------------------------------

    use quote::ToTokens;

    fn src_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Command names listed in `main.rs`'s `generate_handler![...]`, read
    /// from the macro's tokens (so a commented-out entry does not count).
    fn registered_commands() -> Vec<String> {
        let main = std::fs::read_to_string(src_dir().join("main.rs")).expect("read main.rs");
        let tokens: proc_macro2::TokenStream = main.parse().expect("main.rs tokenizes");
        let mut out = Vec::new();
        find_handler_list(tokens, &mut out);
        out
    }

    fn find_handler_list(tokens: proc_macro2::TokenStream, out: &mut Vec<String>) {
        use proc_macro2::TokenTree;
        let mut prev_was_handler_bang = 0u8; // 1 after `generate_handler`, 2 after `!`
        for tt in tokens {
            match &tt {
                TokenTree::Ident(i) if i == "generate_handler" => prev_was_handler_bang = 1,
                TokenTree::Punct(p) if p.as_char() == '!' && prev_was_handler_bang == 1 => {
                    prev_was_handler_bang = 2
                }
                TokenTree::Group(g) if prev_was_handler_bang == 2 => {
                    let list: syn::punctuated::Punctuated<syn::Path, syn::Token![,]> =
                        syn::parse::Parser::parse2(
                            syn::punctuated::Punctuated::parse_terminated,
                            g.stream(),
                        )
                        .expect("generate_handler! holds a path list");
                    for p in list {
                        out.push(p.segments.last().expect("non-empty path").ident.to_string());
                    }
                    prev_was_handler_bang = 0;
                }
                TokenTree::Group(g) => {
                    prev_was_handler_bang = 0;
                    find_handler_list(g.stream(), out);
                }
                _ => prev_was_handler_bang = 0,
            }
        }
    }

    fn is_tauri_command(attr: &syn::Attribute) -> bool {
        let segs: Vec<String> = attr
            .path()
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        segs == ["tauri", "command"]
    }

    /// Every `#[tauri::command]` fn in `src`, with its first statement as
    /// whitespace-free tokens (`None` for an empty body).
    fn commands_in(src: &str) -> Vec<(String, Option<String>)> {
        fn walk(items: &[syn::Item], out: &mut Vec<(String, Option<String>)>) {
            for item in items {
                match item {
                    syn::Item::Fn(f) if f.attrs.iter().any(is_tauri_command) => {
                        let first = f.block.stmts.first().map(|s| {
                            s.to_token_stream()
                                .to_string()
                                .chars()
                                .filter(|c| !c.is_whitespace())
                                .collect()
                        });
                        out.push((f.sig.ident.to_string(), first));
                    }
                    syn::Item::Mod(m) => {
                        if let Some((_, items)) = &m.content {
                            walk(items, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        let file = syn::parse_file(src).expect("source parses");
        let mut out = Vec::new();
        walk(&file.items, &mut out);
        out
    }

    /// Is `stmt` (whitespace-free tokens) exactly an accepted guard call for
    /// command `name`? Exact forms only, so `guard_local(..).ok();` or a
    /// guard buried in a larger expression does not count.
    fn is_guard_statement(name: &str, stmt: &str, embedded_child: bool) -> bool {
        let q = format!("\"{name}\"");
        let accepted: Vec<String> = if embedded_child {
            vec![
                format!("guard_embedded_child(&webview,&registry,{q})?;"),
                format!("letpane=guard_embedded_child(&webview,&registry,{q})?;"),
            ]
        } else {
            vec![
                format!("guard_local(&webview,&request,{q})?;"),
                format!("guard_local(&webview,&request,{q}).map_err(|e|e.to_string())?;"),
                format!("guarded!(webview,request,{q});"),
            ]
        };
        accepted.iter().any(|a| a == stmt)
    }

    fn defined_commands() -> Vec<(String, PathBuf, Option<String>)> {
        let mut files = Vec::new();
        rust_files(&src_dir(), &mut files);
        let mut out = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).expect("read source");
            for (name, first) in commands_in(&text) {
                out.push((name, file.clone(), first));
            }
        }
        out
    }

    /// A guard that is only in a comment — block, line or trailing — is not
    /// a guard.
    #[test]
    fn a_commented_out_guard_does_not_count() {
        let src = r#"
            #[tauri::command]
            pub fn block(webview: W, request: R) -> Res<()> {
                /* guard_local(&webview, &request, "block")?; */
                spawn()?;
                Ok(())
            }
            #[tauri::command]
            pub fn line(webview: W, request: R) -> Res<()> {
                // guard_local(&webview, &request, "line")?;
                spawn()?;
                Ok(())
            }
            #[tauri::command]
            pub fn trailing(webview: W, request: R) -> Res<()> {
                spawn() /* guard_local(&webview, &request, "trailing") */ ?;
                Ok(())
            }
            #[tauri::command]
            pub fn swallowed(webview: W, request: R) -> Res<()> {
                guard_local(&webview, &request, "swallowed").ok();
                Ok(())
            }
            #[tauri::command]
            pub fn empty() {}
            #[tauri::command(async)]
            pub fn good(webview: W, request: R) -> Res<()> {
                // A comment before the guard is fine.
                guard_local(
                    &webview, &request, "good"
                )?;
                Ok(())
            }
        "#;
        let found = commands_in(src);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["block", "line", "trailing", "swallowed", "empty", "good"]
        );
        for (name, first) in &found {
            let ok = first
                .as_deref()
                .is_some_and(|s| is_guard_statement(name, s, false));
            assert_eq!(ok, name == "good", "{name}: {first:?}");
        }
        // The name inside the guard must be this command's own.
        assert!(!is_guard_statement(
            "other",
            "guard_local(&webview,&request,\"good\")?;",
            false
        ));
    }

    /// The test that keeps the hole closed. A new command fails CI until it
    /// begins with `guard_local` — or, if a website genuinely must call it,
    /// with `guard_embedded_child` *and* an entry in
    /// [`EMBEDDED_CHILD_COMMANDS`] explaining why.
    #[test]
    fn every_registered_command_starts_with_a_guard() {
        let registered = registered_commands();
        let defined = defined_commands();
        assert!(
            registered.len() >= 60,
            "parsed only {} commands out of generate_handler! — parser broken?",
            registered.len()
        );

        let reg: BTreeSet<&str> = registered.iter().map(String::as_str).collect();
        assert_eq!(reg.len(), registered.len(), "duplicate registration");
        let def: BTreeSet<&str> = defined.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            reg, def,
            "generate_handler! in main.rs and the #[tauri::command] fns in src/ disagree"
        );
        for eb in EMBEDDED_CHILD_COMMANDS {
            assert!(reg.contains(eb), "{eb} is allow-listed but not registered");
        }

        let mut failures = Vec::new();
        for (name, file, first) in &defined {
            let eb = EMBEDDED_CHILD_COMMANDS.contains(&name.as_str());
            let ok = first
                .as_deref()
                .is_some_and(|s| is_guard_statement(name, s, eb));
            if !ok {
                failures.push(format!("{name} ({}): {first:?}", file.display()));
            }
        }
        assert!(
            failures.is_empty(),
            "commands whose first statement is not their guard:\n  {}",
            failures.join("\n  ")
        );
    }
}
