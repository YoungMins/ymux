use std::io;
use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;

mod app;
mod buffer;
mod ui;

use app::App;

fn main() -> Result<()> {
    let file_path = std::env::args().nth(1).map(PathBuf::from);

    let mut stdout = io::stdout();
    enable_raw_mode()?;
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, file_path);

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    file_path: Option<PathBuf>,
) -> Result<()> {
    let mut app = App::new(file_path)?;

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Exit dialog takes priority over everything
                    if app.exit_dialog {
                        match key.code {
                            KeyCode::Left | KeyCode::Char('h') => app.exit_dialog_left(),
                            KeyCode::Right | KeyCode::Char('l') => app.exit_dialog_right(),
                            KeyCode::Enter => {
                                if let Some(true) = app.exit_dialog_confirm()? {
                                    return Ok(());
                                }
                            }
                            KeyCode::Esc => app.exit_dialog_cancel(),
                            _ => {}
                        }
                    } else if app.command_mode {
                        match key.code {
                            KeyCode::Esc => app.cancel_command(),
                            KeyCode::Enter => {
                                let should_quit = app.execute_command()?;
                                if should_quit {
                                    return Ok(());
                                }
                            }
                            KeyCode::Backspace => app.command_backspace(),
                            KeyCode::Char(c) => app.command_input(c),
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Esc => app.show_exit_dialog(),
                            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                return Ok(());
                            }
                            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.save()?;
                            }
                            KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.undo();
                            }
                            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.redo();
                            }
                            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.enter_command_mode_with("find ");
                            }
                            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.enter_command_mode_with("goto ");
                            }
                            KeyCode::Char(':') => app.enter_command_mode(),
                            KeyCode::Up => app.move_cursor_up(),
                            KeyCode::Down => app.move_cursor_down(),
                            KeyCode::Left => app.move_cursor_left(),
                            KeyCode::Right => app.move_cursor_right(),
                            KeyCode::Home => app.move_cursor_home(),
                            KeyCode::End => app.move_cursor_end(),
                            KeyCode::PageUp => app.page_up(),
                            KeyCode::PageDown => app.page_down(),
                            KeyCode::Enter => app.insert_newline(),
                            KeyCode::Backspace => app.backspace(),
                            KeyCode::Delete => app.delete_char(),
                            KeyCode::Tab => app.insert_tab(),
                            KeyCode::Char(c) => app.insert_char(c),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}
