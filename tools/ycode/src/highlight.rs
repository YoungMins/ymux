//! Syntax highlighting for yCode. Loads syntect's bundled SyntaxSet
//! (~200 languages — Rust, TS/JS, HTML, CSS, JSON, YAML, XML, TOML, C/C++/C#,
//! Python, Go, Markdown, shell, SQL, etc.), splices in the Svelte definition
//! from `assets/syntaxes/svelte.sublime-syntax`, and constructs a syntect
//! `Theme` directly from the ymux palette (`ytheme::Theme.syntax`).
//!
//! State coherence: `HighlightLines` is stateful (block comments, template
//! strings span lines), so `highlight_range` walks from line 0 every call
//! and only emits spans for the visible window.

use std::path::Path;
use std::str::FromStr;

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, ScopeSelectors, Style, StyleModifier, Theme as SyntectTheme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxDefinition, SyntaxSet};

use ytheme::Theme as YTheme;

const SVELTE_SYNTAX: &str = include_str!("../assets/syntaxes/svelte.sublime-syntax");

/// One line worth of highlighted spans: (style, text-without-trailing-newline).
pub type LineHighlights = Vec<(Style, String)>;

pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme: SyntectTheme,
    /// Cached syntax name for the current file — None means "no recognized
    /// extension"; we fall back to plain-text rendering with the default fg.
    syntax_name: Option<String>,
}

impl Highlighter {
    pub fn new(file_path: Option<&Path>, ytheme: &YTheme) -> Self {
        let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
        let svelte = SyntaxDefinition::load_from_str(SVELTE_SYNTAX, true, None)
            .expect("bundled svelte.sublime-syntax must parse");
        builder.add(svelte);
        let syntax_set = builder.build();
        let syntax_name = detect_syntax(&syntax_set, file_path);
        let theme = build_theme(ytheme);
        Self {
            syntax_set,
            theme,
            syntax_name,
        }
    }

    /// Re-detect language for a new file path. Cheap — just an extension
    /// lookup against the already-built SyntaxSet.
    pub fn set_file(&mut self, file_path: Option<&Path>) {
        self.syntax_name = detect_syntax(&self.syntax_set, file_path);
    }

    /// Get the language name being highlighted (or "Plain Text").
    pub fn language_name(&self) -> &str {
        self.syntax_name.as_deref().unwrap_or("Plain Text")
    }

    /// Walk highlighter state from line 0 through `end_exclusive`, returning
    /// spans for rows in `start..end_exclusive`. Walking from the start keeps
    /// state coherent across multi-line constructs.
    pub fn highlight_range(
        &self,
        lines: &[String],
        start: usize,
        end_exclusive: usize,
    ) -> Vec<LineHighlights> {
        let syntax = self
            .syntax_name
            .as_deref()
            .and_then(|n| self.syntax_set.find_syntax_by_name(n))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        let mut hl = HighlightLines::new(syntax, &self.theme);
        let end = end_exclusive.min(lines.len());
        let mut out = Vec::with_capacity(end.saturating_sub(start));
        for (row, line) in lines.iter().enumerate().take(end) {
            // HighlightLines needs the trailing newline for state to advance
            // (otherwise multi-line constructs leak into the next line).
            let mut buf = String::with_capacity(line.len() + 1);
            buf.push_str(line);
            buf.push('\n');
            let ranges = match hl.highlight_line(&buf, &self.syntax_set) {
                Ok(r) => r,
                Err(_) => {
                    if row >= start {
                        out.push(vec![(Style::default(), line.clone())]);
                    }
                    continue;
                }
            };
            if row < start {
                continue;
            }
            // Drop zero-length spans. syntect's markdown highlighter emits
            // empty trailing/leading spans at scope boundaries (`# heading`
            // ends with one, the inline-code closer adds one, empty lines
            // are one empty span). They contribute no characters but carry
            // a style; ratatui's per-frame buffer diff can confuse those
            // styled phantom positions for real cells, leaving "first
            // letter" ghosts after scrolling through a markdown file.
            let owned: LineHighlights = ranges
                .into_iter()
                .map(|(st, t)| (st, t.strip_suffix('\n').unwrap_or(t).to_string()))
                .filter(|(_, t)| !t.is_empty())
                .collect();
            out.push(owned);
        }
        out
    }
}

fn detect_syntax(set: &SyntaxSet, file_path: Option<&Path>) -> Option<String> {
    file_path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .and_then(|ext| set.find_syntax_by_extension(ext))
        .map(|s| s.name.clone())
}

fn build_theme(yt: &YTheme) -> SyntectTheme {
    let fg = hex(&yt.fg);
    let bg = hex(&yt.bg);
    let comment = hex(&yt.syntax.comment);
    let keyword = hex(&yt.syntax.keyword);
    let string = hex(&yt.syntax.string);
    let number = hex(&yt.syntax.number);
    let function = hex(&yt.syntax.function);
    let type_name = hex(&yt.syntax.type_name);
    let variable = hex(&yt.syntax.variable);
    let punctuation = hex(&yt.syntax.punctuation);

    let settings = ThemeSettings {
        foreground: Some(fg),
        background: Some(bg),
        ..ThemeSettings::default()
    };

    // TextMate scope selectors. The most specific matching selector wins,
    // so we list broad fallbacks (e.g. `storage`) and let user-defined-type
    // selectors (`entity.name.type`) override only where appropriate. This
    // mirrors VS Code's default mapping: `fn`, `let`, `pub`, `struct`, `i32`
    // all read as keywords; only entity-named types (your `MyStruct`) get
    // the type color.
    let scopes = vec![
        item("comment", comment),
        item("string", string),
        item("constant.numeric", number),
        item("constant.language", number),
        item("constant.character.escape", string),
        item("keyword", keyword),
        item("storage", keyword),
        item("entity.name.function", function),
        item("entity.name.type", type_name),
        item("entity.name.class", type_name),
        item("entity.name.tag", keyword),
        item("entity.other.attribute-name", function),
        item("variable", variable),
        item("variable.parameter", variable),
        item("support.function", function),
        item("support.type", type_name),
        item("support.class", type_name),
        item("punctuation", punctuation),
    ];

    SyntectTheme {
        name: Some("ymux".to_string()),
        author: None,
        settings,
        scopes,
    }
}

fn item(selector: &'static str, fg: Color) -> ThemeItem {
    ThemeItem {
        scope: ScopeSelectors::from_str(selector)
            .unwrap_or_else(|_| panic!("hardcoded scope selector must parse: {selector}")),
        style: StyleModifier {
            foreground: Some(fg),
            background: None,
            font_style: None,
        },
    }
}

/// Convert a ytheme `HexColor` (already validated as `#rrggbb`) to a syntect
/// `Color`. `HexColor::to_rgb` returning None means the user wrote malformed
/// TOML — surface it loudly with a panic rather than rendering a silent
/// wrong color.
fn hex(hex: &ytheme::HexColor) -> Color {
    let (r, g, b) = hex
        .to_rgb()
        .unwrap_or_else(|| panic!("malformed hex color in theme: {}", hex.0));
    Color { r, g, b, a: 0xff }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_rust_by_extension() {
        let yt = YTheme::default();
        let h = Highlighter::new(Some(&PathBuf::from("foo.rs")), &yt);
        assert_eq!(h.language_name(), "Rust");
    }

    #[test]
    fn detects_json() {
        let yt = YTheme::default();
        let h = Highlighter::new(Some(&PathBuf::from("foo.json")), &yt);
        assert!(h.language_name().contains("JSON"));
    }

    #[test]
    fn detects_svelte() {
        let yt = YTheme::default();
        let h = Highlighter::new(Some(&PathBuf::from("App.svelte")), &yt);
        assert_eq!(h.language_name(), "Svelte");
    }

    #[test]
    fn unknown_extension_falls_back_to_plain() {
        let yt = YTheme::default();
        let h = Highlighter::new(Some(&PathBuf::from("foo.xyzzy")), &yt);
        assert_eq!(h.language_name(), "Plain Text");
    }

    #[test]
    fn highlights_a_rust_keyword_with_keyword_color() {
        let yt = YTheme::default();
        let h = Highlighter::new(Some(&PathBuf::from("foo.rs")), &yt);
        let lines = vec!["fn main() {}".to_string()];
        let out = h.highlight_range(&lines, 0, 1);
        assert_eq!(out.len(), 1);
        // `fn` should be styled as a keyword — not the same color as the
        // default foreground.
        let fg = &yt.fg.0;
        let kw = &yt.syntax.keyword.0;
        let any_keyword_colored = out[0].iter().any(|(st, t)| {
            let hex = format!(
                "#{:02x}{:02x}{:02x}",
                st.foreground.r, st.foreground.g, st.foreground.b
            );
            t.contains("fn") && hex == *kw && hex != *fg
        });
        assert!(
            any_keyword_colored,
            "expected `fn` to be colored with the keyword color {kw}, got spans: {:?}",
            out[0]
        );
    }

    /// Regression for the markdown scroll-ghost bug: syntect's markdown
    /// highlighter emits zero-length spans at scope boundaries (heading end,
    /// inline-code close, empty lines). `highlight_range` must drop them
    /// before they reach the renderer — otherwise their styled-but-empty
    /// positions confuse ratatui's per-frame cell diff and leave first-letter
    /// ghosts when scrolling through markdown.
    #[test]
    fn markdown_highlight_drops_empty_spans() {
        let yt = YTheme::default();
        let h = Highlighter::new(Some(&PathBuf::from("test.md")), &yt);
        let lines = vec![
            "# Hello World".to_string(),
            "".to_string(),
            "This is **bold** and *italic* text.".to_string(),
            "- A bullet item with `inline code`".to_string(),
        ];
        let highlighted = h.highlight_range(&lines, 0, lines.len());
        for (row, spans) in highlighted.iter().enumerate() {
            for (i, (_, text)) in spans.iter().enumerate() {
                assert!(
                    !text.is_empty(),
                    "row {row} span[{i}] is empty — should be filtered",
                );
            }
        }
        // Sum-of-text-chars must still equal the source — the filter must
        // only drop empty spans, never real characters.
        let total: usize = highlighted
            .iter()
            .map(|spans| spans.iter().map(|(_, t)| t.chars().count()).sum::<usize>())
            .sum();
        let expected: usize = lines.iter().map(|l| l.chars().count()).sum();
        assert_eq!(total, expected);
    }

    /// Diagnostic probe: for the actual problem-report file, dump every
    /// line's span widths and compare to the raw-source width. Any drift
    /// (spans summing to a different cell count than the source line)
    /// pinpoints where syntect's segmentation desyncs from ratatui's
    /// width assumption — the mechanism behind the markdown scroll-ghost.
    #[test]
    fn probe_width_drift_for_release_notes_v0_8_11() {
        use unicode_width::UnicodeWidthStr;
        let path = PathBuf::from("../../.omc/release-notes-v0.8.11.md");
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("skip: {} not present", path.display());
            return;
        };
        let lines: Vec<String> = text.lines().map(String::from).collect();
        let yt = YTheme::default();
        let h = Highlighter::new(Some(&PathBuf::from("test.md")), &yt);
        let highlighted = h.highlight_range(&lines, 0, lines.len());

        let mut drift_rows: Vec<(usize, usize, usize)> = Vec::new();
        for (row, (source, spans)) in lines.iter().zip(highlighted.iter()).enumerate() {
            let source_w = UnicodeWidthStr::width(source.as_str());
            let spans_w: usize = spans
                .iter()
                .map(|(_, t)| UnicodeWidthStr::width(t.as_str()))
                .sum();
            if source_w != spans_w {
                drift_rows.push((row, source_w, spans_w));
                eprintln!("DRIFT row {row}: source_width={source_w} spans_width={spans_w}");
                eprintln!("  source: {:?}", source);
                for (i, (_, t)) in spans.iter().enumerate() {
                    eprintln!(
                        "  span[{i}] w={} text={:?}",
                        UnicodeWidthStr::width(t.as_str()),
                        t
                    );
                }
            }
        }
        eprintln!(
            "{} drift rows out of {} highlighted",
            drift_rows.len(),
            highlighted.len()
        );
    }

    #[test]
    fn highlight_range_emits_only_requested_window() {
        let yt = YTheme::default();
        let h = Highlighter::new(Some(&PathBuf::from("foo.rs")), &yt);
        let lines: Vec<String> = (0..10).map(|i| format!("// line {i}")).collect();
        let out = h.highlight_range(&lines, 3, 7);
        assert_eq!(out.len(), 4);
    }
}
