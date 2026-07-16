//! Thin wrappers over the `git` binary for worktree operations. We shell out
//! rather than link libgit2 to keep the dependency surface minimal and match
//! whatever git the user already has on PATH. Deliberately free of any Tauri
//! dependency (only `std` + `crate::error` + `serde::Serialize`) so it
//! compiles and its pure-parsing tests run under
//! `cargo test --no-default-features --lib -p ymux` on Linux CI, mirroring
//! `src-tauri/src/scrollback.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{YmuxError, YmuxResult};

/// A single entry from `git worktree list --porcelain`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorktreeEntry {
    pub path: String,
    pub branch: String,
}

/// Run a git subcommand in `cwd`, returning stdout on success or a
/// `YmuxError::Git` wrapping stderr on failure.
fn run_git(cwd: &Path, args: &[&str]) -> YmuxResult<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(YmuxError::Io)?;
    if !out.status.success() {
        return Err(YmuxError::Git(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether `cwd` is inside a git working tree.
pub fn is_git_repo(cwd: &Path) -> bool {
    Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve the top-level directory of the repo containing `cwd`.
pub fn repo_root(cwd: &Path) -> YmuxResult<PathBuf> {
    let out = run_git(cwd, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

/// Add a worktree at `path`. Attaches to `branch` if it already exists,
/// otherwise creates it (`git worktree add -b`).
pub fn worktree_add(repo: &Path, branch: &str, path: &Path) -> YmuxResult<()> {
    let path_s = path.to_string_lossy();
    let branch_exists = run_git(repo, &["rev-parse", "--verify", "--quiet", branch]).is_ok();
    if branch_exists {
        run_git(repo, &["worktree", "add", &path_s, branch])?;
    } else {
        run_git(repo, &["worktree", "add", "-b", branch, &path_s])?;
    }
    Ok(())
}

/// Remove the worktree at `path`, optionally forcing removal even with
/// uncommitted changes.
pub fn worktree_remove(path: &Path, force: bool) -> YmuxResult<()> {
    let path_s = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_s);
    // `git worktree remove` can run from anywhere inside the repo; use the
    // worktree's own parent as cwd fallback.
    let cwd = path.parent().unwrap_or(path);
    run_git(cwd, &args)?;
    Ok(())
}

/// List all worktrees registered against `repo`.
pub fn worktree_list(repo: &Path) -> YmuxResult<Vec<WorktreeEntry>> {
    let out = run_git(repo, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_porcelain(&out))
}

/// Parse `git worktree list --porcelain` output into entries. Entries with
/// a detached HEAD have no `branch` line and are reported with an empty
/// `branch` field.
pub fn parse_worktree_porcelain(out: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut path: Option<String> = None;
    let mut branch = String::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.to_string());
            branch = String::new();
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
        } else if line.is_empty() {
            if let Some(p) = path.take() {
                entries.push(WorktreeEntry {
                    path: p,
                    branch: std::mem::take(&mut branch),
                });
            }
        }
    }
    if let Some(p) = path.take() {
        entries.push(WorktreeEntry { path: p, branch });
    }
    entries
}

/// Compute the worktree directory for `branch`. An empty `base` defaults to
/// a sibling `.ymux-worktrees` dir next to the repo; a non-empty `base` is
/// used as-is. Branch slashes are flattened to dashes so the result is
/// always a single path component.
pub fn suggested_worktree_path(repo: &Path, branch: &str, base: &str) -> PathBuf {
    let flat = branch.replace('/', "-");
    if base.is_empty() {
        let parent = repo.parent().unwrap_or(repo);
        parent.join(".ymux-worktrees").join(flat)
    } else {
        PathBuf::from(base).join(flat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_porcelain_worktree_list() {
        let out = "\
worktree /home/u/repo
HEAD abc
branch refs/heads/main

worktree /home/u/.ymux-worktrees/agent-1
HEAD def
branch refs/heads/agent/xyz
";
        let list = parse_worktree_porcelain(out);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].path, "/home/u/.ymux-worktrees/agent-1");
        assert_eq!(list[1].branch, "agent/xyz");
    }

    #[test]
    fn suggested_path_uses_default_base_when_empty() {
        let repo = Path::new("/home/u/repo");
        let p = suggested_worktree_path(repo, "agent/xyz", "");
        // sibling `.ymux-worktrees` dir, branch slashes flattened
        assert!(p
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with(".ymux-worktrees/agent-xyz"));
    }

    #[test]
    fn suggested_path_honours_custom_base() {
        let repo = Path::new("/home/u/repo");
        let p = suggested_worktree_path(repo, "feature/a", "/tmp/wt");
        assert!(p
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("/tmp/wt/feature-a"));
    }

    #[test]
    fn parses_porcelain_detached_head_entry() {
        // Detached HEAD worktrees have no `branch` line at all.
        let out = "\
worktree /home/u/repo
HEAD abc

worktree /home/u/.ymux-worktrees/detached
HEAD def
detached
";
        let list = parse_worktree_porcelain(out);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].path, "/home/u/.ymux-worktrees/detached");
        assert_eq!(list[1].branch, "", "detached HEAD entries have no branch");
    }
}
