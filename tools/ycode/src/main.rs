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
mod highlight;
mod markdown;
mod sidebar;
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

/// Layout overhead consumed by non-editor chrome (title bar + status line +
/// command/message line). Kept in sync with the constraints in `ui::draw`.
const EDITOR_CHROME_ROWS: u16 = 3;

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    file_path: Option<PathBuf>,
) -> Result<()> {
    let mut app = App::new(file_path)?;

    loop {
        // Sync viewport_rows with the actual editor area before drawing so
        // scrolling math matches what the user sees. Without this, scroll
        // and page-up/down use a stale 24-row viewport and the bottom of the
        // editor fills with "~" placeholders once the cursor passes line 24.
        let size = terminal.size()?;
        app.viewport_rows = size.height.saturating_sub(EDITOR_CHROME_ROWS).max(1) as usize;
        if app.sidebar_open {
            app.sidebar
                .ensure_scroll(app.viewport_rows.saturating_sub(1));
        }

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
                    } else if app.switch_prompt.is_some() {
                        match key.code {
                            KeyCode::Left | KeyCode::Char('h') => app.switch_prompt_left(),
                            KeyCode::Right | KeyCode::Char('l') => app.switch_prompt_right(),
                            KeyCode::Enter => app.switch_prompt_confirm()?,
                            KeyCode::Esc => app.switch_prompt_cancel(),
                            _ => {}
                        }
                    } else if app.sidebar_open {
                        match key.code {
                            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.toggle_sidebar();
                            }
                            KeyCode::Esc => app.toggle_sidebar(),
                            KeyCode::Up => app.sidebar.move_up(),
                            KeyCode::Down => app.sidebar.move_down(),
                            KeyCode::PageUp => app.sidebar.page_up(app.viewport_rows),
                            KeyCode::PageDown => app.sidebar.page_down(app.viewport_rows),
                            KeyCode::Home => app.sidebar.move_home(),
                            KeyCode::End => app.sidebar.move_end(),
                            KeyCode::Enter => app.sidebar_enter()?,
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
                    } else if app.preview_mode {
                        // Read-only markdown preview: only navigation,
                        // save, sidebar, and toggles. Esc exits preview
                        // rather than opening the exit dialog so the user
                        // can get back to editing without confirming.
                        //
                        // NOTE: Alt+M (not Ctrl+M) is the toggle. Ctrl+M is
                        // indistinguishable from Enter in every terminal
                        // that doesn't speak the kitty keyboard protocol —
                        // both send the carriage-return byte.
                        match key.code {
                            KeyCode::Esc => app.toggle_preview(),
                            KeyCode::Char('m') | KeyCode::Char('M')
                                if key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                app.toggle_preview();
                            }
                            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                return Ok(());
                            }
                            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.save()?;
                            }
                            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.toggle_sidebar();
                            }
                            KeyCode::Up => app.preview_scroll_up(),
                            KeyCode::Down => app.preview_scroll_down(),
                            KeyCode::PageUp => app.preview_page_up(),
                            KeyCode::PageDown => app.preview_page_down(),
                            KeyCode::Home => app.preview_home(),
                            KeyCode::End => app.preview_end(),
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
                            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.toggle_sidebar();
                            }
                            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.enter_command_mode_with("find ");
                            }
                            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.enter_command_mode_with("goto ");
                            }
                            KeyCode::Char('m') | KeyCode::Char('M')
                                if key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                app.toggle_preview();
                            }
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
