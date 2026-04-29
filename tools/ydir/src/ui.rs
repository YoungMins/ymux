use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::{App, Panel, PanelSide};

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
}

fn draw_panel(frame: &mut Frame, panel: &Panel, area: Rect, active: bool) {
    let border_style = if active {
        Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))
    } else {
        Style::default().fg(Color::Rgb(0x1e, 0x2a, 0x38))
    };

    let title = format!(" {} ", panel.cwd.display());
    let block = Block::default()
        .title(title)
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

    // Header
    let header_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let list_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            pad_right("Name", inner.width as usize / 2),
            Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)),
        ),
        Span::styled(
            pad_right("Size", 10),
            Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)),
        ),
        Span::styled(
            "Modified",
            Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a)),
        ),
    ]));
    frame.render_widget(header, header_area);

    // Scrolling: keep selected item visible
    let visible_height = list_area.height as usize;
    let scroll = if panel.selected >= visible_height {
        panel.selected - visible_height + 1
    } else {
        0
    };

    let items: Vec<ListItem> = panel
        .entries
        .iter()
        .skip(scroll)
        .take(visible_height)
        .enumerate()
        .map(|(i, entry)| {
            let idx = i + scroll;
            let is_selected = idx == panel.selected;

            let icon = if entry.is_dir { "\u{1F4C1} " } else { "   " };
            let name_width = (inner.width as usize).saturating_sub(28);
            let name = truncate(&entry.name, name_width);
            let size = pad_right(&entry.size_display(), 10);
            let date = entry
                .modified
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
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

            let line = Line::from(vec![
                Span::raw(icon),
                Span::raw(pad_right(&name, name_width)),
                Span::raw(size),
                Span::raw(date),
            ]);
            ListItem::new(line).style(style)
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, list_area);

    // Scroll indicator
    if panel.entries.len() > visible_height {
        let pct = if panel.entries.len() <= 1 {
            0
        } else {
            panel.selected * 100 / (panel.entries.len() - 1)
        };
        let indicator = format!(" {}% ", pct);
        let ind_area = Rect {
            x: area.x + area.width - indicator.len() as u16 - 1,
            y: area.y,
            width: indicator.len() as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(indicator).style(Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a))),
            ind_area,
        );
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let status = app.status_msg.as_deref().unwrap_or("");
    let text = Line::from(vec![
        Span::styled("q", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Quit  "),
        Span::styled("Tab", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Switch  "),
        Span::styled("Enter", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Open  "),
        Span::styled("BS", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Parent  "),
        Span::styled(".", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Hidden  "),
        Span::styled("c/m/p/d", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Copy/Move/Paste/Del  "),
        Span::styled(status, Style::default().fg(Color::Rgb(0xe5, 0xc0, 0x7b))),
    ]);
    frame.render_widget(Paragraph::new(text), area);
}

fn pad_right(s: &str, width: usize) -> String {
    if s.len() >= width {
        s[..width].to_string()
    } else {
        format!("{:width$}", s, width = width)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}~", truncated)
    }
}
