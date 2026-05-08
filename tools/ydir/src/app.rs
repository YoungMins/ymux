use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<chrono::NaiveDateTime>,
}

impl FileEntry {
    pub fn size_display(&self) -> String {
        if self.is_dir {
            "<DIR>".to_string()
        } else if self.size < 1024 {
            format!("{} B", self.size)
        } else if self.size < 1024 * 1024 {
            format!("{:.1} KB", self.size as f64 / 1024.0)
        } else if self.size < 1024 * 1024 * 1024 {
            format!("{:.1} MB", self.size as f64 / 1024.0 / 1024.0)
        } else {
            format!("{:.1} GB", self.size as f64 / 1024.0 / 1024.0 / 1024.0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSide {
    Left,
    Right,
}

#[derive(Debug)]
pub struct Panel {
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub selected: usize,
}

impl Panel {
    pub fn new(dir: &Path) -> Result<Self> {
        let mut panel = Self {
            cwd: dir.to_path_buf(),
            entries: Vec::new(),
            selected: 0,
        };
        panel.reload(false)?;
        Ok(panel)
    }

    pub fn reload(&mut self, show_hidden: bool) -> Result<()> {
        self.entries = list_dir(&self.cwd, show_hidden)?;
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
        Ok(())
    }

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.selected)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardOp {
    Copy,
    Move,
}

/// Run dialog state: shown when user presses Enter on an executable file.
pub struct RunDialog {
    pub file_path: PathBuf,
    pub file_name: String,
    pub args_input: String,
}

pub struct App {
    pub left: Panel,
    pub right: Panel,
    pub active: PanelSide,
    pub show_hidden: bool,
    pub clipboard: Option<(PathBuf, ClipboardOp)>,
    pub status_msg: Option<String>,
    pub run_dialog: Option<RunDialog>,
    pub open_in_ycode: Option<PathBuf>,
}

impl App {
    pub fn new(start_dir: PathBuf) -> Result<Self> {
        let left = Panel::new(&start_dir)?;
        let right = Panel::new(&start_dir)?;
        Ok(Self {
            left,
            right,
            active: PanelSide::Left,
            show_hidden: false,
            clipboard: None,
            status_msg: None,
            run_dialog: None,
            open_in_ycode: None,
        })
    }

    pub fn active_panel(&self) -> &Panel {
        match self.active {
            PanelSide::Left => &self.left,
            PanelSide::Right => &self.right,
        }
    }

    fn active_panel_mut(&mut self) -> &mut Panel {
        match self.active {
            PanelSide::Left => &mut self.left,
            PanelSide::Right => &mut self.right,
        }
    }

    pub fn toggle_panel(&mut self) {
        self.active = match self.active {
            PanelSide::Left => PanelSide::Right,
            PanelSide::Right => PanelSide::Left,
        };
    }

    pub fn move_up(&mut self) {
        let panel = self.active_panel_mut();
        panel.selected = panel.selected.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        let panel = self.active_panel_mut();
        if !panel.entries.is_empty() {
            panel.selected = (panel.selected + 1).min(panel.entries.len() - 1);
        }
    }

    pub fn move_to_top(&mut self) {
        self.active_panel_mut().selected = 0;
    }

    pub fn move_to_bottom(&mut self) {
        let panel = self.active_panel_mut();
        panel.selected = panel.entries.len().saturating_sub(1);
    }

    pub fn enter_dir(&mut self) -> Result<()> {
        let entry = self.active_panel().selected_entry().cloned();
        if let Some(entry) = entry {
            if entry.is_dir {
                let show_hidden = self.show_hidden;
                let panel = self.active_panel_mut();
                panel.cwd = entry.path;
                panel.selected = 0;
                panel.reload(show_hidden)?;
            } else if is_executable(&entry.name) {
                self.run_dialog = Some(RunDialog {
                    file_name: entry.name.clone(),
                    file_path: entry.path.clone(),
                    args_input: String::new(),
                });
            } else if is_binary_file(&entry.path) {
                self.status_msg = Some(format!("Binary file: {}", entry.name));
            } else {
                self.open_in_ycode = Some(entry.path.clone());
            }
        }
        Ok(())
    }

    pub fn run_dialog_input(&mut self, c: char) {
        if let Some(ref mut dlg) = self.run_dialog {
            dlg.args_input.push(c);
        }
    }

    pub fn run_dialog_backspace(&mut self) {
        if let Some(ref mut dlg) = self.run_dialog {
            dlg.args_input.pop();
        }
    }

    pub fn run_dialog_cancel(&mut self) {
        self.run_dialog = None;
    }

    /// Execute the file with args from the dialog. Returns the Command to run.
    pub fn run_dialog_confirm(&mut self) -> Option<std::process::Command> {
        let dlg = self.run_dialog.take()?;
        let mut cmd = std::process::Command::new(&dlg.file_path);
        let args_str = dlg.args_input.trim();
        if !args_str.is_empty() {
            for arg in args_str.split_whitespace() {
                cmd.arg(arg);
            }
        }
        // Run in the active panel's cwd
        cmd.current_dir(&self.active_panel().cwd);
        self.status_msg = Some(format!("Running: {} {}", dlg.file_name, args_str));
        Some(cmd)
    }

    pub fn go_parent(&mut self) -> Result<()> {
        let show_hidden = self.show_hidden;
        let panel = self.active_panel_mut();
        if let Some(parent) = panel.cwd.parent().map(|p| p.to_path_buf()) {
            panel.cwd = parent;
            panel.selected = 0;
            panel.reload(show_hidden)?;
        }
        Ok(())
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.left.reload(self.show_hidden)?;
        self.right.reload(self.show_hidden)?;
        self.status_msg = Some("Refreshed".to_string());
        Ok(())
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let _ = self.left.reload(self.show_hidden);
        let _ = self.right.reload(self.show_hidden);
        self.status_msg = Some(if self.show_hidden {
            "Hidden files: shown".to_string()
        } else {
            "Hidden files: hidden".to_string()
        });
    }

    pub fn delete_selected(&mut self) -> Result<()> {
        let show_hidden = self.show_hidden;
        let entry = self.active_panel().selected_entry().cloned();
        if let Some(entry) = entry {
            if entry.is_dir {
                fs::remove_dir_all(&entry.path)?;
            } else {
                fs::remove_file(&entry.path)?;
            }
            self.active_panel_mut().reload(show_hidden)?;
            self.status_msg = Some(format!("Deleted: {}", entry.name));
        }
        Ok(())
    }

    pub fn mark_copy(&mut self) {
        let info = self
            .active_panel()
            .selected_entry()
            .map(|e| (e.path.clone(), e.name.clone()));
        if let Some((path, name)) = info {
            self.clipboard = Some((path, ClipboardOp::Copy));
            self.status_msg = Some(format!("Copied: {}", name));
        }
    }

    pub fn mark_move(&mut self) {
        let info = self
            .active_panel()
            .selected_entry()
            .map(|e| (e.path.clone(), e.name.clone()));
        if let Some((path, name)) = info {
            self.clipboard = Some((path, ClipboardOp::Move));
            self.status_msg = Some(format!("Cut: {}", name));
        }
    }

    pub fn paste(&mut self) -> Result<()> {
        if let Some((src, op)) = self.clipboard.take() {
            let dest_dir = self.active_panel().cwd.clone();
            let filename = src
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let dest = dest_dir.join(&filename);

            match op {
                ClipboardOp::Copy => {
                    if src.is_dir() {
                        copy_dir_recursive(&src, &dest)?;
                    } else {
                        fs::copy(&src, &dest)?;
                    }
                    self.status_msg = Some(format!("Pasted: {}", filename));
                }
                ClipboardOp::Move => {
                    fs::rename(&src, &dest)?;
                    self.status_msg = Some(format!("Moved: {}", filename));
                }
            }

            let show_hidden = self.show_hidden;
            self.left.reload(show_hidden)?;
            self.right.reload(show_hidden)?;
        }
        Ok(())
    }
}

pub fn is_binary_file(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = [0u8; 8192];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    buf[..n].contains(&0u8)
}

fn is_executable(name: &str) -> bool {
    let lower = name.to_lowercase();
    matches!(
        lower.rsplit('.').next(),
        Some("exe" | "bat" | "cmd" | "ps1" | "com" | "msi" | "sh" | "py" | "rb" | "js")
    )
}

fn list_dir(dir: &Path, show_hidden: bool) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(dir)?;

    for item in read_dir {
        let item = item?;
        let name = item.file_name().to_string_lossy().to_string();

        if !show_hidden && name.starts_with('.') {
            continue;
        }

        let meta = item.metadata()?;
        let modified = meta.modified().ok().and_then(|t| {
            let dur = t.duration_since(std::time::UNIX_EPOCH).ok()?;
            chrono::DateTime::from_timestamp(dur.as_secs() as i64, 0).map(|dt| dt.naive_utc())
        });

        entries.push(FileEntry {
            name,
            path: item.path(),
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified,
        });
    }

    // Directories first, then alphabetical
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_temp_dir() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_path_buf();

        fs::create_dir(path.join("subdir")).unwrap();
        fs::write(path.join("file_a.txt"), "hello").unwrap();
        fs::write(path.join("file_b.rs"), "fn main() {}").unwrap();
        fs::write(path.join(".hidden"), "secret").unwrap();

        (tmp, path)
    }

    #[test]
    fn list_dir_excludes_hidden_by_default() {
        let (_tmp, path) = setup_temp_dir();
        let entries = list_dir(&path, false).unwrap();
        assert!(
            !entries.iter().any(|e| e.name == ".hidden"),
            "should not include hidden files"
        );
    }

    #[test]
    fn list_dir_includes_hidden_when_asked() {
        let (_tmp, path) = setup_temp_dir();
        let entries = list_dir(&path, true).unwrap();
        assert!(
            entries.iter().any(|e| e.name == ".hidden"),
            "should include hidden files"
        );
    }

    #[test]
    fn list_dir_sorts_dirs_first() {
        let (_tmp, path) = setup_temp_dir();
        let entries = list_dir(&path, false).unwrap();
        let first_file_idx = entries.iter().position(|e| !e.is_dir);
        let last_dir_idx = entries.iter().rposition(|e| e.is_dir);
        if let (Some(first_file), Some(last_dir)) = (first_file_idx, last_dir_idx) {
            assert!(last_dir < first_file, "dirs should come before files");
        }
    }

    #[test]
    fn app_navigation() {
        let (_tmp, path) = setup_temp_dir();
        let mut app = App::new(path).unwrap();
        assert_eq!(app.active, PanelSide::Left);
        assert!(!app.left.entries.is_empty());

        app.move_down();
        assert!(app.active_panel().selected <= app.active_panel().entries.len());

        app.move_to_top();
        assert_eq!(app.active_panel().selected, 0);

        app.toggle_panel();
        assert_eq!(app.active, PanelSide::Right);
    }

    #[test]
    fn enter_and_go_parent() {
        let (_tmp, path) = setup_temp_dir();
        let mut app = App::new(path.clone()).unwrap();

        // The first entry should be "subdir" (dirs sort first)
        let first = app.left.entries[0].clone();
        assert!(first.is_dir);

        app.enter_dir().unwrap();
        assert_eq!(app.left.cwd, first.path);

        app.go_parent().unwrap();
        assert_eq!(app.left.cwd, path);
    }

    #[test]
    fn toggle_hidden_refreshes() {
        let (_tmp, path) = setup_temp_dir();
        let mut app = App::new(path).unwrap();
        let count_before = app.left.entries.len();
        app.toggle_hidden();
        let count_after = app.left.entries.len();
        assert!(count_after > count_before);
    }

    #[test]
    fn copy_and_paste() {
        let (_tmp, path) = setup_temp_dir();
        let mut app = App::new(path.clone()).unwrap();

        // Select file_a.txt
        let file_idx = app
            .left
            .entries
            .iter()
            .position(|e| e.name == "file_a.txt")
            .unwrap();
        app.left.selected = file_idx;
        app.mark_copy();
        assert!(app.clipboard.is_some());

        // Switch to right panel which is in the same dir — navigate into subdir
        app.toggle_panel();
        app.right.selected = 0;
        app.enter_dir().unwrap(); // enter subdir
        app.paste().unwrap();

        assert!(app.right.cwd.join("file_a.txt").exists());
    }

    #[test]
    fn delete_file() {
        let (_tmp, path) = setup_temp_dir();
        let mut app = App::new(path.clone()).unwrap();

        let file_idx = app
            .left
            .entries
            .iter()
            .position(|e| e.name == "file_a.txt")
            .unwrap();
        app.left.selected = file_idx;
        app.delete_selected().unwrap();

        assert!(!path.join("file_a.txt").exists());
    }

    #[test]
    fn file_entry_size_display() {
        let small = FileEntry {
            name: "a".into(),
            path: PathBuf::new(),
            is_dir: false,
            size: 500,
            modified: None,
        };
        assert_eq!(small.size_display(), "500 B");

        let kb = FileEntry {
            size: 2048,
            ..small.clone()
        };
        assert!(kb.size_display().contains("KB"));

        let dir = FileEntry {
            is_dir: true,
            ..small
        };
        assert_eq!(dir.size_display(), "<DIR>");
    }

    #[test]
    fn copy_dir_recursive_works() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src_dir");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("inner.txt"), "data").unwrap();
        fs::create_dir(src.join("nested")).unwrap();
        fs::write(src.join("nested").join("deep.txt"), "deep").unwrap();

        let dest = tmp.path().join("dest_dir");
        copy_dir_recursive(&src, &dest).unwrap();

        assert!(dest.join("inner.txt").exists());
        assert!(dest.join("nested").join("deep.txt").exists());
    }

    #[test]
    fn executable_detection() {
        assert!(super::is_executable("test.exe"));
        assert!(super::is_executable("build.bat"));
        assert!(super::is_executable("run.cmd"));
        assert!(super::is_executable("script.ps1"));
        assert!(super::is_executable("hello.py"));
        assert!(super::is_executable("tool.sh"));
        assert!(!super::is_executable("readme.md"));
        assert!(!super::is_executable("data.json"));
        assert!(!super::is_executable("noext"));
    }

    #[test]
    fn run_dialog_opens_for_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("test.exe");
        fs::write(&exe, "fake").unwrap();
        let mut app = App::new(tmp.path().to_path_buf()).unwrap();
        // Find the exe in entries
        let idx = app.left.entries.iter().position(|e| e.name == "test.exe");
        assert!(idx.is_some());
        app.left.selected = idx.unwrap();
        app.enter_dir().unwrap();
        assert!(app.run_dialog.is_some());
        assert_eq!(app.run_dialog.as_ref().unwrap().file_name, "test.exe");
    }

    #[test]
    fn run_dialog_input_and_cancel() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("test.exe");
        fs::write(&exe, "fake").unwrap();
        let mut app = App::new(tmp.path().to_path_buf()).unwrap();
        app.left.selected = app
            .left
            .entries
            .iter()
            .position(|e| e.name == "test.exe")
            .unwrap();
        app.enter_dir().unwrap();

        app.run_dialog_input('a');
        app.run_dialog_input('r');
        app.run_dialog_input('g');
        assert_eq!(app.run_dialog.as_ref().unwrap().args_input, "arg");

        app.run_dialog_backspace();
        assert_eq!(app.run_dialog.as_ref().unwrap().args_input, "ar");

        app.run_dialog_cancel();
        assert!(app.run_dialog.is_none());
    }

    #[test]
    fn enter_on_non_executable_shows_status() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("data.bin"), b"bin\x00data").unwrap();
        let mut app = App::new(tmp.path().to_path_buf()).unwrap();
        let idx = app
            .left
            .entries
            .iter()
            .position(|e| e.name == "data.bin")
            .unwrap();
        app.left.selected = idx;
        app.enter_dir().unwrap();
        assert!(app.run_dialog.is_none());
        assert!(app.status_msg.as_ref().unwrap().contains("Binary file"));
    }

    #[test]
    fn binary_detection_null_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("data.bin");
        fs::write(&bin, b"some\x00binary\x00data").unwrap();
        assert!(super::is_binary_file(&bin), "should detect null bytes as binary");
    }

    #[test]
    fn binary_detection_text_file() {
        let tmp = tempfile::tempdir().unwrap();
        let txt = tmp.path().join("readme.md");
        fs::write(&txt, b"# Hello\nThis is plain text.\n").unwrap();
        assert!(!super::is_binary_file(&txt), "plain text should not be binary");
    }

    #[test]
    fn binary_detection_missing_file() {
        let path = std::path::Path::new("/nonexistent/file.bin");
        assert!(!super::is_binary_file(path));
    }

    #[test]
    fn enter_on_text_file_sets_open_in_ycode() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("notes.txt"), "hello world").unwrap();
        let mut app = App::new(tmp.path().to_path_buf()).unwrap();
        let idx = app.left.entries.iter().position(|e| e.name == "notes.txt").unwrap();
        app.left.selected = idx;
        app.enter_dir().unwrap();
        assert!(app.open_in_ycode.is_some());
        assert!(app.run_dialog.is_none());
        assert!(app.status_msg.is_none());
    }

    #[test]
    fn enter_on_binary_file_shows_status() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("data.bin"), b"bin\x00data").unwrap();
        let mut app = App::new(tmp.path().to_path_buf()).unwrap();
        let idx = app.left.entries.iter().position(|e| e.name == "data.bin").unwrap();
        app.left.selected = idx;
        app.enter_dir().unwrap();
        assert!(app.open_in_ycode.is_none());
        assert!(app.status_msg.as_ref().unwrap().contains("Binary file"));
    }
}
