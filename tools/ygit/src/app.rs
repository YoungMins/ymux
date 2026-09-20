use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Graph,
    Branches,
}

pub struct App {
    pub log_lines: Vec<String>,
    pub branches: Vec<String>,
    pub log_scroll: usize,
    pub branch_idx: usize,
    pub focus: Focus,
    pub status: Option<String>,
}

impl App {
    pub fn new() -> Self {
        let mut app = Self {
            log_lines: Vec::new(),
            branches: Vec::new(),
            log_scroll: 0,
            branch_idx: 0,
            focus: Focus::Graph,
            status: None,
        };
        app.refresh();
        app
    }

    pub fn refresh(&mut self) {
        self.log_lines = run_git_log();
        self.branches = run_git_branches();
        if self.branch_idx >= self.branches.len() && !self.branches.is_empty() {
            self.branch_idx = self.branches.len() - 1;
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Graph => Focus::Branches,
            Focus::Branches => Focus::Graph,
        };
    }

    pub fn scroll_up(&mut self) {
        match self.focus {
            Focus::Graph => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
            }
            Focus::Branches => {
                self.branch_idx = self.branch_idx.saturating_sub(1);
            }
        }
    }

    pub fn scroll_down(&mut self) {
        match self.focus {
            Focus::Graph => {
                let max = self.log_lines.len().saturating_sub(1);
                if self.log_scroll < max {
                    self.log_scroll += 1;
                }
            }
            Focus::Branches => {
                let max = self.branches.len().saturating_sub(1);
                if self.branch_idx < max {
                    self.branch_idx += 1;
                }
            }
        }
    }

    pub fn checkout_selected(&mut self) {
        let Some(raw) = self.branches.get(self.branch_idx) else {
            return;
        };
        let Some(branch) = branch_name(raw) else {
            self.status = Some(format!("Not a branch: {}", raw.trim()));
            return;
        };
        let branch = branch.to_string();

        // `--` ends the revision list, so a branch whose name also matches a
        // file on disk is still read as a branch.
        match Command::new("git")
            .args(["checkout", &branch, "--"])
            .output()
        {
            Ok(out) if out.status.success() => {
                self.status = Some(format!("Checked out: {branch}"));
                self.refresh();
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                self.status = Some(format!("Error: {}", err.trim()));
            }
            Err(e) => {
                self.status = Some(format!("Failed to run git: {e}"));
            }
        }
    }
}

fn run_git_log() -> Vec<String> {
    match Command::new("git")
        .args([
            "log",
            "--graph",
            "--oneline",
            "--all",
            "--decorate",
            "--color=never",
        ])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            vec![format!("git error: {}", err.trim())]
        }
        Err(e) => vec![format!("failed to run git: {e}")],
    }
}

/// The branch name inside one `git branch` line, or `None` when the line
/// names no branch that can be checked out.
///
/// `git branch` writes a two-column marker before every name: `* ` for the
/// branch HEAD is on, `+ ` for one that is checked out in *another*
/// worktree, and two spaces otherwise. It is a fixed-width column, so
/// exactly one marker comes off -- a branch really named `* x` would keep
/// its own `* `, and there is nothing else to strip.
///
/// The pseudo-entries are the other half. In a detached HEAD, `git branch`
/// prints `* (HEAD detached at b013596)` (or `(no branch)` mid-rebase),
/// which is prose, not a ref. Git rejects both spellings outright --
/// `fatal: invalid reference: + 워크트리브랜치` for an unstripped marker --
/// so neither must reach `git checkout`.
///
/// A `+ ` branch is returned, not rejected: `git checkout` refuses it with
/// "already used by worktree at <path>", which tells the user something.
///
/// Branch names themselves need no further care. Git rejects a ref name
/// containing a space (`fatal: 'space branch name' is not a valid branch
/// name`), and `git branch` prints non-ASCII names as raw UTF-8 -- verified
/// against git 2.52 with a Korean branch -- so there is no C-quoting to
/// undo and no embedded whitespace to split on.
fn branch_name(raw: &str) -> Option<&str> {
    let name = raw
        .strip_prefix("* ")
        .or_else(|| raw.strip_prefix("+ "))
        .or_else(|| raw.strip_prefix("  "))
        .unwrap_or(raw)
        .trim();
    if name.is_empty() || name.starts_with('(') {
        return None;
    }
    Some(name)
}

fn run_git_branches() -> Vec<String> {
    match Command::new("git").args(["branch"]).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_does_not_panic() {
        let _app = App::new();
    }

    #[test]
    fn refresh_does_not_panic() {
        let mut app = App::new();
        app.refresh();
    }

    #[test]
    fn focus_toggles() {
        let mut app = App::new();
        assert_eq!(app.focus, Focus::Graph);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Branches);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Graph);
    }

    #[test]
    fn log_scroll_up_at_zero_stays_zero() {
        let mut app = App::new();
        app.log_scroll = 0;
        app.scroll_up();
        assert_eq!(app.log_scroll, 0);
    }

    #[test]
    fn branch_scroll_up_at_zero_stays_zero() {
        let mut app = App::new();
        app.focus = Focus::Branches;
        app.branch_idx = 0;
        app.scroll_up();
        assert_eq!(app.branch_idx, 0);
    }

    #[test]
    fn log_scroll_down_bounded() {
        let mut app = App::new();
        let max = app.log_lines.len().saturating_sub(1);
        for _ in 0..max + 5 {
            app.scroll_down();
        }
        assert!(app.log_scroll <= max);
    }

    #[test]
    fn branch_scroll_down_bounded() {
        let mut app = App::new();
        app.focus = Focus::Branches;
        let max = app.branches.len().saturating_sub(1);
        for _ in 0..max + 5 {
            app.scroll_down();
        }
        assert!(app.branch_idx <= max);
    }

    #[test]
    fn checkout_noop_on_empty_branches() {
        let mut app = App::new();
        app.branches.clear();
        app.checkout_selected(); // must not panic
    }

    /// Captured from real git 2.52 in a repo with a Korean branch checked
    /// out and a second Korean branch held by a linked worktree.
    const REAL_BRANCH_OUTPUT: &[&str] = &["  master", "* 기능/한글브랜치", "+ 워크트리브랜치"];

    #[test]
    fn branch_name_strips_every_marker_git_writes() {
        let names: Vec<_> = REAL_BRANCH_OUTPUT
            .iter()
            .map(|line| branch_name(line))
            .collect();
        assert_eq!(
            names,
            vec![
                Some("master"),
                Some("기능/한글브랜치"),
                // RED before the fix: `+ ` was left on, and git answered
                // `fatal: invalid reference: + 워크트리브랜치`.
                Some("워크트리브랜치"),
            ]
        );
    }

    /// A detached HEAD gives `git branch` nothing to name, so it prints
    /// prose in parentheses. Checking that out is not a thing.
    #[test]
    fn branch_name_rejects_the_detached_head_pseudo_entry() {
        assert_eq!(branch_name("* (HEAD detached at b013596)"), None);
        assert_eq!(branch_name("* (HEAD detached from v1.2)"), None);
        assert_eq!(branch_name("* (no branch, rebasing main)"), None);
        assert_eq!(branch_name(""), None);
        assert_eq!(branch_name("   "), None);
    }

    /// Exactly one marker column comes off. Git permits `*` inside a branch
    /// name, so a repeated strip would eat a real character.
    #[test]
    fn branch_name_strips_one_marker_not_a_run() {
        assert_eq!(branch_name("* * odd"), Some("* odd"));
    }

    #[test]
    fn checkout_reports_a_pseudo_entry_instead_of_running_git() {
        let mut app = App::new();
        app.branches = vec!["* (HEAD detached at b013596)".to_string()];
        app.branch_idx = 0;
        app.checkout_selected();
        let status = app.status.expect("a pseudo-entry must explain itself");
        assert!(
            status.starts_with("Not a branch"),
            "unexpected status: {status}"
        );
    }
}
