//! Fan-out "keep this lane": merge the winner into the base branch, then
//! remove the other lanes' worktrees and branches.
//!
//! One entry point, [`keep_lane`]: it checks every precondition first (and with
//! `dry_run` stops there, so the UI can close panes knowing the real run will
//! be accepted), merges with `--no-ff`, and cleans up only after a successful
//! merge. Failures of the user's situation (dirty base, conflicts, ...) are
//! values in the [`KeepReport`], coded for the UI to translate; only a
//! malformed request is an `Err`.
//!
//! `--no-ff` rather than fast-forward: the lane stays visible in history as
//! one merge commit, and the merged commit is always a fresh, nameable sha.
//!
//! A branch is deleted (`git branch -D`, the losers are unmerged by design)
//! only if the fan-out's own record on it -- `branch.<b>.ymuxFanout`, written
//! by [`super::lane_meta_set`] when the lane was created -- names this run.
//! A name pattern alone never qualifies.

use std::path::{Path, PathBuf};

use super::{
    git_command, parse_status_z, run_git, run_git_untimed, valid_run_id, validate_ref_name,
    work_status, worktree_list, OutputTimeout, WorktreeEntry,
};
use crate::error::{YmuxError, YmuxResult};

/// What the caller asks for. `others` are the lane branches to clean up
/// (empty = merge only).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepRequest {
    /// Any path inside the repository; the base checkout is its main worktree.
    pub repo: String,
    /// The branch that must still be checked out in the base checkout.
    pub base_branch: String,
    pub run_id: String,
    pub keep_branch: String,
    #[serde(default)]
    pub others: Vec<String>,
    /// Remove the kept lane's worktree after merging.
    #[serde(default)]
    pub remove_kept_worktree: bool,
    /// Force-remove losing worktrees that have uncommitted changes.
    #[serde(default)]
    pub discard_dirty_others: bool,
    /// Check preconditions only; change nothing. Also looks for merge
    /// conflicts (`git merge-tree`, which touches neither tree nor index).
    #[serde(default)]
    pub dry_run: bool,
    /// Skip the merge and only run the cleanup. Refused (`not-merged`) unless
    /// the kept branch is already an ancestor of the base branch's HEAD.
    #[serde(default)]
    pub cleanup_only: bool,
}

/// A lane the cleanup left alone, with why.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Skipped {
    pub branch: String,
    pub path: String,
    /// `dirty` | `not-fanout` | `protected`.
    pub reason: String,
    /// Changed paths for `dirty`.
    pub files: Vec<String>,
}

/// A step that failed.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct StepError {
    /// `remove-worktree` | `delete-branch` | `remove-kept-worktree`.
    pub step: String,
    pub branch: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Removed {
    pub branch: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct KeepReport {
    /// `ok` (dry run: checks passed) | `would-conflict` (dry run: the merge
    /// would conflict, `files` lists them) | `merged` | `cleaned`
    /// (`cleanup_only` done) | `refused` | `conflict` | `failed`.
    pub outcome: String,
    /// For `refused`: `base-branch` | `base-dirty` |
    /// `lane-dirty` | `no-commits` | `not-fanout` | `not-merged`.
    pub reason: String,
    /// Changed files (`base-dirty`, `lane-dirty`) or conflicting files.
    pub files: Vec<String>,
    /// git's own message for `failed`, or the abort failure for `conflict`.
    pub detail: String,
    /// `conflict` only: the abort failed and the base is still mid-merge.
    pub left_mid_merge: bool,
    pub merged_commit: String,
    pub removed_worktrees: Vec<Removed>,
    pub deleted_branches: Vec<String>,
    pub kept_worktree_removed: bool,
    pub skipped: Vec<Skipped>,
    pub errors: Vec<StepError>,
    /// Dry run only: this git cannot check for conflicts without merging
    /// (`merge-tree --write-tree` needs git 2.38). A warning, not a refusal.
    pub conflict_check_unavailable: bool,
}

impl KeepReport {
    fn refused(reason: &str, files: Vec<String>) -> Self {
        KeepReport {
            outcome: "refused".into(),
            reason: reason.into(),
            files,
            ..Default::default()
        }
    }
}

/// The run id recorded on `branch`, empty when none.
fn recorded_run(repo: &Path, branch: &str) -> String {
    git_command(repo)
        .args(["config", "--get", &format!("branch.{branch}.ymuxFanout")])
        .output_timeout()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Tracked and staged changes (untracked files are not listed).
fn tracked_changes(cwd: &Path) -> YmuxResult<Vec<String>> {
    let out = run_git(
        cwd,
        &["status", "--porcelain=v1", "-z", "--untracked-files=no"],
    )?;
    Ok(parse_status_z(&out).into_iter().map(|e| e.path).collect())
}

/// Commits on `branch` that `base` lacks.
fn count_ahead(cwd: &Path, base: &str, branch: &str) -> YmuxResult<u32> {
    let range = format!("{base}..{branch}");
    let out = run_git(cwd, &["rev-list", "--count", &range, "--"])?;
    out.trim()
        .parse()
        .map_err(|_| YmuxError::Git(format!("unexpected rev-list output: {}", out.trim())))
}

fn conflicted_files(cwd: &Path) -> Vec<String> {
    run_git(cwd, &["diff", "--name-only", "--diff-filter=U", "-z"])
        .map(|o| {
            o.split('\0')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

enum ConflictCheck {
    Clean,
    Conflicts(Vec<String>),
    Unavailable,
}

/// Would merging `keep` into `base` conflict? `git merge-tree --write-tree`
/// computes the merge in the object database only: the working tree and the
/// index are not touched. Exit 0 = clean, 1 = conflicts (listed after the tree
/// id, NUL-separated, up to an empty entry); anything else (an older git
/// rejects the option, unrelated histories, ...) = cannot tell.
fn conflict_check(cwd: &Path, base: &str, keep: &str) -> ConflictCheck {
    let out = git_command(cwd)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "-z",
            base,
            keep,
        ])
        .output();
    let Ok(out) = out else {
        return ConflictCheck::Unavailable;
    };
    match out.status.code() {
        Some(0) => ConflictCheck::Clean,
        Some(1) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let files = text
                .split('\0')
                .skip(1)
                .take_while(|s| !s.is_empty())
                .map(String::from)
                .collect();
            ConflictCheck::Conflicts(files)
        }
        _ => ConflictCheck::Unavailable,
    }
}

/// Is `branch` already contained in `of`?
fn is_ancestor(cwd: &Path, branch: &str, of: &str) -> bool {
    git_command(cwd)
        .args(["merge-base", "--is-ancestor", branch, of])
        .output_timeout()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn merge_in_progress(cwd: &Path) -> bool {
    run_git(cwd, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok()
}

fn entry_for<'a>(list: &'a [WorktreeEntry], branch: &str) -> Option<&'a WorktreeEntry> {
    list.iter().find(|e| !e.bare && e.branch == branch)
}

/// See the module docs.
pub fn keep_lane(req: &KeepRequest) -> YmuxResult<KeepReport> {
    validate_ref_name(&req.base_branch)?;
    validate_ref_name(&req.keep_branch)?;
    for b in &req.others {
        validate_ref_name(b)?;
    }
    if !valid_run_id(&req.run_id) {
        return Err(YmuxError::Git(format!(
            "invalid fan-out run id: {}",
            req.run_id
        )));
    }
    let list = worktree_list(Path::new(&req.repo))?;
    let Some(main) = list.iter().find(|e| e.main) else {
        return Err(YmuxError::Git("no main worktree".into()));
    };
    let base_dir = PathBuf::from(&main.path);

    // The base branch must still be what the base checkout has out.
    if super::current_branch(&base_dir).as_deref() != Some(req.base_branch.as_str()) {
        return Ok(KeepReport::refused("base-branch", vec![]));
    }
    let changes = tracked_changes(&base_dir)?;
    if !changes.is_empty() {
        return Ok(KeepReport::refused("base-dirty", changes));
    }

    // The kept lane: a linked worktree of this run, on a branch the fan-out made.
    // Its worktree may already be gone (closing a lane's terminal offers to
    // remove it); the branch is what gets merged, so that is not an error.
    if req.keep_branch == req.base_branch || recorded_run(&base_dir, &req.keep_branch) != req.run_id
    {
        return Ok(KeepReport::refused("not-fanout", vec![]));
    }
    let keeper_dir = entry_for(&list, &req.keep_branch)
        .filter(|e| !e.main)
        .map(|e| PathBuf::from(&e.path));
    if let Some(dir) = &keeper_dir {
        let dirty: Vec<String> = work_status(dir, false)?
            .changes
            .into_iter()
            .map(|e| e.path)
            .collect();
        if !dirty.is_empty() {
            return Ok(KeepReport::refused("lane-dirty", dirty));
        }
    }
    if req.cleanup_only {
        if !is_ancestor(&base_dir, &req.keep_branch, &req.base_branch) {
            return Ok(KeepReport::refused("not-merged", vec![]));
        }
    } else if count_ahead(&base_dir, &req.base_branch, &req.keep_branch)? == 0 {
        return Ok(KeepReport::refused("no-commits", vec![]));
    }
    if req.dry_run {
        let mut report = KeepReport {
            outcome: "ok".into(),
            ..Default::default()
        };
        if !req.cleanup_only {
            match conflict_check(&base_dir, &req.base_branch, &req.keep_branch) {
                ConflictCheck::Clean => {}
                ConflictCheck::Conflicts(files) => {
                    report.outcome = "would-conflict".into();
                    report.files = files;
                }
                ConflictCheck::Unavailable => report.conflict_check_unavailable = true,
            }
        }
        return Ok(report);
    }

    // Merge. Whatever happens, the base checkout is never left mid-merge.
    let mut report = KeepReport::default();
    if req.cleanup_only {
        report.outcome = "cleaned".into();
    } else if let Err(e) = run_git_untimed(
        &base_dir,
        &["merge", "--no-ff", "--no-edit", "--", &req.keep_branch],
    ) {
        let files = conflicted_files(&base_dir);
        let mut abort_err = String::new();
        if merge_in_progress(&base_dir) {
            if let Err(a) = run_git_untimed(&base_dir, &["merge", "--abort"]) {
                abort_err = a.to_string();
            }
        }
        report.left_mid_merge = !abort_err.is_empty() && merge_in_progress(&base_dir);
        if files.is_empty() {
            report.outcome = "failed".into();
            report.detail = e.to_string();
        } else {
            report.outcome = "conflict".into();
            report.files = files;
            report.detail = abort_err;
        }
        return Ok(report);
    } else {
        report.outcome = "merged".into();
        // The merge commit exists; failing to name it must not fail the command
        // (a retry would then be refused as having no commits to merge).
        report.merged_commit = run_git(&base_dir, &["rev-parse", "HEAD"])
            .map(|h| h.trim().to_string())
            .unwrap_or_default();
    }

    // Losing lanes.
    let mut seen = std::collections::HashSet::new();
    for branch in &req.others {
        if !seen.insert(branch.as_str()) {
            continue;
        }
        let entry = entry_for(&list, branch);
        let path = entry.map(|e| e.path.clone()).unwrap_or_default();
        if branch == &req.keep_branch
            || branch == &req.base_branch
            || entry.is_some_and(|e| e.main || ypath::same_path(&e.path, &main.path))
        {
            report.skipped.push(Skipped {
                branch: branch.clone(),
                path,
                reason: "protected".into(),
                files: vec![],
            });
            continue;
        }
        if recorded_run(&base_dir, branch) != req.run_id {
            report.skipped.push(Skipped {
                branch: branch.clone(),
                path,
                reason: "not-fanout".into(),
                files: vec![],
            });
            continue;
        }
        if let Some(e) = entry {
            let dir = PathBuf::from(&e.path);
            let force = req.discard_dirty_others;
            if !force {
                match work_status(&dir, false) {
                    Ok(s) if !s.changes.is_empty() => {
                        report.skipped.push(Skipped {
                            branch: branch.clone(),
                            path,
                            reason: "dirty".into(),
                            files: s.changes.into_iter().map(|c| c.path).collect(),
                        });
                        continue;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        report.errors.push(StepError {
                            step: "remove-worktree".into(),
                            branch: branch.clone(),
                            detail: err.to_string(),
                        });
                        continue;
                    }
                }
            }
            if let Err(err) = super::worktree_remove(&dir, force) {
                report.errors.push(StepError {
                    step: "remove-worktree".into(),
                    branch: branch.clone(),
                    detail: err.to_string(),
                });
                continue;
            }
            report.removed_worktrees.push(Removed {
                branch: branch.clone(),
                path: e.path.clone(),
            });
        }
        match run_git_untimed(&base_dir, &["branch", "-D", "--", branch]) {
            Ok(_) => report.deleted_branches.push(branch.clone()),
            Err(err) => report.errors.push(StepError {
                step: "delete-branch".into(),
                branch: branch.clone(),
                detail: err.to_string(),
            }),
        }
    }

    // The kept lane's own worktree (its branch stays: it is merged and cheap).
    if let (true, Some(dir)) = (req.remove_kept_worktree, &keeper_dir) {
        match super::worktree_remove(dir, false) {
            Ok(()) => report.kept_worktree_removed = true,
            Err(err) => report.errors.push(StepError {
                step: "remove-kept-worktree".into(),
                branch: req.keep_branch.clone(),
                detail: err.to_string(),
            }),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    struct Fixture {
        repo: PathBuf,
        wt: PathBuf,
        main: String,
        paths: Vec<PathBuf>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Worktree dirs first; the repo's metadata is removed with it.
            let _ = std::fs::remove_dir_all(&self.wt);
            let _ = std::fs::remove_dir_all(&self.repo);
        }
    }

    /// A repo with fan-out lanes `lane/a`, `lane/b`, `lane/c` of run `run-1`,
    /// each with one commit touching its own file.
    fn fixture(name: &str) -> Fixture {
        let repo = std::env::temp_dir().join(format!(
            "ymux_keep_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.email", "t@example.com"]);
        git_ok(&repo, &["config", "user.name", "T"]);
        git_ok(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("f.txt"), "hello\n").unwrap();
        git_ok(&repo, &["add", "f.txt"]);
        git_ok(&repo, &["commit", "-q", "-m", "init"]);
        let main = git_ok(&repo, &["symbolic-ref", "--short", "HEAD"])
            .trim()
            .to_string();
        let wt = PathBuf::from(format!("{}_wt", repo.display()));
        let mut paths = Vec::new();
        for b in ["lane/a", "lane/b", "lane/c"] {
            let p = super::super::suggested_worktree_path(&repo, b, &wt.to_string_lossy());
            super::super::worktree_add_lane(&repo, b, &p, &main, "run-1").unwrap();
            let file = format!("{}.txt", b.replace('/', "_"));
            std::fs::write(p.join(&file), "x\n").unwrap();
            git_ok(&p, &["add", &file]);
            git_ok(&p, &["commit", "-q", "-m", &format!("work {b}")]);
            paths.push(p);
        }
        Fixture {
            repo,
            wt,
            main,
            paths,
        }
    }

    fn req(fx: &Fixture) -> KeepRequest {
        KeepRequest {
            repo: fx.repo.to_string_lossy().into_owned(),
            base_branch: fx.main.clone(),
            run_id: "run-1".into(),
            keep_branch: "lane/a".into(),
            others: vec!["lane/b".into(), "lane/c".into()],
            remove_kept_worktree: true,
            discard_dirty_others: false,
            dry_run: false,
            cleanup_only: false,
        }
    }

    fn branches(repo: &Path) -> String {
        git_ok(repo, &["branch", "--list"])
    }

    #[test]
    fn happy_path_merges_and_cleans_up() {
        if !git_available() {
            return;
        }
        let fx = fixture("happy");
        let r = keep_lane(&req(&fx)).unwrap();
        assert_eq!(r.outcome, "merged", "{r:?}");
        let head = git_ok(&fx.repo, &["rev-parse", "HEAD"]).trim().to_string();
        assert_eq!(r.merged_commit, head);
        // --no-ff: a real merge commit with two parents.
        let parents = git_ok(&fx.repo, &["rev-list", "--parents", "-n1", "HEAD"]);
        assert_eq!(parents.split_whitespace().count(), 3, "{parents}");
        assert!(fx.repo.join("lane_a.txt").exists());
        assert!(!fx.repo.join("lane_b.txt").exists());
        assert_eq!(r.removed_worktrees.len(), 2, "{r:?}");
        assert_eq!(r.deleted_branches, vec!["lane/b", "lane/c"]);
        assert!(r.kept_worktree_removed);
        assert!(r.skipped.is_empty() && r.errors.is_empty(), "{r:?}");
        assert!(!fx.paths.iter().any(|p| p.exists()));
        let b = branches(&fx.repo);
        assert!(b.contains("lane/a") && !b.contains("lane/b") && !b.contains("lane/c"));
    }

    #[test]
    fn merge_only_and_kept_worktree_option_leave_the_rest() {
        if !git_available() {
            return;
        }
        let fx = fixture("mergeonly");
        let mut q = req(&fx);
        q.others.clear();
        q.remove_kept_worktree = false;
        let r = keep_lane(&q).unwrap();
        assert_eq!(r.outcome, "merged", "{r:?}");
        assert!(fx.paths.iter().all(|p| p.exists()));
        assert!(branches(&fx.repo).contains("lane/b"));
    }

    #[test]
    fn dry_run_changes_nothing() {
        if !git_available() {
            return;
        }
        let fx = fixture("dry");
        let before = git_ok(&fx.repo, &["rev-parse", "HEAD"]);
        let mut q = req(&fx);
        q.dry_run = true;
        assert_eq!(keep_lane(&q).unwrap().outcome, "ok");
        assert_eq!(git_ok(&fx.repo, &["rev-parse", "HEAD"]), before);
        assert!(fx.paths.iter().all(|p| p.exists()));
    }

    /// Lane a and the base both change f.txt differently.
    fn make_conflict(fx: &Fixture) {
        std::fs::write(fx.paths[0].join("f.txt"), "from lane\n").unwrap();
        git_ok(&fx.paths[0], &["commit", "-q", "-am", "lane edit"]);
        std::fs::write(fx.repo.join("f.txt"), "from base\n").unwrap();
        git_ok(&fx.repo, &["commit", "-q", "-am", "base edit"]);
    }

    #[test]
    fn dry_run_reports_a_conflict_and_changes_nothing() {
        if !git_available() {
            return;
        }
        let fx = fixture("drycon");
        make_conflict(&fx);
        let before = git_ok(&fx.repo, &["rev-parse", "HEAD"]);
        let mut q = req(&fx);
        q.dry_run = true;
        let r = keep_lane(&q).unwrap();
        if r.conflict_check_unavailable {
            // git older than 2.38: a warning, and the dry run still passes.
            assert_eq!(r.outcome, "ok", "{r:?}");
        } else {
            assert_eq!(r.outcome, "would-conflict", "{r:?}");
            assert_eq!(r.files, vec!["f.txt"]);
        }
        assert_eq!(git_ok(&fx.repo, &["rev-parse", "HEAD"]), before);
        assert!(git_ok(&fx.repo, &["status", "--porcelain"])
            .trim()
            .is_empty());
        assert!(!merge_in_progress(&fx.repo));
        assert!(fx.paths.iter().all(|p| p.exists()));
        assert!(branches(&fx.repo).contains("lane/b"));
    }

    #[test]
    fn cleanup_only_after_a_merge_only_call_removes_the_losers() {
        if !git_available() {
            return;
        }
        let fx = fixture("cleanonly");
        let mut q = req(&fx);
        q.others.clear();
        q.remove_kept_worktree = false;
        assert_eq!(keep_lane(&q).unwrap().outcome, "merged");
        let head = git_ok(&fx.repo, &["rev-parse", "HEAD"]);

        let mut c = req(&fx);
        c.cleanup_only = true;
        let r = keep_lane(&c).unwrap();
        assert_eq!(r.outcome, "cleaned", "{r:?}");
        assert!(r.merged_commit.is_empty());
        assert_eq!(r.deleted_branches, vec!["lane/b", "lane/c"], "{r:?}");
        assert!(r.kept_worktree_removed);
        assert!(r.errors.is_empty() && r.skipped.is_empty(), "{r:?}");
        // No second merge commit.
        assert_eq!(git_ok(&fx.repo, &["rev-parse", "HEAD"]), head);
        assert!(!fx.paths.iter().any(|p| p.exists()));
        let b = branches(&fx.repo);
        assert!(b.contains("lane/a") && !b.contains("lane/b"));
    }

    #[test]
    fn cleanup_only_keeps_every_safety_rule() {
        if !git_available() {
            return;
        }
        let fx = fixture("cleansafe");
        git_ok(&fx.repo, &["branch", "lane/imposter", &fx.main]);
        std::fs::write(fx.paths[1].join("scratch.txt"), "wip\n").unwrap();
        let mut q = req(&fx);
        q.others.clear();
        q.remove_kept_worktree = false;
        assert_eq!(keep_lane(&q).unwrap().outcome, "merged");

        let mut c = req(&fx);
        c.cleanup_only = true;
        c.others = vec!["lane/imposter".into(), "lane/b".into(), "lane/c".into()];
        let r = keep_lane(&c).unwrap();
        assert_eq!(r.outcome, "cleaned", "{r:?}");
        assert_eq!(r.deleted_branches, vec!["lane/c"]);
        let reasons: Vec<_> = r
            .skipped
            .iter()
            .map(|s| (s.branch.as_str(), s.reason.as_str()))
            .collect();
        assert_eq!(
            reasons,
            vec![("lane/imposter", "not-fanout"), ("lane/b", "dirty")]
        );
        assert!(fx.paths[1].join("scratch.txt").exists());
        assert!(branches(&fx.repo).contains("lane/imposter"));
    }

    #[test]
    fn cleanup_only_is_refused_when_not_merged() {
        if !git_available() {
            return;
        }
        let fx = fixture("cleanrefused");
        let mut c = req(&fx);
        c.cleanup_only = true;
        let r = keep_lane(&c).unwrap();
        assert_eq!(
            (r.outcome.as_str(), r.reason.as_str()),
            ("refused", "not-merged")
        );
        c.dry_run = true;
        assert_eq!(keep_lane(&c).unwrap().reason, "not-merged");
        assert!(fx.paths.iter().all(|p| p.exists()));
        assert!(branches(&fx.repo).contains("lane/b"));
    }

    #[test]
    fn conflict_is_aborted_and_base_stays_clean() {
        if !git_available() {
            return;
        }
        let fx = fixture("conflict");
        // Lane a and the base both change f.txt differently.
        std::fs::write(fx.paths[0].join("f.txt"), "from lane\n").unwrap();
        git_ok(&fx.paths[0], &["commit", "-q", "-am", "lane edit"]);
        std::fs::write(fx.repo.join("f.txt"), "from base\n").unwrap();
        git_ok(&fx.repo, &["commit", "-q", "-am", "base edit"]);
        let before = git_ok(&fx.repo, &["rev-parse", "HEAD"]);

        let r = keep_lane(&req(&fx)).unwrap();
        assert_eq!(r.outcome, "conflict", "{r:?}");
        assert_eq!(r.files, vec!["f.txt"]);
        assert!(!r.left_mid_merge);
        assert_eq!(git_ok(&fx.repo, &["rev-parse", "HEAD"]), before);
        assert!(git_ok(&fx.repo, &["status", "--porcelain"])
            .trim()
            .is_empty());
        assert!(!merge_in_progress(&fx.repo));
        // Nothing was cleaned up.
        assert!(fx.paths.iter().all(|p| p.exists()));
        assert!(branches(&fx.repo).contains("lane/b"));
        assert_eq!(
            std::fs::read_to_string(fx.repo.join("f.txt"))
                .unwrap()
                .trim(),
            "from base"
        );
    }

    #[test]
    fn dirty_base_is_refused_but_untracked_is_fine() {
        if !git_available() {
            return;
        }
        let fx = fixture("dirtybase");
        std::fs::write(fx.repo.join("stray.txt"), "untracked\n").unwrap();
        let mut q = req(&fx);
        q.dry_run = true;
        assert_eq!(keep_lane(&q).unwrap().outcome, "ok");

        std::fs::write(fx.repo.join("f.txt"), "edited\n").unwrap();
        q.dry_run = false;
        let r = keep_lane(&q).unwrap();
        assert_eq!(
            (r.outcome.as_str(), r.reason.as_str()),
            ("refused", "base-dirty")
        );
        assert_eq!(r.files, vec!["f.txt"]);
        assert!(fx.paths.iter().all(|p| p.exists()));
        assert!(!fx.repo.join("lane_a.txt").exists());
    }

    #[test]
    fn dirty_kept_lane_is_refused() {
        if !git_available() {
            return;
        }
        let fx = fixture("dirtykeep");
        std::fs::write(fx.paths[0].join("lane_a.txt"), "changed\n").unwrap();
        let r = keep_lane(&req(&fx)).unwrap();
        assert_eq!(
            (r.outcome.as_str(), r.reason.as_str()),
            ("refused", "lane-dirty")
        );
        assert_eq!(r.files, vec!["lane_a.txt"]);
        assert!(!fx.repo.join("lane_a.txt").exists());
        assert!(fx.paths.iter().all(|p| p.exists()));
    }

    #[test]
    fn base_branch_moved_and_no_commits_are_refused() {
        if !git_available() {
            return;
        }
        let fx = fixture("refusals");
        let mut q = req(&fx);
        q.base_branch = "somewhere-else".into();
        assert_eq!(keep_lane(&q).unwrap().reason, "base-branch");

        // A lane with nothing ahead of base.
        let p = fx.wt.join("empty");
        super::super::worktree_add_lane(&fx.repo, "lane/e", &p, &fx.main, "run-1").unwrap();
        let mut q = req(&fx);
        q.keep_branch = "lane/e".into();
        assert_eq!(keep_lane(&q).unwrap().reason, "no-commits");
    }

    #[test]
    fn dirty_losing_lane_is_skipped_not_deleted() {
        if !git_available() {
            return;
        }
        let fx = fixture("dirtyloser");
        std::fs::write(fx.paths[1].join("scratch.txt"), "wip\n").unwrap();
        let r = keep_lane(&req(&fx)).unwrap();
        assert_eq!(r.outcome, "merged", "{r:?}");
        assert_eq!(r.skipped.len(), 1, "{r:?}");
        assert_eq!(r.skipped[0].branch, "lane/b");
        assert_eq!(r.skipped[0].reason, "dirty");
        assert_eq!(r.skipped[0].files, vec!["scratch.txt"]);
        assert!(fx.paths[1].join("scratch.txt").exists());
        assert!(branches(&fx.repo).contains("lane/b"));
        // The clean loser went.
        assert_eq!(r.deleted_branches, vec!["lane/c"]);

        // With the explicit discard option the dirty one goes too.
        let fx2 = fixture("dirtyloser2");
        std::fs::write(fx2.paths[1].join("scratch.txt"), "wip\n").unwrap();
        let mut q2 = req(&fx2);
        q2.discard_dirty_others = true;
        let r2 = keep_lane(&q2).unwrap();
        assert_eq!(r2.deleted_branches, vec!["lane/b", "lane/c"], "{r2:?}");
        assert!(!fx2.paths[1].exists());
    }

    #[test]
    fn a_branch_the_fanout_did_not_make_is_never_deleted() {
        if !git_available() {
            return;
        }
        let fx = fixture("foreign");
        // Looks like a lane by name, has no fan-out record.
        git_ok(&fx.repo, &["branch", "lane/imposter", &fx.main]);
        // And a branch recorded under a different run.
        git_ok(&fx.repo, &["branch", "lane/other-run", &fx.main]);
        git_ok(
            &fx.repo,
            &["config", "branch.lane/other-run.ymuxFanout", "run-2"],
        );
        let mut q = req(&fx);
        q.others = vec![
            "lane/imposter".into(),
            "lane/other-run".into(),
            fx.main.clone(),
            "lane/a".into(),
            "lane/b".into(),
        ];
        q.remove_kept_worktree = false;
        let r = keep_lane(&q).unwrap();
        assert_eq!(r.outcome, "merged", "{r:?}");
        assert_eq!(r.deleted_branches, vec!["lane/b"]);
        let reasons: Vec<_> = r
            .skipped
            .iter()
            .map(|s| (s.branch.as_str(), s.reason.as_str()))
            .collect();
        assert_eq!(
            reasons,
            vec![
                ("lane/imposter", "not-fanout"),
                ("lane/other-run", "not-fanout"),
                (fx.main.as_str(), "protected"),
                ("lane/a", "protected"),
            ]
        );
        let b = branches(&fx.repo);
        for keep in [
            "lane/imposter",
            "lane/other-run",
            "lane/a",
            fx.main.as_str(),
        ] {
            assert!(b.contains(keep), "{keep} missing from {b}");
        }
        assert!(fx.paths[0].exists());
    }

    #[test]
    fn kept_lane_whose_worktree_is_already_gone_still_merges() {
        if !git_available() {
            return;
        }
        let fx = fixture("gone");
        super::super::worktree_remove(&fx.paths[0], false).unwrap();
        let r = keep_lane(&req(&fx)).unwrap();
        assert_eq!(r.outcome, "merged", "{r:?}");
        assert!(!r.kept_worktree_removed);
        assert!(r.errors.is_empty(), "{r:?}");
        assert!(fx.repo.join("lane_a.txt").exists());
    }

    #[test]
    fn options_that_look_like_flags_are_rejected() {
        let mut q = KeepRequest {
            repo: ".".into(),
            base_branch: "main".into(),
            run_id: "run-1".into(),
            keep_branch: "-D".into(),
            ..Default::default()
        };
        assert!(keep_lane(&q).is_err());
        q.keep_branch = "x".into();
        q.others = vec!["--force".into()];
        assert!(keep_lane(&q).is_err());
        q.others.clear();
        q.run_id = "bad id".into();
        assert!(keep_lane(&q).is_err());
    }
}
