use ratatui::prelude::*;
use ratatui::widgets::*;
use unicode_width::UnicodeWidthChar;

use crate::app::{App, Panel, PanelSide, RunDialog};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(frame.area());

    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    draw_panel(frame, &app.left, panels[0], app.active == PanelSide::Left);
    draw_panel(frame, &app.right, panels[1], app.active == PanelSide::Right);
    draw_footer(frame, app, chunks[1]);

    if let Some(ref dlg) = app.run_dialog {
        draw_run_dialog(frame, dlg);
    }
}

fn draw_panel(frame: &mut Frame, panel: &Panel, area: Rect, active: bool) {
    let border_style = if active {
        Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))
    } else {
        Style::default().fg(Color::Rgb(0x1e, 0x2a, 0x38))
    };

    let cwd_display = panel.cwd.display().to_string();
    let title_text = panel_title(&cwd_display, (area.width as usize).saturating_sub(4));

    let block = Block::default()
        .title(title_text)
        .title_style(if active {
            Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)).bold()
        } else {
            Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a))
        })
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if panel.entries.is_empty() {
        let empty =
            Paragraph::new("(empty)").style(Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)));
        frame.render_widget(empty, inner);
        return;
    }

    let w = inner.width as usize;
    let size_w = 10;
    let date_w = 16;
    let name_w = w.saturating_sub(size_w + date_w + 2); // 2 for spacing

    // Header
    if inner.height >= 2 {
        let header_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        let hdr = Line::from(vec![
            Span::styled(
                pad(name_w, "Name"),
                Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)),
            ),
            Span::styled(" ", Style::default()),
            Span::styled(
                pad(size_w, "Size"),
                Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)),
            ),
            Span::styled(" ", Style::default()),
            Span::styled(
                pad(date_w, "Modified"),
                Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)),
            ),
        ]);
        frame.render_widget(Paragraph::new(hdr), header_area);
    }

    let list_y = inner.y + 1;
    let list_h = inner.height.saturating_sub(1) as usize;
    let list_area = Rect {
        x: inner.x,
        y: list_y,
        width: inner.width,
        height: list_h as u16,
    };

    let scroll = if panel.selected >= list_h {
        panel.selected - list_h + 1
    } else {
        0
    };

    let items: Vec<ListItem> = panel
        .entries
        .iter()
        .skip(scroll)
        .take(list_h)
        .enumerate()
        .map(|(i, entry)| {
            let idx = i + scroll;
            let is_selected = idx == panel.selected;

            // Fixed-width ASCII prefix for dirs
            let prefix = if entry.is_dir { "[D] " } else { "    " };
            let prefix_w = 4;
            let avail_name = name_w.saturating_sub(prefix_w);
            let name_str = trunc(&entry.name, avail_name);
            let name_col = format!("{}{}", prefix, pad(avail_name, &name_str));
            let size_col = pad(size_w, &entry.size_display());
            let date_col = entry
                .modified
                .map(|d| d.format("%y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| " ".repeat(date_w));

            let style = if is_selected && active {
                Style::default()
                    .bg(Color::Rgb(0x1a, 0x22, 0x30))
                    .fg(Color::Rgb(0x7f, 0xdb, 0xca))
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default()
                    .bg(Color::Rgb(0x1a, 0x22, 0x30))
                    .fg(Color::Rgb(0xd6, 0xde, 0xeb))
            } else if entry.is_dir {
                Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))
            } else {
                Style::default().fg(Color::Rgb(0xd6, 0xde, 0xeb))
            };

            let line = Line::from(format!("{} {} {}", name_col, size_col, date_col));
            ListItem::new(line).style(style)
        })
        .collect();

    frame.render_widget(List::new(items), list_area);

    // Scroll info
    if panel.entries.len() > list_h {
        let pct = panel.selected * 100 / panel.entries.len().max(1);
        let info = format!("{}/{} {}%", panel.selected + 1, panel.entries.len(), pct);
        let info_w = info.len() as u16;
        if area.width > info_w + 2 {
            let info_area = Rect {
                x: area.x + area.width - info_w - 2,
                y: area.y + area.height - 1,
                width: info_w + 1,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(info).style(Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a))),
                info_area,
            );
        }
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let status = app.status_msg.as_deref().unwrap_or("");
    let text = Line::from(vec![
        Span::styled("q", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Quit  "),
        Span::styled("Enter", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Open  "),
        Span::styled("BS", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Parent  "),
        Span::styled(".", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Hidden  "),
        Span::styled("c/m/p/d", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Copy/Move/Paste/Del  "),
        Span::styled("Tab", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Switch  "),
        Span::styled(status, Style::default().fg(Color::Rgb(0xe5, 0xc0, 0x7b))),
    ]);
    // Right-aligned ymux release version. See ymon::draw_footer for the
    // same split pattern across the y* tool family.
    let version = format!(" v{} ", yversion::VERSION);
    let v_width = version.chars().count() as u16;
    let parts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(v_width)])
        .split(area);
    frame.render_widget(Paragraph::new(text), parts[0]);
    frame.render_widget(
        Paragraph::new(version)
            .style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)))
            .alignment(Alignment::Right),
        parts[1],
    );
}

/// Display width of `s` in terminal cells.
///
/// Every helper below counts cells, not `char`s and not bytes. A Hangul
/// syllable is one `char` but occupies two cells, so a char-counted column
/// is twice as wide as its header and every column to its right shifts —
/// which is what a Korean directory listing looked like before.
pub fn width_of(s: &str) -> usize {
    s.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// The longest prefix of `s` that fits in `max` cells. Never splits a
/// character, and never leaves half of a double-width one behind.
pub fn clip(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// The longest *suffix* of `s` that fits in `max` cells.
fn clip_tail(s: &str, max: usize) -> String {
    let mut take = 0usize;
    let mut w = 0usize;
    for c in s.chars().rev() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > max {
            break;
        }
        take += 1;
        w += cw;
    }
    let skip = s.chars().count() - take;
    s.chars().skip(skip).collect()
}

/// Pad or clip `s` to exactly `width` cells.
pub fn pad(width: usize, s: &str) -> String {
    let mut out = clip(s, width);
    for _ in 0..width - width_of(&out) {
        out.push(' ');
    }
    out
}

/// Clip to `max` cells, marking a cut with `~`.
pub fn trunc(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if width_of(s) <= max {
        return s.to_string();
    }
    format!("{}~", clip(s, max - 1))
}

/// A panel's title bar: the cwd, keeping the tail when it does not fit.
///
/// `max` is the budget for the text itself; the returned string adds the
/// two padding spaces the border draws around it.
///
/// This used to slice the `String` by byte offset, which panics outright on
/// any non-ASCII path — exactly the narrow-dock, Hangul-path case.
pub fn panel_title(cwd: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if width_of(cwd) <= max {
        return format!(" {} ", cwd);
    }
    if max <= 3 {
        return format!(" {} ", ".".repeat(max));
    }
    format!(" ...{} ", clip_tail(cwd, max - 3))
}

fn draw_run_dialog(frame: &mut Frame, dlg: &RunDialog) {
    let area = frame.area();

    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(0x00, 0x00, 0x00))),
        area,
    );

    let dlg_w = 60u16.min(area.width.saturating_sub(4));
    let dlg_h = 8u16;
    let dlg_x = (area.width.saturating_sub(dlg_w)) / 2;
    let dlg_y = (area.height.saturating_sub(dlg_h)) / 2;
    let dlg_area = Rect {
        x: dlg_x,
        y: dlg_y,
        width: dlg_w,
        height: dlg_h,
    };

    let title = format!(" Run: {} ", dlg.file_name);
    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)))
        .style(Style::default().bg(Color::Rgb(0x11, 0x18, 0x20)));

    let inner = block.inner(dlg_area);
    frame.render_widget(block, dlg_area);

    // File path
    let path_line = Paragraph::new(format!("File: {}", dlg.file_path.display()))
        .style(Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)));
    frame.render_widget(
        path_line,
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );

    // Args label
    let args_label = Paragraph::new("Arguments (optional):")
        .style(Style::default().fg(Color::Rgb(0xd6, 0xde, 0xeb)));
    frame.render_widget(
        args_label,
        Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: 1,
        },
    );

    // Args input field
    let input_w = inner.width.saturating_sub(2);
    let input_text = if dlg.args_input.is_empty() {
        String::new()
    } else {
        dlg.args_input.clone()
    };
    let input = Paragraph::new(format!("> {}", input_text)).style(
        Style::default()
            .fg(Color::Rgb(0x7f, 0xdb, 0xca))
            .bg(Color::Rgb(0x0b, 0x0f, 0x14)),
    );
    frame.render_widget(
        input,
        Rect {
            x: inner.x,
            y: inner.y + 3,
            width: input_w,
            height: 1,
        },
    );

    // Hint
    let hint = Paragraph::new("Enter: Run  |  Esc: Cancel")
        .style(Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)))
        .alignment(Alignment::Center);
    frame.render_widget(
        hint,
        Rect {
            x: inner.x,
            y: inner.y + 5,
            width: inner.width,
            height: 1,
        },
    );

    // Cursor position
    let cursor_x = inner.x + 2 + dlg.args_input.chars().count() as u16;
    let cursor_y = inner.y + 3;
    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two cells each.
    const HANGUL: &str = "문서";

    #[test]
    fn width_counts_cells_not_chars() {
        assert_eq!(width_of("abc"), 3);
        assert_eq!(width_of(HANGUL), 4);
        assert_eq!(width_of("a문b"), 4);
    }

    #[test]
    fn clip_never_splits_a_wide_character() {
        // 3 cells cannot hold one and a half syllables: it holds one.
        assert_eq!(clip(HANGUL, 3), "문");
        assert_eq!(clip(HANGUL, 4), "문서");
        assert_eq!(clip(HANGUL, 1), "");
        assert_eq!(clip("abcd", 2), "ab");
    }

    #[test]
    fn pad_fills_to_exactly_the_cell_width() {
        assert_eq!(width_of(&pad(10, HANGUL)), 10);
        assert_eq!(pad(6, HANGUL), "문서  ");
        // An odd budget leaves one trailing space rather than half a glyph.
        assert_eq!(pad(5, HANGUL), "문서 ");
        assert_eq!(width_of(&pad(3, "abcdef")), 3);
    }

    #[test]
    fn trunc_marks_the_cut_and_stays_within_budget() {
        assert_eq!(trunc("abcdef", 4), "abc~");
        assert_eq!(trunc("abc", 4), "abc");
        assert_eq!(trunc(HANGUL, 4), HANGUL);
        let cut = trunc("문서파일", 5);
        assert_eq!(cut, "문서~");
        assert!(width_of(&cut) <= 5);
        assert_eq!(trunc("anything", 0), "");
    }

    /// Byte-slicing this used to panic; it is the dock's normal case.
    #[test]
    fn panel_title_tail_truncates_a_hangul_path_without_panicking() {
        let cwd = r"D:\Git\ymux\프로젝트\문서";
        for max in 0..=40usize {
            let title = panel_title(cwd, max);
            assert!(
                width_of(&title) <= max + 2,
                "max {max} produced {title:?}, too wide"
            );
        }
        assert_eq!(panel_title(cwd, 10), r" ...트\문서 ");
    }

    #[test]
    fn panel_title_keeps_a_path_that_fits() {
        assert_eq!(panel_title("/work", 20), " /work ");
        assert_eq!(panel_title("/work", 5), " /work ");
    }
}
