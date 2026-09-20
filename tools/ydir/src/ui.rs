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

    draw_panel(
        frame,
        &app.left,
        panels[0],
        app.active == PanelSide::Left,
        Columns::legacy,
    );
    draw_panel(
        frame,
        &app.right,
        panels[1],
        app.active == PanelSide::Right,
        Columns::legacy,
    );
    draw_footer(frame, app, chunks[1]);

    if let Some(ref dlg) = app.run_dialog {
        draw_run_dialog(frame, dlg);
    }
}

/// Width of the `[D] ` / `    ` prefix every name carries.
const PREFIX_W: usize = 4;
const SIZE_W: usize = 10;
const DATE_W: usize = 16;
/// A name column shorter than this tells two files apart about as well as
/// no name column at all, so the column to its right is dropped instead.
const MIN_NAME_W: usize = 8;

/// The cell width of each column of a listing. A `size`/`date` of 0 means
/// that column is dropped entirely — no header, no separator space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columns {
    /// Includes [`PREFIX_W`].
    pub name: usize,
    pub size: usize,
    pub date: usize,
}

impl Columns {
    /// The two-panel layout: Size and Modified are always drawn and Name
    /// takes what is left. Kept exactly as it was, so a yDir opened in a
    /// pane renders as it always has.
    pub fn legacy(width: usize) -> Self {
        Self {
            name: width.saturating_sub(SIZE_W + DATE_W + 2),
            size: SIZE_W,
            date: DATE_W,
        }
    }

    /// Dock mode: drop the rightmost column that would squeeze the name
    /// below [`MIN_NAME_W`]. In a ~250 px dock the fixed layout left four
    /// cells for the name, which is the whole complaint.
    pub fn adaptive(width: usize) -> Self {
        if width >= PREFIX_W + MIN_NAME_W + 1 + SIZE_W + 1 + DATE_W {
            Self::legacy(width)
        } else if width >= PREFIX_W + MIN_NAME_W + 1 + SIZE_W {
            Self {
                name: width - SIZE_W - 1,
                size: SIZE_W,
                date: 0,
            }
        } else {
            Self {
                name: width,
                size: 0,
                date: 0,
            }
        }
    }
}

/// One listing line: the name column, then whichever of Size/Modified
/// survived the width.
fn columns_line(cols: &Columns, name: &str, size: &str, date: &str) -> String {
    let mut out = pad(cols.name, &trunc(name, cols.name));
    if cols.size > 0 {
        out.push(' ');
        out.push_str(&pad(cols.size, size));
    }
    if cols.date > 0 {
        out.push(' ');
        out.push_str(&pad(cols.date, date));
    }
    out
}

fn draw_panel(
    frame: &mut Frame,
    panel: &Panel,
    area: Rect,
    active: bool,
    columns: fn(usize) -> Columns,
) {
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

    let cols = columns(inner.width as usize);

    // Header
    if inner.height >= 2 {
        let header_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        let hdr = Paragraph::new(columns_line(&cols, "Name", "Size", "Modified"))
            .style(Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)));
        frame.render_widget(hdr, header_area);
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
            let name_col = format!("{}{}", prefix, entry.name);
            let date_col = entry
                .modified
                .map(|d| d.format("%y-%m-%d %H:%M").to_string())
                .unwrap_or_default();

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

            let line = Line::from(columns_line(
                &cols,
                &name_col,
                &entry.size_display(),
                &date_col,
            ));
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

    /// Name width the user actually reads, i.e. minus the `[D] ` prefix.
    fn usable_name(width: usize) -> usize {
        Columns::adaptive(width).name - PREFIX_W
    }

    #[test]
    fn legacy_columns_are_unchanged() {
        // The old inline arithmetic: width - (10 + 16 + 2).
        assert_eq!(
            Columns::legacy(60),
            Columns {
                name: 32,
                size: 10,
                date: 16
            }
        );
        assert_eq!(Columns::legacy(10).name, 0);
    }

    /// The two widths the ymux-side defaults are picked to produce: the
    /// dock's minimum (~260 px ≈ 34 cols, 32 inner) and its default
    /// (~440 px ≈ 58 cols, 56 inner).
    #[test]
    fn adaptive_columns_buy_the_name_column_back() {
        // 56 cells: everything fits, with a name you can read.
        assert_eq!(
            Columns::adaptive(56),
            Columns {
                name: 28,
                size: 10,
                date: 16
            }
        );
        assert_eq!(usable_name(56), 24);

        // 32 cells: Modified goes, Size stays.
        assert_eq!(
            Columns::adaptive(32),
            Columns {
                name: 21,
                size: 10,
                date: 0
            }
        );
        assert_eq!(usable_name(32), 17);

        // What the fixed layout gave at that width: four cells.
        assert_eq!(Columns::legacy(32).name - PREFIX_W, 0);
    }

    #[test]
    fn adaptive_columns_drop_size_last_and_never_starve_the_name() {
        assert_eq!(
            Columns::adaptive(22),
            Columns {
                name: 22,
                size: 0,
                date: 0
            }
        );
        for width in 1..80usize {
            let c = Columns::adaptive(width);
            assert!(c.name >= MIN_NAME_W + PREFIX_W || c.size == 0 && c.date == 0);
            let spacers = usize::from(c.size > 0) + usize::from(c.date > 0);
            assert_eq!(
                c.name + c.size + c.date + spacers,
                width,
                "width {width} does not add up"
            );
        }
    }

    #[test]
    fn columns_line_fills_exactly_the_panel_width() {
        for width in [22usize, 32, 56] {
            let cols = Columns::adaptive(width);
            let line = columns_line(&cols, "    보고서_최종_v2.txt", "1.2 KB", "26-09-21 10:00");
            assert_eq!(width_of(&line), width, "width {width} produced {line:?}");
        }
    }

    /// A dropped column takes its separator space with it, so nothing is
    /// left dangling at the right edge.
    #[test]
    fn columns_line_omits_dropped_columns_entirely() {
        let cols = Columns {
            name: 10,
            size: 0,
            date: 0,
        };
        assert_eq!(columns_line(&cols, "a.txt", "1 B", "x"), "a.txt     ");
    }
}
