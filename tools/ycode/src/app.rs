use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::buffer::Buffer;
use crate::highlight::Highlighter;
use crate::sidebar::Sidebar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitChoice {
    Save,
    Quit,
    Cancel,
}

impl ExitChoice {
    pub const ALL: [ExitChoice; 3] = [ExitChoice::Save, ExitChoice::Quit, ExitChoice::Cancel];

    pub fn label(self) -> &'static str {
        match self {
            ExitChoice::Save => "Save & Quit",
            ExitChoice::Quit => "Quit without saving",
            ExitChoice::Cancel => "Cancel",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchChoice {
    Save,
    Discard,
    Cancel,
}

impl SwitchChoice {
    pub const ALL: [SwitchChoice; 3] = [
        SwitchChoice::Save,
        SwitchChoice::Discard,
        SwitchChoice::Cancel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SwitchChoice::Save => "Save & Open",
            SwitchChoice::Discard => "Discard & Open",
            SwitchChoice::Cancel => "Cancel",
        }
    }
}

/// Active "current buffer is dirty — what now?" modal raised when the user
/// picks a different file in the sidebar before saving the open one.
pub struct SwitchPrompt {
    pub target: PathBuf,
    pub choice: usize,
}

pub struct App {
    pub buffer: Buffer,
    pub file_path: Option<PathBuf>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll_row: usize,
    pub scroll_col: usize,
    pub viewport_rows: usize,
    pub command_mode: bool,
    pub command_input: String,
    pub status_msg: Option<String>,
    pub exit_dialog: bool,
    pub exit_choice: usize,
    pub highlighter: Highlighter,
    pub sidebar: Sidebar,
    pub sidebar_open: bool,
    pub switch_prompt: Option<SwitchPrompt>,
}

impl App {
    pub fn new(file_path: Option<PathBuf>) -> Result<Self> {
        let buffer = if let Some(ref path) = file_path {
            if path.exists() {
                let text = std::fs::read_to_string(path)?;
                Buffer::from_text(&text)
            } else {
                Buffer::new()
            }
        } else {
            Buffer::new()
        };

        let theme = ytheme::load_theme();
        let highlighter = Highlighter::new(file_path.as_deref(), &theme);
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let sidebar = Sidebar::new(cwd);

        Ok(Self {
            buffer,
            file_path,
            cursor_row: 0,
            cursor_col: 0,
            scroll_row: 0,
            scroll_col: 0,
            viewport_rows: 24,
            command_mode: false,
            command_input: String::new(),
            status_msg: None,
            exit_dialog: false,
            exit_choice: 2, // default to Cancel
            highlighter,
            sidebar,
            sidebar_open: false,
            switch_prompt: None,
        })
    }

    pub fn save(&mut self) -> Result<()> {
        if let Some(ref path) = self.file_path {
            std::fs::write(path, self.buffer.content())?;
            self.buffer.dirty = false;
            self.status_msg = Some(format!("Saved: {}", path.display()));
        } else {
            self.status_msg = Some("No file path — use :w <path>".to_string());
        }
        Ok(())
    }

    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert_char(self.cursor_row, self.cursor_col, c);
        self.cursor_col += 1;
    }

    pub fn insert_newline(&mut self) {
        self.buffer.insert_newline(self.cursor_row, self.cursor_col);
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.ensure_scroll();
    }

    pub fn backspace(&mut self) {
        let (r, c) = self.buffer.backspace(self.cursor_row, self.cursor_col);
        self.cursor_row = r;
        self.cursor_col = c;
        self.ensure_scroll();
    }

    pub fn delete_char(&mut self) {
        self.buffer.delete_char(self.cursor_row, self.cursor_col);
    }

    pub fn insert_tab(&mut self) {
        self.buffer.insert_tab(self.cursor_row, self.cursor_col);
        self.cursor_col += 4;
    }

    pub fn undo(&mut self) {
        if let Some((r, c)) = self.buffer.undo(self.cursor_row, self.cursor_col) {
            self.cursor_row = r;
            self.cursor_col = c;
            self.ensure_scroll();
        }
    }

    pub fn redo(&mut self) {
        if let Some((r, c)) = self.buffer.redo(self.cursor_row, self.cursor_col) {
            self.cursor_row = r;
            self.cursor_col = c;
            self.ensure_scroll();
        }
    }

    pub fn move_cursor_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.clamp_col();
            self.ensure_scroll();
        }
    }

    pub fn move_cursor_down(&mut self) {
        if self.cursor_row < self.buffer.line_count() - 1 {
            self.cursor_row += 1;
            self.clamp_col();
            self.ensure_scroll();
        }
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.buffer.line_len(self.cursor_row);
            self.ensure_scroll();
        }
    }

    pub fn move_cursor_right(&mut self) {
        let line_len = self.buffer.line_len(self.cursor_row);
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_row < self.buffer.line_count() - 1 {
            self.cursor_row += 1;
            self.cursor_col = 0;
            self.ensure_scroll();
        }
    }

    pub fn move_cursor_home(&mut self) {
        self.cursor_col = 0;
    }

    pub fn move_cursor_end(&mut self) {
        self.cursor_col = self.buffer.line_len(self.cursor_row);
    }

    pub fn page_up(&mut self) {
        self.cursor_row = self.cursor_row.saturating_sub(self.viewport_rows);
        self.clamp_col();
        self.ensure_scroll();
    }

    pub fn page_down(&mut self) {
        self.cursor_row =
            (self.cursor_row + self.viewport_rows).min(self.buffer.line_count().saturating_sub(1));
        self.clamp_col();
        self.ensure_scroll();
    }

    pub fn show_exit_dialog(&mut self) {
        if self.command_mode {
            self.command_mode = false;
            self.command_input.clear();
            return;
        }
        self.exit_dialog = true;
        self.exit_choice = 2; // default Cancel
    }

    pub fn exit_dialog_left(&mut self) {
        self.exit_choice = self.exit_choice.saturating_sub(1);
    }

    pub fn exit_dialog_right(&mut self) {
        self.exit_choice = (self.exit_choice + 1).min(ExitChoice::ALL.len() - 1);
    }

    /// Returns Some(true) if should quit, Some(false) if saved+quit, None if cancelled.
    pub fn exit_dialog_confirm(&mut self) -> Result<Option<bool>> {
        self.exit_dialog = false;
        match ExitChoice::ALL[self.exit_choice] {
            ExitChoice::Save => {
                self.save()?;
                Ok(Some(true))
            }
            ExitChoice::Quit => Ok(Some(true)),
            ExitChoice::Cancel => Ok(None),
        }
    }

    pub fn exit_dialog_cancel(&mut self) {
        self.exit_dialog = false;
    }

    // ── Sidebar ─────────────────────────────────────────────────────

    pub fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
    }

    /// Enter pressed inside the sidebar. Dirs toggle expand/collapse;
    /// files request open (which may surface the dirty-buffer prompt).
    pub fn sidebar_enter(&mut self) -> Result<()> {
        if self.sidebar.toggle_expand_selected() {
            return Ok(());
        }
        let target = match self.sidebar.selected_entry() {
            Some(e) if !e.is_dir => e.path.clone(),
            _ => return Ok(()),
        };
        self.request_open(target)
    }

    fn request_open(&mut self, target: PathBuf) -> Result<()> {
        if self.file_path.as_deref() == Some(target.as_path()) {
            self.sidebar_open = false;
            return Ok(());
        }
        if self.buffer.dirty {
            self.switch_prompt = Some(SwitchPrompt { target, choice: 2 });
            return Ok(());
        }
        self.load_file(&target)
    }

    /// Replace the open buffer with the file at `path`, reset cursor and
    /// scroll, point the highlighter at the new extension, close the
    /// sidebar. Failures (missing file, permission denied, etc.) surface
    /// as a status message rather than crashing the editor.
    fn load_file(&mut self, path: &Path) -> Result<()> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.buffer = Buffer::from_text(&text);
                self.file_path = Some(path.to_path_buf());
                self.cursor_row = 0;
                self.cursor_col = 0;
                self.scroll_row = 0;
                self.scroll_col = 0;
                self.highlighter.set_file(self.file_path.as_deref());
                self.sidebar_open = false;
                self.status_msg = Some(format!("Opened: {}", path.display()));
            }
            Err(e) => {
                self.status_msg = Some(format!("Failed to open {}: {}", path.display(), e));
            }
        }
        Ok(())
    }

    // ── Switch prompt (dirty buffer ↔ "open different file") ────────

    pub fn switch_prompt_left(&mut self) {
        if let Some(p) = &mut self.switch_prompt {
            p.choice = p.choice.saturating_sub(1);
        }
    }

    pub fn switch_prompt_right(&mut self) {
        if let Some(p) = &mut self.switch_prompt {
            p.choice = (p.choice + 1).min(SwitchChoice::ALL.len() - 1);
        }
    }

    pub fn switch_prompt_confirm(&mut self) -> Result<()> {
        let Some(p) = self.switch_prompt.take() else {
            return Ok(());
        };
        match SwitchChoice::ALL[p.choice] {
            SwitchChoice::Save => {
                if self.file_path.is_none() {
                    self.status_msg =
                        Some("Untitled buffer — use :w <path> before opening another file".into());
                    return Ok(());
                }
                self.save()?;
                self.load_file(&p.target)?;
            }
            SwitchChoice::Discard => {
                self.load_file(&p.target)?;
            }
            SwitchChoice::Cancel => {}
        }
        Ok(())
    }

    pub fn switch_prompt_cancel(&mut self) {
        self.switch_prompt = None;
    }

    pub fn enter_command_mode(&mut self) {
        self.command_mode = true;
        self.command_input.clear();
    }

    pub fn enter_command_mode_with(&mut self, prefix: &str) {
        self.command_mode = true;
        self.command_input = prefix.to_string();
    }

    pub fn cancel_command(&mut self) {
        self.command_mode = false;
        self.command_input.clear();
    }

    pub fn command_input(&mut self, c: char) {
        self.command_input.push(c);
    }

    pub fn command_backspace(&mut self) {
        self.command_input.pop();
        if self.command_input.is_empty() {
            self.command_mode = false;
        }
    }

    /// Returns true if the app should quit.
    pub fn execute_command(&mut self) -> Result<bool> {
        let cmd = self.command_input.trim().to_string();
        self.command_mode = false;
        self.command_input.clear();

        if cmd == "q" || cmd == "quit" {
            return Ok(true);
        }
        if cmd == "q!" {
            return Ok(true);
        }
        if cmd == "w" || cmd == "save" {
            self.save()?;
        } else if cmd == "wq" {
            self.save()?;
            return Ok(true);
        } else if let Some(path) = cmd.strip_prefix("w ") {
            self.file_path = Some(PathBuf::from(path.trim()));
            self.save()?;
        } else if let Some(line) = cmd.strip_prefix("goto ") {
            if let Ok(n) = line.trim().parse::<usize>() {
                self.cursor_row = n.saturating_sub(1).min(self.buffer.line_count() - 1);
                self.clamp_col();
                self.ensure_scroll();
            }
        } else if let Some(query) = cmd.strip_prefix("find ") {
            self.find_next(query);
        } else {
            self.status_msg = Some(format!("Unknown command: {}", cmd));
        }
        Ok(false)
    }

    fn find_next(&mut self, query: &str) {
        for row in self.cursor_row..self.buffer.line_count() {
            let start_col = if row == self.cursor_row {
                self.cursor_col + 1
            } else {
                0
            };
            let line = self.buffer.line(row);
            if let Some(pos) = line[start_col.min(line.len())..].find(query) {
                self.cursor_row = row;
                self.cursor_col = start_col + pos;
                self.ensure_scroll();
                self.status_msg = Some(format!("Found at {}:{}", row + 1, self.cursor_col + 1));
                return;
            }
        }
        // Wrap around from the beginning
        for row in 0..=self.cursor_row {
            let line = self.buffer.line(row);
            if let Some(pos) = line.find(query) {
                self.cursor_row = row;
                self.cursor_col = pos;
                self.ensure_scroll();
                self.status_msg = Some(format!("Found at {}:{} (wrapped)", row + 1, pos + 1));
                return;
            }
        }
        self.status_msg = Some(format!("Not found: {}", query));
    }

    fn clamp_col(&mut self) {
        let len = self.buffer.line_len(self.cursor_row);
        if self.cursor_col > len {
            self.cursor_col = len;
        }
    }

    fn ensure_scroll(&mut self) {
        if self.cursor_row < self.scroll_row {
            self.scroll_row = self.cursor_row;
        }
        if self.cursor_row >= self.scroll_row + self.viewport_rows {
            self.scroll_row = self.cursor_row - self.viewport_rows + 1;
        }
    }

    pub fn title(&self) -> String {
        let name = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "[untitled]".to_string());
        if self.buffer.dirty {
            format!("{}*", name)
        } else {
            name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_text(text: &str) -> App {
        let highlighter = Highlighter::new(None, &ytheme::Theme::default());
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let sidebar = Sidebar::new(cwd);
        App {
            buffer: Buffer::from_text(text),
            file_path: None,
            cursor_row: 0,
            cursor_col: 0,
            scroll_row: 0,
            scroll_col: 0,
            viewport_rows: 10,
            command_mode: false,
            command_input: String::new(),
            status_msg: None,
            exit_dialog: false,
            exit_choice: 2,
            highlighter,
            sidebar,
            sidebar_open: false,
            switch_prompt: None,
        }
    }

    #[test]
    fn cursor_movement() {
        let mut app = app_with_text("hello\nworld\nfoo");
        app.move_cursor_down();
        assert_eq!(app.cursor_row, 1);
        app.move_cursor_end();
        assert_eq!(app.cursor_col, 5);
        app.move_cursor_home();
        assert_eq!(app.cursor_col, 0);
        app.move_cursor_up();
        assert_eq!(app.cursor_row, 0);
    }

    #[test]
    fn cursor_clamps_col_on_short_line() {
        let mut app = app_with_text("long line here\nhi");
        app.cursor_col = 14;
        app.move_cursor_down();
        assert_eq!(app.cursor_col, 2); // clamped to "hi" length
    }

    #[test]
    fn insert_and_backspace() {
        let mut app = app_with_text("hello");
        app.cursor_col = 5;
        app.insert_char('!');
        assert_eq!(app.buffer.line(0), "hello!");
        app.backspace();
        assert_eq!(app.buffer.line(0), "hello");
    }

    #[test]
    fn newline_and_join() {
        let mut app = app_with_text("helloworld");
        app.cursor_col = 5;
        app.insert_newline();
        assert_eq!(app.buffer.line_count(), 2);
        assert_eq!(app.cursor_row, 1);
        assert_eq!(app.cursor_col, 0);
        app.backspace();
        assert_eq!(app.buffer.line_count(), 1);
        assert_eq!(app.buffer.line(0), "helloworld");
    }

    #[test]
    fn undo_redo() {
        let mut app = app_with_text("hello");
        app.cursor_col = 5;
        app.insert_char('!');
        assert_eq!(app.buffer.line(0), "hello!");
        app.undo();
        assert_eq!(app.buffer.line(0), "hello");
        app.redo();
        assert_eq!(app.buffer.line(0), "hello!");
    }

    #[test]
    fn command_mode() {
        let mut app = app_with_text("hello");
        app.enter_command_mode();
        assert!(app.command_mode);
        app.command_input('q');
        assert_eq!(app.command_input, "q");
        let quit = app.execute_command().unwrap();
        assert!(quit);
    }

    #[test]
    fn goto_command() {
        let mut app = app_with_text("line1\nline2\nline3\nline4");
        app.enter_command_mode_with("goto 3");
        app.execute_command().unwrap();
        assert_eq!(app.cursor_row, 2);
    }

    #[test]
    fn find_command() {
        let mut app = app_with_text("hello world\nfoo bar");
        app.enter_command_mode_with("find bar");
        app.execute_command().unwrap();
        assert_eq!(app.cursor_row, 1);
        assert_eq!(app.cursor_col, 4);
    }

    #[test]
    fn title_shows_dirty() {
        let mut app = app_with_text("hello");
        assert!(!app.title().contains('*'));
        app.insert_char('x');
        assert!(app.title().contains('*'));
    }

    #[test]
    fn save_to_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut app = App::new(Some(path.clone())).unwrap();
        app.insert_char('h');
        app.insert_char('i');
        app.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi");
    }

    #[test]
    fn page_up_down() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app_with_text(&text);
        app.viewport_rows = 10;
        app.page_down();
        assert_eq!(app.cursor_row, 10);
        app.page_up();
        assert_eq!(app.cursor_row, 0);
    }

    #[test]
    fn scroll_follows_cursor() {
        let text = (0..50)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app_with_text(&text);
        app.viewport_rows = 10;
        for _ in 0..15 {
            app.move_cursor_down();
        }
        assert!(app.scroll_row > 0);
        assert!(app.cursor_row >= app.scroll_row);
        assert!(app.cursor_row < app.scroll_row + app.viewport_rows);
    }

    #[test]
    fn scroll_fills_viewport_at_end_of_file() {
        // Regression: main.rs used to never update `viewport_rows`, so on a
        // 40-row terminal the scroll math capped `scroll_row` at
        // `line_count - 24`, leaving 16 trailing rows of "~" once the cursor
        // reached the bottom. When the viewport is synced to actual editor
        // height, the entire viewport stays filled with real content.
        let text = (0..100)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app_with_text(&text);
        app.viewport_rows = 40;
        for _ in 0..100 {
            app.move_cursor_down();
        }
        assert_eq!(app.cursor_row, 99);
        // With viewport_rows = 40, the bottom of the viewport sits exactly on
        // the last line of the file.
        assert_eq!(app.scroll_row, 100 - app.viewport_rows);
        assert!(app.cursor_row < app.scroll_row + app.viewport_rows);
    }

    #[test]
    fn exit_dialog_cancel() {
        let mut app = app_with_text("hello");
        assert!(!app.exit_dialog);
        app.show_exit_dialog();
        assert!(app.exit_dialog);
        app.exit_dialog_cancel();
        assert!(!app.exit_dialog);
    }

    #[test]
    fn exit_dialog_quit_without_save() {
        let mut app = app_with_text("hello");
        app.show_exit_dialog();
        app.exit_choice = 1; // Quit without saving
        let result = app.exit_dialog_confirm().unwrap();
        assert_eq!(result, Some(true));
        assert!(!app.exit_dialog);
    }

    #[test]
    fn exit_dialog_save_and_quit() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut app = App::new(Some(path.clone())).unwrap();
        app.insert_char('x');
        assert!(app.buffer.dirty);
        app.show_exit_dialog();
        app.exit_choice = 0; // Save & Quit
        let result = app.exit_dialog_confirm().unwrap();
        assert_eq!(result, Some(true));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x");
    }

    #[test]
    fn exit_dialog_navigation() {
        let mut app = app_with_text("hello");
        app.show_exit_dialog();
        assert_eq!(app.exit_choice, 2); // default = Cancel
        app.exit_dialog_left();
        assert_eq!(app.exit_choice, 1);
        app.exit_dialog_left();
        assert_eq!(app.exit_choice, 0);
        app.exit_dialog_left();
        assert_eq!(app.exit_choice, 0); // clamped
        app.exit_dialog_right();
        assert_eq!(app.exit_choice, 1);
    }

    #[test]
    fn esc_in_command_mode_exits_command_not_dialog() {
        let mut app = app_with_text("hello");
        app.enter_command_mode();
        assert!(app.command_mode);
        app.show_exit_dialog(); // should exit command mode, not show dialog
        assert!(!app.command_mode);
        assert!(!app.exit_dialog);
    }
}
