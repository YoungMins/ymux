use std::path::PathBuf;

use anyhow::Result;

use crate::buffer::Buffer;

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
}
