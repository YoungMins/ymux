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

/// Add a worktree at `path`. Attaches to `branch` if it already exists as a
/// local branch, otherwise creates it (`git worktree add -b`).
pub fn worktree_add(repo: &Path, branch: &str, path: &Path) -> YmuxResult<()> {
    let path_s = path.to_string_lossy();
    // Probe the local-branch namespace specifically. `git rev-parse --verify`
    // on an unqualified name also resolves tags, remote-tracking refs, and
    // SHA prefixes -- if `branch` collides with any of those, the "attach"
    // branch below runs `worktree add <path> <branch>` (no `-b`), which git
    // checks out **detached** rather than attaching to a local branch,
    // silently contradicting this function's contract. `refs/heads/<branch>`
    // only matches an actual local branch.
    let heads_ref = format!("refs/heads/{branch}");
    let branch_exists =
        run_git(repo, &["show-ref", "--verify", "--quiet", "--", &heads_ref]).is_ok();
    if branch_exists {
        run_git(repo, &["worktree", "add", "--", &path_s, branch])?;
    } else {
        run_git(repo, &["worktree", "add", "-b", branch, "--", &path_s])?;
    }
    Ok(())
}

/// Remove the worktree at `path`, optionally forcing removal even with
/// uncommitted changes.
///
/// `git worktree remove` must be run from a directory connected to the
/// repository (the main worktree or another linked worktree) -- it is not
/// enough to use `path`'s parent directory as cwd. This module's own
/// `suggested_worktree_path` default layout places worktrees as *siblings*
/// of the repo (`<repo-parent>/.ymux-worktrees/<branch>`), so
/// `path.parent()` there is `.ymux-worktrees`, which has no `.git` and is
/// not a git directory itself -- `git worktree remove` invoked with that
/// cwd fails unconditionally with `fatal: not a git repository`.
///
/// Deliberately takes no `repo` parameter (the eventual caller only knows
/// the pane's worktree path, not the origin repo). Instead, `path` itself is
/// a valid git worktree -- its `.git` file always points back at the main
/// repo -- so we resolve the main worktree from `path` via
/// `git rev-parse --git-common-dir` (whose parent directory is the main
/// worktree root for both linked and main worktrees) and run the removal
/// from there. Running the removal with cwd *inside* the worktree being
/// removed also fails ("not a working tree" / cannot delete the cwd on
/// Windows), which is the other reason cwd must be the main worktree, not
/// `path` itself.
pub fn worktree_remove(path: &Path, force: bool) -> YmuxResult<()> {
    let path_s = path.to_string_lossy();

    let common_dir = run_git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common_dir = PathBuf::from(common_dir.trim());
    let main_worktree = common_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| common_dir.clone());

    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push("--");
    args.push(&path_s);
    run_git(&main_worktree, &args)?;
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
///
/// Note: this flattening is lossy and can collide -- `"feature/a-b"` and
/// `"feature-a/b"` (and `"feature/a/b"`) all flatten to `"feature-a-b"`. Not
/// currently detected or resolved; a caller wiring up real branch names
/// (e.g. Task 9/12's UI) should either reject branch names with literal `-`
/// adjacent to where a `/` would be, or hash-suffix on collision.
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

    // --- Real-git integration tests below. These shell out to an actual
    // `git` binary rather than mocking, because the bugs they guard against
    // (worktree_remove's cwd derivation, worktree_add's branch-existence
    // probe) only reproduce against real git subcommand semantics. Skipped
    // gracefully (not failed) when `git` isn't on PATH, mirroring the
    // `#[cfg(unix)]` skip-if-missing style in `pty/session.rs`'s test.

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Create a fresh temp dir, `git init` it, configure a commit identity,
    /// and commit one file. Returns the repo path. Caller must remove it.
    fn init_test_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ymux_git_test_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp repo dir");

        let run = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .expect("run git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("f.txt"), "hello\n").expect("write file");
        run(&["add", "f.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    fn cleanup_dir(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn worktree_add_then_remove_round_trip() {
        if !git_available() {
            eprintln!("skipping worktree_add_then_remove_round_trip: git not on PATH");
            return;
        }
        let repo = init_test_repo("roundtrip");

        // Use the module's own default sibling layout -- this is exactly the
        // layout that made the pre-fix `worktree_remove` fail
        // unconditionally (cwd derived from `path.parent()` landed in
        // `.ymux-worktrees`, which has no `.git`).
        let wt_path = suggested_worktree_path(&repo, "agent/x", "");

        worktree_add(&repo, "agent/x", &wt_path).expect("worktree_add should succeed");

        let list = worktree_list(&repo).expect("worktree_list should succeed");
        let entry = list
            .iter()
            .find(|e| e.branch == "agent/x")
            .expect("worktree_add must attach to branch 'agent/x', not leave it detached");
        assert!(
            Path::new(&entry.path)
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("agent-x"),
            "entry path should be the suggested worktree path, got {}",
            entry.path
        );

        // RED (pre-fix): this call failed unconditionally with
        // "fatal: not a git repository" because `path.parent()` for the
        // sibling `.ymux-worktrees/agent-x` layout is `.ymux-worktrees`,
        // which is not a git directory.
        // GREEN (post-fix): resolves the main worktree from `path` itself
        // via `--git-common-dir` and runs the removal from there.
        worktree_remove(&wt_path, false).expect("worktree_remove should succeed (GREEN)");

        let list_after = worktree_list(&repo).expect("worktree_list should succeed");
        assert!(
            !list_after.iter().any(|e| e.branch == "agent/x"),
            "worktree should be gone from worktree_list after removal"
        );

        cleanup_dir(&repo);
        // `git worktree remove` already deleted the leaf worktree dir itself;
        // the shared sibling `.ymux-worktrees` parent is common across temp
        // repos (its parent is the OS temp root) and possibly other
        // concurrently-running tests, so only reclaim it if it's now empty --
        // never `remove_dir_all` a directory other tests may still be using.
        if let Some(parent) = repo.parent() {
            let _ = std::fs::remove_dir(parent.join(".ymux-worktrees"));
        }
    }

    #[test]
    fn worktree_add_tag_collision_attaches_branch_not_detached() {
        if !git_available() {
            eprintln!(
                "skipping worktree_add_tag_collision_attaches_branch_not_detached: git not on PATH"
            );
            return;
        }
        let repo = init_test_repo("tagcollision");

        // Create a *tag* named identically to the branch we're about to
        // request. The pre-fix probe (`rev-parse --verify --quiet <branch>`)
        // resolves this tag and takes the "attach to existing branch" path
        // (no `-b`), which git checks out **detached** since `tagonly` does
        // not actually name a local branch.
        let out = Command::new("git")
            .current_dir(&repo)
            .args(["tag", "tagonly"])
            .output()
            .expect("run git tag");
        assert!(out.status.success(), "git tag failed");

        let wt_path = suggested_worktree_path(&repo, "tagonly", "");

        worktree_add(&repo, "tagonly", &wt_path).expect("worktree_add should succeed");

        let list = worktree_list(&repo).expect("worktree_list should succeed");
        let entry = list
            .iter()
            .find(|e| {
                Path::new(&e.path)
                    .to_string_lossy()
                    .replace('\\', "/")
                    .ends_with("tagonly")
            })
            .expect("worktree entry for the requested path should exist");

        // RED (pre-fix): entry.branch == "" (detached), because the
        // unqualified `rev-parse --verify` probe matched the tag and the
        // code ran `worktree add <path> <branch>` without `-b`, so git
        // checked out detached rather than attaching to (or creating) a
        // local branch.
        // GREEN (post-fix): the `refs/heads/tagonly` probe correctly reports
        // "no such local branch", so the code takes the `-b` path and
        // creates+attaches a real local branch named `tagonly`.
        assert_eq!(
            entry.branch, "tagonly",
            "expected worktree_add to create/attach local branch 'tagonly', not leave it detached (branch={:?})",
            entry.branch
        );

        worktree_remove(&wt_path, false).expect("cleanup: worktree_remove should succeed");
        cleanup_dir(&repo);
        if let Some(parent) = repo.parent() {
            let _ = std::fs::remove_dir(parent.join(".ymux-worktrees"));
        }
    }
}
