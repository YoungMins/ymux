use ratatui::prelude::*;
use ratatui::widgets::*;
use unicode_width::UnicodeWidthChar;

use crate::app::{App, Panel, PanelSide, RunDialog};
use crate::preview::{split_dock, Preview};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(frame.area());

    if app.dock {
        draw_dock(frame, app, chunks[0]);
    } else {
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
    }
    draw_footer(frame, app, chunks[1]);

    if let Some(ref dlg) = app.run_dialog {
        draw_run_dialog(frame, dlg);
    }
}

const ACCENT: Color = Color::Rgb(0x7f, 0xdb, 0xca);
const MUTED: Color = Color::Rgb(0x6a, 0x7a, 0x8a);
const FG: Color = Color::Rgb(0xd6, 0xde, 0xeb);
const IDLE_BORDER: Color = Color::Rgb(0x1e, 0x2a, 0x38);

/// The dock: one listing filling the top, the selected entry's head below
/// it. The two-panel layout is 250 px of unreadable columns in a dock this
/// narrow, which is what this replaces (spec §2).
fn draw_dock(frame: &mut Frame, app: &App, area: Rect) {
    let (list_h, preview_h) = if app.show_preview {
        split_dock(area.height)
    } else {
        (area.height, 0)
    };

    draw_panel(
        frame,
        &app.left,
        Rect {
            height: list_h,
            ..area
        },
        true,
        Columns::adaptive,
    );

    if preview_h > 0 {
        draw_preview(
            frame,
            app,
            Rect {
                y: area.y + list_h,
                height: preview_h,
                ..area
            },
        );
    }
}

fn draw_preview(frame: &mut Frame, app: &App, area: Rect) {
    let title = app.preview_title().unwrap_or_else(|| "Preview".to_string());
    let block = Block::default()
        .title(panel_title(&title, (area.width as usize).saturating_sub(4)))
        .title_style(Style::default().fg(MUTED))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(IDLE_BORDER));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let rows = inner.height as usize;
    let width = inner.width as usize;
    let note = |s: &str| vec![Line::from(s.to_string()).style(Style::default().fg(MUTED))];
    let body: Vec<Line> = match &app.preview {
        Preview::Empty => note("(nothing selected)"),
        Preview::Binary => note("(binary file)"),
        Preview::Error(e) => note(&format!("({})", clip(e, width.saturating_sub(2)))),
        Preview::Text(lines) if lines.is_empty() => note("(empty file)"),
        Preview::Text(lines) => lines
            .iter()
            .take(rows)
            .map(|l| Line::from(clip(l, width)).style(Style::default().fg(FG)))
            .collect(),
        Preview::Dir { names, .. } if names.is_empty() => note("(empty directory)"),
        Preview::Dir { names, more } => {
            // The tail line costs a row, so reserve it before filling.
            let needs_tail = *more || names.len() > rows;
            let shown = if needs_tail {
                rows.saturating_sub(1)
            } else {
                rows
            };
            let mut out: Vec<Line> = names
                .iter()
                .take(shown)
                .map(|n| {
                    let style = if n.ends_with('/') {
                        Style::default().fg(ACCENT)
                    } else {
                        Style::default().fg(FG)
                    };
                    Line::from(clip(n, width)).style(style)
                })
                .collect();
            if needs_tail {
                let hidden = names.len().saturating_sub(out.len());
                // `more` means the walk stopped at its cap, so the count is
                // a floor, not a total.
                let label = match (*more, hidden) {
                    (true, 0) => "… more".to_string(),
                    (true, n) => format!("… {n}+ more"),
                    (false, n) => format!("… {n} more"),
                };
                out.push(Line::from(label).style(Style::default().fg(MUTED)));
            }
            out
        }
    };
    frame.render_widget(Paragraph::new(body), inner);
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

/// `(key, label)` pairs for the footer. The dock's list is short on
/// purpose: it renders into roughly 30 cells, and the full one is 70.
/// Tab appears in both, but in the dock it toggles the preview — the
/// footer is the only place that is discoverable.
fn footer_keys(dock: bool) -> &'static [(&'static str, &'static str)] {
    if dock {
        &[("Enter", " Open  "), ("BS", " Up  "), ("Tab", " Prev  ")]
    } else {
        &[
            ("q", " Quit  "),
            ("Enter", " Open  "),
            ("BS", " Parent  "),
            (".", " Hidden  "),
            ("c/m/p/d", " Copy/Move/Paste/Del  "),
            ("Tab", " Switch  "),
        ]
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let status = app.status_msg.as_deref().unwrap_or("");
    let mut spans: Vec<Span> = Vec::new();
    for (key, label) in footer_keys(app.dock) {
        spans.push(Span::styled(*key, Style::default().fg(ACCENT)));
        spans.push(Span::raw(*label));
    }
    spans.push(Span::styled(
        status,
        Style::default().fg(Color::Rgb(0xe5, 0xc0, 0x7b)),
    ));
    let text = Line::from(spans);
    // The right-aligned release version identifies a tool the user ran
    // themselves. The dock's yDir is started by ymux, which shows its own
    // version, and 9 cells is a third of a 34-cell dock footer.
    if app.dock {
        frame.render_widget(Paragraph::new(text), area);
        return;
    }
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

    /// Render the dock for real, through ratatui's test backend, and read
    /// the cells back. Closest thing to looking at it without a TTY.
    fn render_dock(width: u16, height: u16, dir: &std::path::Path) -> Vec<String> {
        let mut app = App::new(dir.to_path_buf()).unwrap().with_dock(true);
        app.sync_preview();
        let mut terminal = Terminal::new(backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                // A wide glyph lives in one cell and blanks the next, so
                // walking every cell would double every space after it.
                let mut row = String::new();
                let mut x = 0u16;
                while x < width {
                    let sym = buf[(x, y)].symbol();
                    row.push_str(sym);
                    x += width_of(sym).max(1) as u16;
                }
                row.trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn the_dock_draws_one_listing_over_a_preview_of_the_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("보고서.txt"), "첫 번째 줄\n두 번째 줄\n").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("안쪽.md"), "x").unwrap();

        // 34 cells ~= the 260 px minimum; 24 rows is a short window.
        let rows = render_dock(34, 24, dir);
        let screen = rows.join("\n");

        // One listing, not two: no vertical rule anywhere in the body.
        for (i, row) in rows.iter().enumerate().take(rows.len() - 1) {
            assert!(
                !row.trim_start_matches('│')
                    .trim_end_matches('│')
                    .contains('│'),
                "row {i} has a second panel: {row:?}"
            );
        }
        // Both entries are listed, with the dir first and marked.
        assert!(screen.contains("[D] sub"), "{screen}");
        assert!(screen.contains("보고서.txt"), "{screen}");
        // The preview is under it, titled with the selected entry.
        let preview_top = rows
            .iter()
            .rposition(|r| r.starts_with('┌'))
            .expect("a second box below the listing");
        assert!(rows[preview_top].contains("sub"), "{:?}", rows[preview_top]);
        assert!(
            rows[preview_top + 1].contains("안쪽.md"),
            "the directory preview lists its own entries: {:?}",
            rows[preview_top + 1]
        );
        // Footer says what Tab does here.
        assert!(rows.last().unwrap().contains("Tab Prev"), "{screen}");
    }

    #[test]
    fn a_hangul_file_preview_renders_its_own_characters() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("메모.txt"), "첫 번째 줄\n두 번째 줄\n").unwrap();

        let rows = render_dock(34, 24, dir);
        let screen = rows.join("\n");
        assert!(screen.contains("첫 번째 줄"), "{screen}");
        assert!(screen.contains("두 번째 줄"), "{screen}");
        assert!(!screen.contains('\u{fffd}'), "{screen}");
        // Every row fits the terminal: a wide glyph must not push one over.
        for row in &rows {
            assert!(width_of(row) <= 34, "row too wide: {row:?}");
        }
    }

    /// A dock too short to split shows the listing alone, and nothing that
    /// reads as a half-drawn preview.
    #[test]
    fn a_short_dock_drops_the_preview_entirely() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        let rows = render_dock(34, 10, tmp.path());
        assert_eq!(
            rows.iter().filter(|r| r.starts_with('┌')).count(),
            1,
            "only the listing has a box: {rows:?}"
        );
    }

    /// The old byte-slicing title crashed here; the dock is where the
    /// truncation actually happens.
    #[test]
    fn a_narrow_dock_over_a_hangul_path_renders_instead_of_panicking() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("프로젝트 문서 보관함");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("메모.txt"), "내용").unwrap();
        for width in [8u16, 16, 24, 34, 60] {
            let rows = render_dock(width, 24, &dir);
            for row in &rows {
                assert!(width_of(row) <= width as usize, "{width}: {row:?}");
            }
        }
    }
}
