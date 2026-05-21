//! VS Code-style file tree sidebar. Toggled with Ctrl+B in `App`,
//! navigated with arrow keys, Enter expands a directory or signals the
//! caller to open a file. Hidden files (dotfiles) are skipped.
//!
//! The tree is a flat `Vec<TreeEntry>` where each entry carries its own
//! `depth`. Expanding a directory at index `i` splices its children
//! (depth = parent + 1) immediately after `i`; collapsing drains the
//! contiguous run of entries with greater depth.

use std::path::{Path, PathBuf};

/// Max visible characters of an entry's name. Longer names are truncated
/// with an ellipsis so the sidebar column doesn't blow up its width.
const MAX_NAME_LEN: usize = 60;

pub struct Sidebar {
    pub root: PathBuf,
    pub entries: Vec<TreeEntry>,
    pub selected: usize,
    pub scroll: usize,
}

#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

impl Sidebar {
    pub fn new(root: PathBuf) -> Self {
        let entries = read_children(&root, 0);
        Self {
            root,
            entries,
            selected: 0,
            scroll: 0,
        }
    }

    pub fn selected_entry(&self) -> Option<&TreeEntry> {
        self.entries.get(self.selected)
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    pub fn page_up(&mut self, n: usize) {
        self.selected = self.selected.saturating_sub(n);
    }

    pub fn page_down(&mut self, n: usize) {
        self.selected = (self.selected + n).min(self.entries.len().saturating_sub(1));
    }

    pub fn move_home(&mut self) {
        self.selected = 0;
    }

    pub fn move_end(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
    }

    /// Expand or collapse the directory at the selected entry. Returns
    /// false if the selected entry isn't a directory — the caller should
    /// then treat Enter as "open this file".
    pub fn toggle_expand_selected(&mut self) -> bool {
        let i = self.selected;
        let Some(entry) = self.entries.get(i) else {
            return false;
        };
        if !entry.is_dir {
            return false;
        }
        if entry.expanded {
            self.collapse_at(i);
        } else {
            self.expand_at(i);
        }
        true
    }

    fn expand_at(&mut self, i: usize) {
        let (path, depth) = {
            let entry = &mut self.entries[i];
            if entry.expanded {
                return;
            }
            entry.expanded = true;
            (entry.path.clone(), entry.depth)
        };
        let children = read_children(&path, depth + 1);
        for (offset, child) in children.into_iter().enumerate() {
            self.entries.insert(i + 1 + offset, child);
        }
    }

    fn collapse_at(&mut self, i: usize) {
        let depth = self.entries[i].depth;
        self.entries[i].expanded = false;
        let mut j = i + 1;
        while j < self.entries.len() && self.entries[j].depth > depth {
            j += 1;
        }
        self.entries.drain(i + 1..j);
    }

    /// Keep `selected` inside the visible window of `viewport_height` rows.
    pub fn ensure_scroll(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + viewport_height {
            self.scroll = self.selected - viewport_height + 1;
        }
    }
}

fn read_children(dir: &Path, depth: usize) -> Vec<TreeEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<(PathBuf, bool, String)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // Skip dotfiles — the user can configure this later if needed.
            if name.starts_with('.') {
                return None;
            }
            let path = e.path();
            let is_dir = path.is_dir();
            let display = if name.chars().count() > MAX_NAME_LEN {
                let mut s: String = name.chars().take(MAX_NAME_LEN).collect();
                s.push('…');
                s
            } else {
                name
            };
            Some((path, is_dir, display))
        })
        .collect();
    items.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.2.to_lowercase().cmp(&b.2.to_lowercase()),
    });
    items
        .into_iter()
        .map(|(path, is_dir, name)| TreeEntry {
            path,
            name,
            depth,
            is_dir,
            expanded: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_tree(td: &TempDir) {
        let root = td.path();
        fs::create_dir(root.join("subdir")).unwrap();
        fs::write(root.join("subdir/inner.txt"), "x").unwrap();
        fs::write(root.join("a.rs"), "fn main() {}").unwrap();
        fs::write(root.join("b.md"), "# hi").unwrap();
        fs::write(root.join(".hidden"), "secret").unwrap();
    }

    #[test]
    fn lists_top_level_dirs_first_then_files_alphabetical() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let sb = Sidebar::new(td.path().to_path_buf());
        let names: Vec<&str> = sb.entries.iter().map(|e| e.name.as_str()).collect();
        // Dirs first, then files alphabetical, dotfiles hidden.
        assert_eq!(names, vec!["subdir", "a.rs", "b.md"]);
    }

    #[test]
    fn expand_and_collapse_round_trip() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let mut sb = Sidebar::new(td.path().to_path_buf());
        assert_eq!(sb.entries.len(), 3);
        // Select subdir (index 0) and expand.
        assert_eq!(sb.selected, 0);
        assert!(sb.toggle_expand_selected());
        assert_eq!(sb.entries.len(), 4); // subdir + inner.txt + 2 files
        assert_eq!(sb.entries[1].name, "inner.txt");
        assert_eq!(sb.entries[1].depth, 1);
        // Collapse.
        assert!(sb.toggle_expand_selected());
        assert_eq!(sb.entries.len(), 3);
    }

    #[test]
    fn toggle_on_file_returns_false() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let mut sb = Sidebar::new(td.path().to_path_buf());
        sb.move_down(); // select a.rs
        assert!(!sb.entries[sb.selected].is_dir);
        assert!(!sb.toggle_expand_selected());
    }

    #[test]
    fn navigation_stays_in_bounds() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let mut sb = Sidebar::new(td.path().to_path_buf());
        sb.move_up();
        assert_eq!(sb.selected, 0);
        for _ in 0..20 {
            sb.move_down();
        }
        assert_eq!(sb.selected, sb.entries.len() - 1);
    }

    #[test]
    fn ensure_scroll_keeps_selection_visible() {
        let td = TempDir::new().unwrap();
        for i in 0..30 {
            fs::write(td.path().join(format!("f{i:02}.txt")), "x").unwrap();
        }
        let mut sb = Sidebar::new(td.path().to_path_buf());
        sb.selected = 25;
        sb.ensure_scroll(10);
        assert!(sb.scroll <= 25);
        assert!(sb.scroll + 10 > 25);
    }
}
