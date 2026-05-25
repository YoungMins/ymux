use ratatui::prelude::*;
use ratatui::widgets::*;
use syntect::highlighting::{FontStyle, Style as SyntectStyle};
use unicode_width::UnicodeWidthChar;

use crate::app::{App, ExitChoice, SwitchChoice};
use crate::markdown;

const SIDEBAR_WIDTH: u16 = 28;

/// Map a syntect `Style` (colors + bold/italic/underline flags) onto a
/// ratatui `Style`. Foreground is converted via `Color::Rgb`; background is
/// dropped (the editor's own background stays in charge).
fn syntect_to_ratatui(s: SyntectStyle) -> Style {
    let mut style = Style::default().fg(Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b));
    if s.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if s.font_style.contains(FontStyle::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Min(0),    // editor area
            Constraint::Length(1), // status line
            Constraint::Length(1), // command / message line
        ])
        .split(frame.area());

    let editor_height = chunks[1].height as usize;

    draw_title_bar(frame, app, chunks[0]);
    draw_editor(frame, app, chunks[1], editor_height);
    draw_status_line(frame, app, chunks[2]);
    draw_command_line(frame, app, chunks[3]);

    if app.exit_dialog {
        draw_exit_dialog(frame, app);
    }
    if app.switch_prompt.is_some() {
        draw_switch_prompt(frame, app);
    }
}

fn draw_title_bar(frame: &mut Frame, app: &App, area: Rect) {
    let suffix = if app.preview_mode { " [PREVIEW]" } else { "" };
    let title = format!(" ycode — {}{} ", app.title(), suffix);
    let bar = Paragraph::new(title).style(
        Style::default()
            .bg(Color::Rgb(0x11, 0x18, 0x20))
            .fg(Color::Rgb(0x7f, 0xdb, 0xca)),
    );
    frame.render_widget(bar, area);
}

fn draw_editor(frame: &mut Frame, app: &App, area: Rect, _viewport_height: usize) {
    // Optional sidebar column on the left. When closed it consumes zero
    // width so the editor occupies the full area.
    let sidebar_width = if app.sidebar_open { SIDEBAR_WIDTH } else { 0 };
    let outer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(sidebar_width), Constraint::Min(0)])
        .split(area);
    if app.sidebar_open {
        draw_sidebar(frame, app, outer[0]);
    }
    let editor_area = outer[1];

    if app.preview_mode {
        draw_markdown_preview(frame, app, editor_area);
        return;
    }

    let line_num_width: u16 = format!("{}", app.buffer.line_count()).len() as u16 + 1;

    let editor_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(line_num_width + 1), Constraint::Min(0)])
        .split(editor_area);

    let visible_lines = area.height as usize;
    let line_numbers: Vec<Line> = (app.scroll_row..app.scroll_row + visible_lines)
        .map(|row| {
            if row < app.buffer.line_count() {
                let style = if row == app.cursor_row {
                    Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))
                } else {
                    Style::default().fg(Color::Rgb(0x3a, 0x4a, 0x5a))
                };
                Line::from(Span::styled(
                    format!("{:>width$} ", row + 1, width = line_num_width as usize),
                    style,
                ))
            } else {
                Line::from(Span::styled(
                    format!("{:>width$} ", "~", width = line_num_width as usize),
                    Style::default().fg(Color::Rgb(0x3a, 0x4a, 0x5a)),
                ))
            }
        })
        .collect();

    let nums =
        Paragraph::new(line_numbers).style(Style::default().bg(Color::Rgb(0x0b, 0x0f, 0x14)));
    frame.render_widget(nums, editor_chunks[0]);

    let start = app.scroll_row;
    let end = (app.scroll_row + visible_lines).min(app.buffer.line_count());
    let highlighted = app
        .highlighter
        .highlight_range(&app.buffer.lines, start, end);

    let mut code_lines: Vec<Line> = Vec::with_capacity(visible_lines);
    for line_spans in &highlighted {
        let spans: Vec<Span> = line_spans
            .iter()
            .map(|(st, text)| Span::styled(text.clone(), syntect_to_ratatui(*st)))
            .collect();
        code_lines.push(Line::from(spans));
    }
    // Pad past EOF so the editor area renders with the right background.
    for _ in highlighted.len()..visible_lines {
        code_lines.push(Line::default());
    }

    let code = Paragraph::new(code_lines).style(Style::default().fg(Color::Rgb(0xd6, 0xde, 0xeb)));
    frame.render_widget(code, editor_chunks[1]);

    // No editor cursor when the sidebar holds focus — the cursor moves to
    // the sidebar instead so it's clear where input is going.
    if !app.sidebar_open {
        let cursor_screen_row = (app.cursor_row - app.scroll_row) as u16;
        // `cursor_col` counts characters; the terminal positions the cursor
        // by display cell. CJK chars are 2 cells wide, so without this
        // conversion the cursor drifts left on lines that mix Korean with
        // ASCII.
        let visual_col = visual_column(app.buffer.line(app.cursor_row), app.cursor_col);
        let cursor_screen_col = visual_col.saturating_sub(app.scroll_col) as u16;
        let cursor_x = editor_chunks[1].x + cursor_screen_col;
        let cursor_y = editor_chunks[1].y + cursor_screen_row;
        if cursor_x < editor_chunks[1].x + editor_chunks[1].width
            && cursor_y < editor_chunks[1].y + editor_chunks[1].height
        {
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        }
    }
}

/// Sum the display widths of the first `char_col` chars in `line`. Used
/// to translate a character-based cursor column into the cell-based column
/// the terminal expects. Combining marks / zero-width chars contribute 0.
fn visual_column(line: &str, char_col: usize) -> usize {
    line.chars()
        .take(char_col)
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// Render the markdown preview into the editor area. A single-character
/// left gutter keeps the rendered text from butting against the sidebar
/// border, mirroring the gutter the source view uses for line numbers.
fn draw_markdown_preview(frame: &mut Frame, app: &App, area: Rect) {
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(area);

    let gutter = Paragraph::new("").style(Style::default().bg(Color::Rgb(0x0b, 0x0f, 0x14)));
    frame.render_widget(gutter, layout[0]);

    let body_area = layout[1];
    let visible = body_area.height as usize;

    let all = markdown::render(&app.buffer.content());
    let start = app.preview_scroll.min(all.len());
    let end = (start + visible).min(all.len());
    let mut slice: Vec<Line> = all[start..end].to_vec();
    while slice.len() < visible {
        slice.push(Line::default());
    }

    let para = Paragraph::new(slice)
        .style(Style::default().fg(Color::Rgb(0xd6, 0xde, 0xeb)))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, body_area);
}

fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    // 1 row for the root header, the rest for entries.
    let viewport = area.height.saturating_sub(1) as usize;
    let scroll = app.sidebar.scroll;
    let selected = app.sidebar.selected;

    let mut lines: Vec<Line> = Vec::with_capacity(viewport + 1);
    let root_label = app
        .sidebar
        .root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| app.sidebar.root.display().to_string());
    lines.push(Line::from(Span::styled(
        format!(" {}", root_label),
        Style::default()
            .fg(Color::Rgb(0x7f, 0xdb, 0xca))
            .add_modifier(Modifier::BOLD),
    )));

    let row_width = area.width as usize;
    for (offset, entry) in app
        .sidebar
        .entries
        .iter()
        .skip(scroll)
        .take(viewport)
        .enumerate()
    {
        let abs_i = scroll + offset;
        let indent = "  ".repeat(entry.depth);
        let icon = if entry.is_dir {
            if entry.expanded {
                "▾ "
            } else {
                "▸ "
            }
        } else {
            "  "
        };
        // Pad to full row width so the selection highlight covers the
        // entire row, not just up to the end of the file name.
        let text = format!(" {}{}{}", indent, icon, entry.name);
        let padded = format!("{:<width$}", text, width = row_width);

        let style = if abs_i == selected {
            // High-contrast accent-on-dark — easy to find with the eye.
            Style::default()
                .bg(Color::Rgb(0x7f, 0xdb, 0xca))
                .fg(Color::Rgb(0x0b, 0x0f, 0x14))
                .add_modifier(Modifier::BOLD)
        } else if entry.is_dir {
            Style::default().fg(Color::Rgb(0x82, 0xaa, 0xff))
        } else {
            Style::default().fg(Color::Rgb(0xd6, 0xde, 0xeb))
        };
        lines.push(Line::from(Span::styled(padded, style)));
    }

    let para = Paragraph::new(lines).style(Style::default().bg(Color::Rgb(0x0b, 0x0f, 0x14)));
    frame.render_widget(para, area);
}

fn draw_switch_prompt(frame: &mut Frame, app: &App) {
    let Some(prompt) = app.switch_prompt.as_ref() else {
        return;
    };
    let area = frame.area();

    let dialog_width = 60u16.min(area.width.saturating_sub(4));
    let dialog_height = 7u16;
    let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
    let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect {
        x: dialog_x,
        y: dialog_y,
        width: dialog_width,
        height: dialog_height,
    };

    let block = Block::default()
        .title(" Unsaved changes ")
        .title_style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)))
        .style(Style::default().bg(Color::Rgb(0x11, 0x18, 0x20)));
    let inner = block.inner(dialog_area);
    frame.render_widget(Clear, dialog_area);
    frame.render_widget(block, dialog_area);

    let msg = format!("Open `{}`?", prompt.target.display());
    let para = Paragraph::new(msg)
        .style(Style::default().fg(Color::Rgb(0xd6, 0xde, 0xeb)))
        .alignment(Alignment::Center);
    let msg_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 2,
    };
    frame.render_widget(para, msg_area);

    let btn_y = inner.y + 3;
    let choices = SwitchChoice::ALL;
    let total_btn_width: u16 = choices.iter().map(|c| c.label().len() as u16 + 4).sum();
    let spacing = inner.width.saturating_sub(total_btn_width) / (choices.len() as u16 + 1);
    let mut x = inner.x + spacing;

    for (i, choice) in choices.iter().enumerate() {
        let label = choice.label();
        let w = label.len() as u16 + 4;
        let is_selected = i == prompt.choice;
        let style = if is_selected {
            Style::default()
                .bg(Color::Rgb(0x7f, 0xdb, 0xca))
                .fg(Color::Rgb(0x0b, 0x0f, 0x14))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Rgb(0xd6, 0xde, 0xeb))
                .bg(Color::Rgb(0x1a, 0x22, 0x30))
        };
        let btn_area = Rect {
            x,
            y: btn_y,
            width: w,
            height: 1,
        };
        let btn = Paragraph::new(format!(" {} ", label))
            .style(style)
            .alignment(Alignment::Center);
        frame.render_widget(btn, btn_area);
        x += w + spacing;
    }
}

fn draw_status_line(frame: &mut Frame, app: &App, area: Rect) {
    let dirty_marker = if app.buffer.dirty { " [+]" } else { "" };
    let file_info = app
        .file_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "[untitled]".to_string());

    let left = format!(" {}{}", file_info, dirty_marker);
    let right = format!(
        "{}  Ln {}, Col {}  ({} lines)  v{} ",
        app.highlighter.language_name(),
        app.cursor_row + 1,
        app.cursor_col + 1,
        app.buffer.line_count(),
        yversion::VERSION
    );

    let padding = (area.width as usize)
        .saturating_sub(left.len())
        .saturating_sub(right.len());
    let text = format!("{}{:pad$}{}", left, "", right, pad = padding);

    let bar = Paragraph::new(text).style(
        Style::default()
            .bg(Color::Rgb(0x1a, 0x22, 0x30))
            .fg(Color::Rgb(0xd6, 0xde, 0xeb)),
    );
    frame.render_widget(bar, area);
}

fn draw_command_line(frame: &mut Frame, app: &App, area: Rect) {
    let text = if app.command_mode {
        format!(":{}", app.command_input)
    } else if let Some(ref msg) = app.status_msg {
        msg.clone()
    } else if app.preview_mode {
        " Esc/Alt+M Exit Preview | ↑↓ PgUp/PgDn Scroll | Ctrl+S Save | Ctrl+Q Quit".to_string()
    } else {
        let mut hint =
            String::from(" Esc Menu | Ctrl+S Save | Ctrl+Q Quit | Ctrl+B Files | Ctrl+F Find");
        if app.is_markdown() {
            hint.push_str(" | Alt+M Preview");
        }
        hint
    };

    let style = if app.command_mode {
        Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))
    } else {
        Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a))
    };

    let bar = Paragraph::new(text).style(style);
    frame.render_widget(bar, area);
}

fn draw_exit_dialog(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Dim background
    let overlay = Block::default().style(Style::default().bg(Color::Rgb(0x00, 0x00, 0x00)));
    let overlay_area = Rect {
        x: 0,
        y: 0,
        width: area.width,
        height: area.height,
    };
    frame.render_widget(Clear, overlay_area);
    frame.render_widget(overlay, overlay_area);

    // Dialog box
    let dialog_width = 50u16.min(area.width.saturating_sub(4));
    let dialog_height = 7u16;
    let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
    let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;

    let dialog_area = Rect {
        x: dialog_x,
        y: dialog_y,
        width: dialog_width,
        height: dialog_height,
    };

    let dirty_note = if app.buffer.dirty {
        "File has unsaved changes."
    } else {
        "No unsaved changes."
    };

    let block = Block::default()
        .title(" Exit ")
        .title_style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)))
        .style(Style::default().bg(Color::Rgb(0x11, 0x18, 0x20)));

    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    // Message
    let msg = Paragraph::new(dirty_note)
        .style(Style::default().fg(Color::Rgb(0xd6, 0xde, 0xeb)))
        .alignment(Alignment::Center);
    let msg_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 2,
    };
    frame.render_widget(msg, msg_area);

    // Buttons
    let btn_y = inner.y + 3;
    let choices = ExitChoice::ALL;
    let total_btn_width: u16 = choices.iter().map(|c| c.label().len() as u16 + 4).sum();
    let spacing = inner.width.saturating_sub(total_btn_width) / (choices.len() as u16 + 1);
    let mut x = inner.x + spacing;

    for (i, choice) in choices.iter().enumerate() {
        let label = choice.label();
        let w = label.len() as u16 + 4;
        let is_selected = i == app.exit_choice;

        let style = if is_selected {
            Style::default()
                .bg(Color::Rgb(0x7f, 0xdb, 0xca))
                .fg(Color::Rgb(0x0b, 0x0f, 0x14))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Rgb(0xd6, 0xde, 0xeb))
                .bg(Color::Rgb(0x1a, 0x22, 0x30))
        };

        let btn_area = Rect {
            x,
            y: btn_y,
            width: w,
            height: 1,
        };
        let btn = Paragraph::new(format!(" {} ", label))
            .style(style)
            .alignment(Alignment::Center);
        frame.render_widget(btn, btn_area);

        x += w + spacing;
    }
}
