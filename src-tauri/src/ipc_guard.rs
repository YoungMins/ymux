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
///    owns OS keyboard focus. Only the fixed shortcut set in
///    [`is_forwardable_shortcut`] is accepted, so it cannot be used to type
///    into the main window (let alone into a terminal).
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

/// Is this keystroke one of ymux's global shortcuts that a child page may
/// forward? Mirrors `isYmuxShortcut` in `child_init_script` exactly — the JS
/// is only a convenience filter; this is the check that holds, because a
/// hostile page can call `forward_keystroke` with anything.
pub fn is_forwardable_shortcut(code: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
    if !ctrl {
        return false;
    }
    if code == "Tab" {
        return true;
    }
    if alt && !shift {
        if code == "KeyN" {
            return true;
        }
        return matches!(code.strip_prefix("Digit"), Some(d) if d.len() == 1 && ('1'..='9').contains(&d.chars().next().unwrap_or('0')));
    }
    if shift && !alt {
        if code == "BracketLeft" || code == "BracketRight" {
            return true;
        }
        return matches!(
            code,
            "KeyH" | "KeyV" | "KeyW" | "KeyZ" | "KeyP" | "KeyR" | "KeyE" | "KeyT"
        );
    }
    false
}

/// Longest `KeyboardEvent.key` value forwarded. Real values for the shortcut
/// set are one or three characters (`"W"`, `"Tab"`, `"{"`); the cap only keeps
/// a hostile page from pushing an arbitrary blob through the event bus.
pub const MAX_FORWARDED_KEY_LEN: usize = 16;

/// Is `key` a plausible `KeyboardEvent.key` for a forwarded shortcut?
pub fn is_plausible_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_FORWARDED_KEY_LEN && !key.chars().any(char::is_control)
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
    fn only_ymux_shortcuts_are_forwardable() {
        // Every shape `isYmuxShortcut` in `child_init_script` accepts.
        for d in 1..=9 {
            assert!(is_forwardable_shortcut(
                &format!("Digit{d}"),
                true,
                false,
                true
            ));
        }
        assert!(is_forwardable_shortcut("KeyN", true, false, true));
        for k in ["H", "V", "W", "Z", "P", "R", "E", "T"] {
            assert!(is_forwardable_shortcut(
                &format!("Key{k}"),
                true,
                true,
                false
            ));
        }
        assert!(is_forwardable_shortcut("BracketLeft", true, true, false));
        assert!(is_forwardable_shortcut("BracketRight", true, true, false));
        assert!(is_forwardable_shortcut("Tab", true, false, false));
        assert!(is_forwardable_shortcut("Tab", true, true, false));

        // Anything that would amount to typing, or a modifier mismatch.
        for (code, ctrl, shift, alt) in [
            ("KeyA", false, false, false),
            ("KeyW", false, true, false),
            ("KeyW", true, false, false),
            ("KeyW", true, true, true),
            ("KeyA", true, true, false),
            ("KeyC", true, true, false),
            ("Enter", true, false, false),
            ("Digit0", true, false, true),
            ("Digit1", true, true, true),
            ("Digit10", true, false, true),
            ("Digit", true, false, true),
            ("KeyN", true, true, true),
            ("Tab", false, false, false),
            ("", true, true, false),
        ] {
            assert!(
                !is_forwardable_shortcut(code, ctrl, shift, alt),
                "{code} ctrl={ctrl} shift={shift} alt={alt}"
            );
        }
    }

    #[test]
    fn forwarded_key_must_be_short_and_printable() {
        assert!(is_plausible_key("W"));
        assert!(is_plausible_key("Tab"));
        assert!(is_plausible_key("{"));
        assert!(!is_plausible_key(""));
        assert!(!is_plausible_key("a\r"));
        assert!(!is_plausible_key(&"x".repeat(MAX_FORWARDED_KEY_LEN + 1)));
    }

    // -----------------------------------------------------------------
    // Enforcement: every registered command starts with a guard.
    // -----------------------------------------------------------------

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

    /// Command names listed in `main.rs`'s `generate_handler![...]`.
    fn registered_commands() -> Vec<String> {
        let main = std::fs::read_to_string(src_dir().join("main.rs")).expect("read main.rs");
        let start = main
            .find("generate_handler![")
            .expect("main.rs has generate_handler!");
        let body = &main[start + "generate_handler![".len()..];
        let body = &body[..body.find(']').expect("generate_handler! is closed")];
        body.lines()
            .map(|l| l.split("//").next().unwrap_or("").trim())
            .filter(|l| !l.is_empty())
            .map(|l| {
                let path = l.trim_end_matches(',').trim();
                path.rsplit("::").next().unwrap_or(path).to_string()
            })
            .collect()
    }

    /// Every `#[tauri::command]` fn in `src/`, with the first statement of
    /// its body (comments and blank lines skipped, up to the first `;`).
    fn defined_commands() -> Vec<(String, PathBuf, String)> {
        let mut files = Vec::new();
        rust_files(&src_dir(), &mut files);
        let mut out = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).expect("read source");
            // Only a line that *is* the attribute counts — doc comments and
            // this test's own strings mention it too.
            let mut offset = 0;
            let mut attrs = Vec::new();
            for line in text.split_inclusive('\n') {
                if line.trim_start().starts_with("#[tauri::command") {
                    attrs.push(offset + line.len());
                }
                offset += line.len();
            }
            for at in attrs {
                let rest = &text[at..];
                let fn_at = rest.find("fn ").expect("attribute is followed by a fn");
                let after = &rest[fn_at + 3..];
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let body = &after[after.find('{').expect("fn has a body") + 1..];
                let code: String = body
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with("//"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let first = code[..code.find(';').unwrap_or(code.len())].to_string();
                out.push((name, file.clone(), first));
            }
        }
        out
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
            let ok = if EMBEDDED_CHILD_COMMANDS.contains(&name.as_str()) {
                first.contains(&format!(
                    "guard_embedded_child(&webview, &registry, \"{name}\")"
                ))
            } else {
                first.contains(&format!("guard_local(&webview, &request, \"{name}\")"))
                    || first.contains(&format!("guarded!(webview, request, \"{name}\")"))
            };
            // `guarded!` expands to `...?`; every other form must propagate
            // the rejection itself rather than compute and drop it.
            let propagates = first.ends_with('?') || first.starts_with("guarded!");
            if !(ok && propagates) {
                failures.push(format!("{name} ({}): {first}", file.display()));
            }
        }
        assert!(
            failures.is_empty(),
            "commands whose first statement is not their guard:\n  {}",
            failures.join("\n  ")
        );
    }
}
