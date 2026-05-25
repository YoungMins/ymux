//! Markdown → ratatui `Line` renderer used by the preview mode.
//!
//! Parses the source with `pulldown-cmark` and walks the event stream into
//! styled `Line<'static>` values that match the editor's general palette
//! (headings/links in the teal accent, body text in the editor foreground,
//! code blocks on a panel background).

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const HEADING: Color = Color::Rgb(0x7f, 0xdb, 0xca);
const TEXT: Color = Color::Rgb(0xd6, 0xde, 0xeb);
const CODE_FG: Color = Color::Rgb(0xff, 0xcb, 0x6b);
const CODE_BG: Color = Color::Rgb(0x1a, 0x22, 0x30);
const LINK: Color = Color::Rgb(0x82, 0xaa, 0xff);
const QUOTE: Color = Color::Rgb(0x6a, 0x7a, 0x8a);
const RULE: Color = Color::Rgb(0x3a, 0x4a, 0x5a);
const BULLET: Color = Color::Rgb(0x7f, 0xdb, 0xca);

/// Convert markdown `source` into a series of pre-styled `Line`s ready to
/// hand to a ratatui `Paragraph`. The output owns its strings (`'static`),
/// so it can outlive the input.
pub fn render(source: &str) -> Vec<Line<'static>> {
    let parser = Parser::new(source);
    let mut renderer = Renderer::default();
    for event in parser {
        renderer.handle(event);
    }
    renderer.finish()
}

#[derive(Default)]
struct Renderer {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    /// Stack of block-level prefixes (e.g. blockquote bars) re-emitted on
    /// every wrapped line so visual nesting survives a `flush_line`.
    indent: String,
    heading: Option<HeadingLevel>,
    code_block: bool,
    in_link: bool,
    link_url: String,
    bold: u32,
    italic: u32,
    strike: u32,
    /// Per-active-list state: `None` = unordered, `Some(n)` = next ordinal.
    list_stack: Vec<Option<u64>>,
}

impl Renderer {
    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.push_text(text.into_string()),
            Event::Code(code) => {
                self.current.push(Span::styled(
                    code.into_string(),
                    Style::default().fg(CODE_FG).bg(CODE_BG),
                ));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                self.push_text(html.into_string());
            }
            Event::SoftBreak => self.current.push(Span::raw(" ")),
            Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.flush_line();
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(40),
                    Style::default().fg(RULE),
                )));
                self.blank_line();
            }
            Event::TaskListMarker(checked) => {
                let mark = if checked { "[x] " } else { "[ ] " };
                self.current
                    .push(Span::styled(mark.to_string(), Style::default().fg(BULLET)));
            }
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.heading = Some(level);
                let prefix = match level {
                    HeadingLevel::H1 => "# ",
                    HeadingLevel::H2 => "## ",
                    HeadingLevel::H3 => "### ",
                    HeadingLevel::H4 => "#### ",
                    HeadingLevel::H5 => "##### ",
                    HeadingLevel::H6 => "###### ",
                };
                self.current.push(Span::styled(
                    prefix.to_string(),
                    Style::default().fg(HEADING).add_modifier(Modifier::BOLD),
                ));
            }
            Tag::Paragraph => {}
            Tag::BlockQuote(_) => self.indent.push_str("│ "),
            Tag::CodeBlock(kind) => {
                self.code_block = true;
                let fence = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        format!("```{}", lang)
                    }
                    _ => "```".to_string(),
                };
                self.flush_line();
                self.lines
                    .push(Line::from(Span::styled(fence, Style::default().fg(RULE))));
            }
            Tag::List(start) => self.list_stack.push(start),
            Tag::Item => {
                let depth = self.list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = if let Some(Some(n)) = self.list_stack.last_mut() {
                    let m = format!("{}. ", n);
                    *n += 1;
                    m
                } else {
                    "• ".to_string()
                };
                self.current.push(Span::raw(indent));
                self.current
                    .push(Span::styled(marker, Style::default().fg(BULLET)));
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                self.in_link = true;
                self.link_url = dest_url.into_string();
            }
            Tag::Image { dest_url, .. } => {
                self.current.push(Span::styled(
                    format!("[image: {}]", dest_url),
                    Style::default().fg(LINK),
                ));
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.heading = None;
                self.flush_line();
                self.blank_line();
            }
            TagEnd::Paragraph => {
                self.flush_line();
                self.blank_line();
            }
            TagEnd::BlockQuote(_) => {
                let drop_by = "│ ".chars().map(|c| c.len_utf8()).sum::<usize>();
                let new_len = self.indent.len().saturating_sub(drop_by);
                self.indent.truncate(new_len);
            }
            TagEnd::CodeBlock => {
                self.code_block = false;
                self.flush_line();
                self.lines.push(Line::from(Span::styled(
                    "```".to_string(),
                    Style::default().fg(RULE),
                )));
                self.blank_line();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                if self.list_stack.is_empty() {
                    self.blank_line();
                }
            }
            TagEnd::Item => self.flush_line(),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if !self.link_url.is_empty() {
                    self.current.push(Span::styled(
                        format!(" ({})", self.link_url),
                        Style::default().fg(RULE),
                    ));
                }
                self.in_link = false;
                self.link_url.clear();
            }
            _ => {}
        }
    }

    fn push_text(&mut self, text: String) {
        if self.code_block {
            // Code-block text can carry embedded `\n`. Split so each visual
            // line gets its own `Line` entry — otherwise the fence renders
            // as one ratatui line that wraps unpredictably.
            let mut parts = text.split('\n').peekable();
            while let Some(part) = parts.next() {
                if !part.is_empty() {
                    self.current.push(Span::styled(
                        part.to_string(),
                        Style::default().fg(CODE_FG).bg(CODE_BG),
                    ));
                }
                if parts.peek().is_some() {
                    self.flush_line();
                }
            }
            return;
        }
        let mut style = if self.heading.is_some() {
            Style::default().fg(HEADING).add_modifier(Modifier::BOLD)
        } else if self.in_link {
            Style::default().fg(LINK).add_modifier(Modifier::UNDERLINED)
        } else {
            Style::default().fg(TEXT)
        };
        if self.bold > 0 {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        self.current.push(Span::styled(text, style));
    }

    fn flush_line(&mut self) {
        if self.current.is_empty() && self.indent.is_empty() {
            // Avoid stacking blank lines from sibling block ends.
            return;
        }
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(self.current.len() + 1);
        if !self.indent.is_empty() {
            spans.push(Span::styled(
                self.indent.clone(),
                Style::default().fg(QUOTE),
            ));
        }
        spans.append(&mut self.current);
        self.lines.push(Line::from(spans));
    }

    /// Push at most one trailing blank line — back-to-back block-end events
    /// (e.g. heading-then-paragraph) would otherwise collapse into a wall of
    /// gaps.
    fn blank_line(&mut self) {
        if matches!(self.lines.last(), Some(l) if l.spans.is_empty()) {
            return;
        }
        self.lines.push(Line::default());
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        if !self.current.is_empty() {
            self.flush_line();
        }
        // Trim any trailing blank line so the preview doesn't end on dead
        // space.
        while matches!(self.lines.last(), Some(l) if l.spans.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_heading() {
        let lines = render("# Hello\n");
        let text = plain(&lines);
        assert!(text.contains("# Hello"));
    }

    #[test]
    fn renders_paragraph_with_bold_and_italic() {
        let lines = render("**bold** and *italic*\n");
        let text = plain(&lines);
        assert!(text.contains("bold"));
        assert!(text.contains("italic"));
    }

    #[test]
    fn renders_unordered_list() {
        let lines = render("- one\n- two\n");
        let text = plain(&lines);
        assert!(text.contains("• one"));
        assert!(text.contains("• two"));
    }

    #[test]
    fn renders_ordered_list() {
        let lines = render("1. one\n2. two\n");
        let text = plain(&lines);
        assert!(text.contains("1. one"));
        assert!(text.contains("2. two"));
    }

    #[test]
    fn renders_code_block_keeps_lines() {
        let lines = render("```rust\nfn foo() {}\nfn bar() {}\n```\n");
        let text = plain(&lines);
        assert!(text.contains("```rust"));
        assert!(text.contains("fn foo()"));
        assert!(text.contains("fn bar()"));
        assert!(text.ends_with("```"));
    }

    #[test]
    fn renders_inline_code() {
        let lines = render("Use `cargo build` to compile.\n");
        let text = plain(&lines);
        assert!(text.contains("cargo build"));
    }

    #[test]
    fn renders_link_with_url() {
        let lines = render("[click](https://example.com)\n");
        let text = plain(&lines);
        assert!(text.contains("click"));
        assert!(text.contains("https://example.com"));
    }

    #[test]
    fn renders_blockquote_prefix() {
        let lines = render("> quoted\n");
        let text = plain(&lines);
        assert!(text.contains("│ "));
        assert!(text.contains("quoted"));
    }

    #[test]
    fn empty_input_returns_empty() {
        let lines = render("");
        assert!(lines.is_empty());
    }

    #[test]
    fn horizontal_rule_renders() {
        let lines = render("before\n\n---\n\nafter\n");
        let text = plain(&lines);
        assert!(text.contains("before"));
        assert!(text.contains("after"));
        assert!(text.contains("─"));
    }
}
