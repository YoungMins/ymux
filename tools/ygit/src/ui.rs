use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus};
use crate::graph;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Split: main content | 1-line status bar
    let [main_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

    // Split main: 70% log graph | 30% branch list
    let [log_area, branch_area] =
        Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)])
            .areas(main_area);

    // ── Graph panel ──────────────────────────────────────────────────────────
    let log_focused = app.focus == Focus::Graph;
    let border_style_log = if log_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let log_text = Text::from(
        app.log_lines
            .iter()
            .map(|l| graph::colorize(l))
            .collect::<Vec<_>>(),
    );

    let log_widget = Paragraph::new(log_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style_log)
                .title(" Log "),
        )
        .scroll((app.log_scroll as u16, 0));

    frame.render_widget(log_widget, log_area);

    // ── Branch panel ─────────────────────────────────────────────────────────
    let branch_focused = app.focus == Focus::Branches;
    let border_style_branch = if branch_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let items: Vec<ListItem> = app
        .branches
        .iter()
        .map(|b| {
            if b.trim_start_matches(' ').starts_with('*') {
                ListItem::new(b.as_str()).style(Style::default().fg(Color::Green))
            } else {
                ListItem::new(b.as_str())
            }
        })
        .collect();

    let mut list_state = ListState::default();
    if branch_focused {
        list_state = list_state.with_selected(Some(app.branch_idx));
    }

    let branch_widget = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style_branch)
                .title(" Branches "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(branch_widget, branch_area, &mut list_state);

    // ── Status bar ───────────────────────────────────────────────────────────
    let (status_text, status_color) = match &app.status {
        Some(msg) if msg.starts_with("Error") => (msg.as_str(), Color::Red),
        Some(msg) => (msg.as_str(), Color::Green),
        None => (
            "q=quit  Tab=switch  j/k=scroll  Enter=checkout  r=refresh",
            Color::DarkGray,
        ),
    };

    let status_widget = Paragraph::new(status_text).style(Style::default().fg(status_color));
    frame.render_widget(status_widget, status_area);
}
