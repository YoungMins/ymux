use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::{App, Tab};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // tabs
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(frame.area());

    draw_tabs(frame, app, chunks[0]);

    match app.tab() {
        Tab::Overview => draw_overview(frame, app, chunks[1]),
        Tab::Cpu => draw_cpu(frame, app, chunks[1]),
        Tab::Memory => draw_memory(frame, app, chunks[1]),
        Tab::Processes => draw_processes(frame, app, chunks[1]),
    }

    draw_footer(frame, chunks[2]);
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let style = if i == app.active_tab {
                Style::default()
                    .fg(Color::Rgb(0x7f, 0xdb, 0xca))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a))
            };
            Line::from(Span::styled(format!(" {} ", t.label()), style))
        })
        .collect();

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::Rgb(0x1e, 0x2a, 0x38)))
                .title(" ymon ")
                .title_style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)).bold()),
        )
        .select(app.active_tab)
        .highlight_style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)));

    frame.render_widget(tabs, area);
}

fn draw_overview(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // CPU gauge
            Constraint::Length(3), // Memory gauge
            Constraint::Length(3), // Swap gauge
            Constraint::Min(0),    // Disk list
        ])
        .split(area);

    // CPU gauge
    let cpu_pct = app.sys.global_cpu_usage() as f64;
    let cpu_gauge = Gauge::default()
        .block(
            Block::default()
                .title(" CPU ")
                .borders(Borders::ALL)
                .border_style(border_style()),
        )
        .gauge_style(gauge_color(cpu_pct as f32))
        .percent(cpu_pct.min(100.0) as u16)
        .label(format!("{:.1}%", cpu_pct));
    frame.render_widget(cpu_gauge, chunks[0]);

    // Memory gauge
    let mem_pct = if app.total_mem_gb() > 0.0 {
        app.used_mem_gb() / app.total_mem_gb() * 100.0
    } else {
        0.0
    };
    let mem_gauge = Gauge::default()
        .block(
            Block::default()
                .title(format!(
                    " Memory  {:.1} / {:.1} GB ",
                    app.used_mem_gb(),
                    app.total_mem_gb()
                ))
                .borders(Borders::ALL)
                .border_style(border_style()),
        )
        .gauge_style(gauge_color(mem_pct as f32))
        .percent(mem_pct.min(100.0) as u16)
        .label(format!("{:.1}%", mem_pct));
    frame.render_widget(mem_gauge, chunks[1]);

    // Swap gauge
    let swap_pct = if app.total_swap_gb() > 0.0 {
        app.used_swap_gb() / app.total_swap_gb() * 100.0
    } else {
        0.0
    };
    let swap_gauge = Gauge::default()
        .block(
            Block::default()
                .title(format!(
                    " Swap  {:.1} / {:.1} GB ",
                    app.used_swap_gb(),
                    app.total_swap_gb()
                ))
                .borders(Borders::ALL)
                .border_style(border_style()),
        )
        .gauge_style(gauge_color(swap_pct as f32))
        .percent(swap_pct.min(100.0) as u16)
        .label(format!("{:.1}%", swap_pct));
    frame.render_widget(swap_gauge, chunks[2]);

    // Disk list
    let rows: Vec<Row> = app
        .disks
        .list()
        .iter()
        .map(|d| {
            let total = d.total_space() as f64 / 1024.0 / 1024.0 / 1024.0;
            let avail = d.available_space() as f64 / 1024.0 / 1024.0 / 1024.0;
            let used = total - avail;
            let pct = if total > 0.0 {
                used / total * 100.0
            } else {
                0.0
            };
            Row::new(vec![
                Cell::from(d.mount_point().to_string_lossy().to_string()),
                Cell::from(format!("{:.1} GB", total)),
                Cell::from(format!("{:.1} GB", used)),
                Cell::from(format!("{:.1}%", pct))
                    .style(Style::default().fg(pct_color(pct as f32))),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(20),
        ],
    )
    .header(
        Row::new(vec!["Mount", "Total", "Used", "Usage"])
            .style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)).bold()),
    )
    .block(
        Block::default()
            .title(" Disks ")
            .borders(Borders::ALL)
            .border_style(border_style()),
    );
    frame.render_widget(table, chunks[3]);
}

fn draw_cpu(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(0)])
        .split(area);

    // CPU usage sparkline / history chart
    let data: Vec<u64> = app.cpu_history.iter().map(|v| *v as u64).collect();
    let sparkline = Sparkline::default()
        .block(
            Block::default()
                .title(format!(
                    " CPU History ({:.1}%) ",
                    app.sys.global_cpu_usage()
                ))
                .borders(Borders::ALL)
                .border_style(border_style()),
        )
        .data(&data)
        .max(100)
        .style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)));
    frame.render_widget(sparkline, chunks[0]);

    // Per-core bars
    let rows: Vec<Row> = app
        .cpu_snapshots
        .iter()
        .map(|c| {
            let bar = usage_bar(c.usage, 30);
            Row::new(vec![
                Cell::from(c.name.clone()),
                Cell::from(bar).style(Style::default().fg(pct_color(c.usage))),
                Cell::from(format!("{:5.1}%", c.usage)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Min(30),
            Constraint::Length(8),
        ],
    )
    .block(
        Block::default()
            .title(" Per-Core ")
            .borders(Borders::ALL)
            .border_style(border_style()),
    );
    frame.render_widget(table, chunks[1]);
}

fn draw_memory(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(0)])
        .split(area);

    let data: Vec<u64> = app.mem_history.iter().map(|v| *v as u64).collect();
    let sparkline = Sparkline::default()
        .block(
            Block::default()
                .title(format!(
                    " Memory History  {:.1} / {:.1} GB ",
                    app.used_mem_gb(),
                    app.total_mem_gb()
                ))
                .borders(Borders::ALL)
                .border_style(border_style()),
        )
        .data(&data)
        .max(100)
        .style(Style::default().fg(Color::Rgb(0xe5, 0xc0, 0x7b)));
    frame.render_widget(sparkline, chunks[0]);

    // Memory breakdown
    let items = vec![
        format!("Total:     {:.2} GB", app.total_mem_gb()),
        format!("Used:      {:.2} GB", app.used_mem_gb()),
        format!(
            "Available: {:.2} GB",
            app.total_mem_gb() - app.used_mem_gb()
        ),
        String::new(),
        format!("Swap Total: {:.2} GB", app.total_swap_gb()),
        format!("Swap Used:  {:.2} GB", app.used_swap_gb()),
    ];

    let list_items: Vec<ListItem> = items
        .into_iter()
        .map(|s| ListItem::new(s).style(Style::default().fg(Color::Rgb(0xd6, 0xde, 0xeb))))
        .collect();

    let list = List::new(list_items).block(
        Block::default()
            .title(" Details ")
            .borders(Borders::ALL)
            .border_style(border_style()),
    );
    frame.render_widget(list, chunks[1]);
}

fn draw_processes(frame: &mut Frame, app: &App, area: Rect) {
    let visible_start = app.scroll_offset;
    let rows: Vec<Row> = app
        .processes
        .iter()
        .skip(visible_start)
        .map(|p| {
            Row::new(vec![
                Cell::from(format!("{}", p.pid)),
                Cell::from(p.name.clone()),
                Cell::from(format!("{:.1}%", p.cpu)).style(Style::default().fg(pct_color(p.cpu))),
                Cell::from(format!("{:.1} MB", p.memory_mb)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Percentage(50),
            Constraint::Length(10),
            Constraint::Length(12),
        ],
    )
    .header(
        Row::new(vec!["PID", "Name", "CPU", "Memory"])
            .style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)).bold()),
    )
    .block(
        Block::default()
            .title(format!(" Processes ({}) — ↑↓ scroll ", app.processes.len()))
            .borders(Borders::ALL)
            .border_style(border_style()),
    );
    frame.render_widget(table, area);
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    let text = Line::from(vec![
        Span::styled(" q", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Quit  "),
        Span::styled("Tab", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Switch tab  "),
        Span::styled("1-4", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Jump to tab  "),
        Span::styled("↑↓", Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca))),
        Span::raw(" Scroll"),
    ]);
    // Split the footer so the hint text keeps its left position and the
    // ymux release version sits flush against the right edge.
    let version = format!(" v{} ", yversion::VERSION);
    let v_width = version.chars().count() as u16;
    let parts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(v_width)])
        .split(area);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Rgb(0x6a, 0x7a, 0x8a))),
        parts[0],
    );
    frame.render_widget(
        Paragraph::new(version)
            .style(Style::default().fg(Color::Rgb(0x7f, 0xdb, 0xca)))
            .alignment(Alignment::Right),
        parts[1],
    );
}

fn border_style() -> Style {
    Style::default().fg(Color::Rgb(0x1e, 0x2a, 0x38))
}

fn pct_color(pct: f32) -> Color {
    if pct >= 90.0 {
        Color::Rgb(0xef, 0x6b, 0x73)
    } else if pct >= 70.0 {
        Color::Rgb(0xe5, 0xc0, 0x7b)
    } else {
        Color::Rgb(0x7f, 0xdb, 0xca)
    }
}

fn gauge_color(pct: f32) -> Style {
    Style::default().fg(pct_color(pct))
}

fn usage_bar(pct: f32, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_color_thresholds() {
        assert_eq!(pct_color(50.0), Color::Rgb(0x7f, 0xdb, 0xca));
        assert_eq!(pct_color(75.0), Color::Rgb(0xe5, 0xc0, 0x7b));
        assert_eq!(pct_color(95.0), Color::Rgb(0xef, 0x6b, 0x73));
    }

    #[test]
    fn usage_bar_length() {
        let bar = usage_bar(50.0, 20);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn usage_bar_extremes() {
        let full = usage_bar(100.0, 10);
        assert_eq!(full, "██████████");
        let empty = usage_bar(0.0, 10);
        assert_eq!(empty, "░░░░░░░░░░");
    }
}
