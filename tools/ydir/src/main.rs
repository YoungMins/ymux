use std::io;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;

mod app;
mod dock;
mod ui;

use app::App;

fn main() -> Result<()> {
    let args = dock::parse_args(std::env::args().skip(1));
    let follow = dock::follow_host(
        args.dock,
        std::env::var("YMUX_IPC").ok(),
        std::env::var("YMUX_PANE_ID").unwrap_or_default(),
    );
    let start_dir = args
        .dir
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));

    let mut stdout = io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, start_dir, follow.as_ref());

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    start_dir: std::path::PathBuf,
    follow: Option<&std::sync::mpsc::Receiver<std::path::PathBuf>>,
) -> Result<()> {
    let mut app = App::new(start_dir)?;

    loop {
        // Dock mode: apply directory changes pushed by ymux. `event::poll`
        // below wakes at least every 100 ms, which bounds the latency.
        if let Some(rx) = follow {
            while let Ok(dir) = rx.try_recv() {
                app.change_dir(&dir);
            }
        }

        terminal.draw(|frame| ui::draw(frame, &app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Run dialog takes priority
                    if app.run_dialog.is_some() {
                        match key.code {
                            KeyCode::Esc => app.run_dialog_cancel(),
                            KeyCode::Backspace => app.run_dialog_backspace(),
                            KeyCode::Enter => {
                                if let Some(mut cmd) = app.run_dialog_confirm() {
                                    // Suspend TUI, run the command, resume TUI
                                    disable_raw_mode()?;
                                    terminal.backend_mut().execute(LeaveAlternateScreen)?;
                                    terminal.show_cursor()?;

                                    let status = cmd.status();
                                    match status {
                                        Ok(s) => {
                                            if !s.success() {
                                                eprintln!(
                                                    "\n[exited with code {}]",
                                                    s.code().unwrap_or(-1)
                                                );
                                            }
                                        }
                                        Err(e) => eprintln!("\nFailed to run: {}", e),
                                    }
                                    eprintln!("\nPress Enter to return to yDir...");
                                    let _ = std::io::stdin().read_line(&mut String::new());

                                    // Resume TUI
                                    enable_raw_mode()?;
                                    terminal.backend_mut().execute(EnterAlternateScreen)?;
                                    terminal.clear()?;
                                    let _ = app.refresh();
                                }
                            }
                            KeyCode::Char(c) => app.run_dialog_input(c),
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                            KeyCode::Tab => app.toggle_panel(),
                            KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                            KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                            KeyCode::Home => app.move_to_top(),
                            KeyCode::End => app.move_to_bottom(),
                            KeyCode::Enter => {
                                app.enter_dir()?;
                                if let Some(path) = app.open_in_ycode.take() {
                                    disable_raw_mode()?;
                                    terminal.backend_mut().execute(LeaveAlternateScreen)?;
                                    terminal.show_cursor()?;

                                    let status =
                                        std::process::Command::new("ycode").arg(&path).status();
                                    if let Err(e) = status {
                                        eprintln!("\nFailed to launch ycode: {}", e);
                                        eprintln!("Press Enter to return to yDir...");
                                        let _ = std::io::stdin().read_line(&mut String::new());
                                    }

                                    enable_raw_mode()?;
                                    terminal.backend_mut().execute(EnterAlternateScreen)?;
                                    terminal.clear()?;
                                    let _ = app.refresh();
                                }
                            }
                            KeyCode::Backspace => app.go_parent()?,
                            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.refresh()?;
                            }
                            KeyCode::Char('d') => app.delete_selected()?,
                            KeyCode::Char('c') => app.mark_copy(),
                            KeyCode::Char('m') => app.mark_move(),
                            KeyCode::Char('p') => app.paste()?,
                            KeyCode::Char('.') => app.toggle_hidden(),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}
