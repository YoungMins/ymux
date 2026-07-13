//! VS Code-style file tree sidebar. Toggled with Ctrl+B in `App`,
//! navigated with arrow keys, Enter expands a directory or signals the
//! caller to open a file. Hidden files (dotfiles) are skipped.
//!
//! The tree is a flat `Vec<TreeEntry>` where each entry carries its own
//! `depth`. Expanding a directory at index `i` splices its children
//! (depth = parent + 1) immediately after `i`; collapsing drains the
//! contiguous run of entries with greater depth.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Max visible characters of an entry's name. Longer names are truncated
/// with an ellipsis so the sidebar column doesn't blow up its width.
const MAX_NAME_LEN: usize = 60;

pub struct Sidebar {
    pub root: PathBuf,
    pub entries: Vec<TreeEntry>,
    pub selected: usize,
    pub scroll: usize,
    /// When false (default), dotfiles/dotdirs are hidden. The user toggles
    /// this with the `.` key while the sidebar has focus.
    pub show_hidden: bool,
}

#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// True only for the synthetic `..` row at the top of the tree. Distinct
    /// from a regular subdirectory because Enter on it re-roots the sidebar
    /// rather than expanding in place.
    pub is_parent_link: bool,
}

impl Sidebar {
    pub fn new(root: PathBuf) -> Self {
        let entries = build_root_entries(&root, false);
        Self {
            root,
            entries,
            selected: 0,
            scroll: 0,
            show_hidden: false,
        }
    }

    /// Re-root the sidebar to the parent of the current root. After re-root
    /// the selection lands on the row representing the directory we came
    /// from when it's visible, otherwise on the `..` row (if any) or the
    /// first entry — never out of bounds.
    pub fn re_root_to_parent(&mut self) {
        let Some(parent) = self.root.parent() else {
            return;
        };
        let parent = parent.to_path_buf();
        let old_root_name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().to_string());
        self.root = parent;
        self.entries = build_root_entries(&self.root, self.show_hidden);
        self.scroll = 0;
        self.selected = old_root_name
            .and_then(|name| {
                self.entries
                    .iter()
                    .position(|e| !e.is_parent_link && e.name == name)
            })
            .unwrap_or(0);
    }

    /// Toggle visibility of dotfiles/dotdirs. Rebuilds the entry list
    /// from scratch but preserves every previously-expanded subdirectory
    /// (recursively) and tries to keep the cursor on the same entry by
    /// path. Falls back to entry 0 when the previously-selected path is
    /// no longer visible (e.g. it was a dotfile and the user just hid
    /// hidden entries).
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let expanded_paths: HashSet<PathBuf> = self
            .entries
            .iter()
            .filter(|e| e.is_dir && !e.is_parent_link && e.expanded)
            .map(|e| e.path.clone())
            .collect();
        let selected_path = self.entries.get(self.selected).map(|e| e.path.clone());

        let mut entries: Vec<TreeEntry> = Vec::new();
        if let Some(parent) = self.root.parent() {
            entries.push(TreeEntry {
                path: parent.to_path_buf(),
                name: "..".to_string(),
                depth: 0,
                is_dir: true,
                expanded: false,
                is_parent_link: true,
            });
        }
        entries.extend(build_expanded_subtree(
            &self.root,
            0,
            self.show_hidden,
            &expanded_paths,
        ));
        self.entries = entries;

        self.selected = selected_path
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .unwrap_or(0);
        self.scroll = 0;
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
        // The `..` row looks like a directory but doesn't expand — it
        // re-roots. Callers handle that via `re_root_to_parent`; for the
        // generic toggle path we treat it as a no-op.
        if entry.is_parent_link {
            return false;
        }
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
        let children = read_children(&path, depth + 1, self.show_hidden);
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

/// Compose the entries shown when `root` is the displayed top: an optional
/// `..` parent-link row first, then the alphabetically-sorted child listing.
fn build_root_entries(root: &Path, show_hidden: bool) -> Vec<TreeEntry> {
    let mut entries: Vec<TreeEntry> = Vec::new();
    if let Some(parent) = root.parent() {
        entries.push(TreeEntry {
            path: parent.to_path_buf(),
            name: "..".to_string(),
            depth: 0,
            is_dir: true,
            expanded: false,
            is_parent_link: true,
        });
    }
    entries.extend(read_children(root, 0, show_hidden));
    entries
}

/// Build the children of `root` plus, recursively, the children of any
/// previously-expanded subdirectory (path in `expanded_paths`). Used to
/// rebuild the flat tree after toggling `show_hidden` so the user's
/// expansion state survives the visibility flip.
fn build_expanded_subtree(
    root: &Path,
    depth: usize,
    show_hidden: bool,
    expanded_paths: &HashSet<PathBuf>,
) -> Vec<TreeEntry> {
    let mut entries = read_children(root, depth, show_hidden);
    let mut i = 0;
    while i < entries.len() {
        if entries[i].is_dir && expanded_paths.contains(&entries[i].path) {
            entries[i].expanded = true;
            let path = entries[i].path.clone();
            let children = build_expanded_subtree(&path, depth + 1, show_hidden, expanded_paths);
            let len = children.len();
            for (offset, child) in children.into_iter().enumerate() {
                entries.insert(i + 1 + offset, child);
            }
            i += len + 1;
        } else {
            i += 1;
        }
    }
    entries
}

fn read_children(dir: &Path, depth: usize, show_hidden: bool) -> Vec<TreeEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<(PathBuf, bool, String)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
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
            is_parent_link: false,
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
        // `..` parent-link row prepended (TempDir always has a parent),
        // then dirs alphabetical, then files alphabetical (dotfiles hidden).
        let names: Vec<&str> = sb.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["..", "subdir", "a.rs", "b.md"]);
        assert!(sb.entries[0].is_parent_link);
        assert!(sb.entries.iter().skip(1).all(|e| !e.is_parent_link));
    }

    #[test]
    fn expand_and_collapse_round_trip() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let mut sb = Sidebar::new(td.path().to_path_buf());
        // 4 = `..` + subdir + 2 files. Move to subdir before toggling.
        assert_eq!(sb.entries.len(), 4);
        sb.move_down();
        assert_eq!(sb.entries[sb.selected].name, "subdir");
        assert!(sb.toggle_expand_selected());
        assert_eq!(sb.entries.len(), 5); // `..` + subdir + inner.txt + 2 files
        assert_eq!(sb.entries[2].name, "inner.txt");
        assert_eq!(sb.entries[2].depth, 1);
        // Collapse.
        assert!(sb.toggle_expand_selected());
        assert_eq!(sb.entries.len(), 4);
    }

    #[test]
    fn toggle_on_file_returns_false() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let mut sb = Sidebar::new(td.path().to_path_buf());
        // Skip `..` (idx 0) and subdir (idx 1), land on a.rs (idx 2).
        sb.move_down();
        sb.move_down();
        assert_eq!(sb.entries[sb.selected].name, "a.rs");
        assert!(!sb.entries[sb.selected].is_dir);
        assert!(!sb.toggle_expand_selected());
    }

    #[test]
    fn toggle_on_parent_link_returns_false() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let mut sb = Sidebar::new(td.path().to_path_buf());
        assert_eq!(sb.selected, 0);
        assert!(sb.entries[0].is_parent_link);
        // toggle is a no-op on the parent link — the App handles `..`
        // via `re_root_to_parent` instead.
        assert!(!sb.toggle_expand_selected());
        assert_eq!(sb.entries.len(), 4);
    }

    #[test]
    fn toggle_hidden_reveals_and_hides_dotfiles() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let mut sb = Sidebar::new(td.path().to_path_buf());
        // Default: dotfiles hidden.
        let names: Vec<&str> = sb.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.iter().any(|n| *n == ".hidden"));

        sb.toggle_hidden();
        assert!(sb.show_hidden);
        let names: Vec<&str> = sb.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.iter().any(|n| *n == ".hidden"));

        sb.toggle_hidden();
        assert!(!sb.show_hidden);
        let names: Vec<&str> = sb.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.iter().any(|n| *n == ".hidden"));
    }

    #[test]
    fn toggle_hidden_preserves_subdir_expansion() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let mut sb = Sidebar::new(td.path().to_path_buf());
        // Expand subdir so inner.txt is visible.
        sb.move_down(); // past `..`
        assert_eq!(sb.entries[sb.selected].name, "subdir");
        assert!(sb.toggle_expand_selected());
        assert!(sb.entries.iter().any(|e| e.name == "inner.txt"));

        // Toggling hidden visibility must not collapse the subdir.
        sb.toggle_hidden();
        assert!(sb.entries.iter().any(|e| e.name == "inner.txt"));
        sb.toggle_hidden();
        assert!(sb.entries.iter().any(|e| e.name == "inner.txt"));
    }

    #[test]
    fn re_root_to_parent_changes_root_and_selects_old_root_dir() {
        let td = TempDir::new().unwrap();
        make_tree(&td);
        let inner = td.path().join("subdir");
        let mut sb = Sidebar::new(inner.clone());
        assert_eq!(sb.root, inner);
        sb.re_root_to_parent();
        assert_eq!(sb.root, td.path());
        // After re-rooting, the row representing the directory we came
        // from (`subdir`) should be selected so the user can navigate
        // back in if they want.
        assert_eq!(sb.entries[sb.selected].name, "subdir");
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
