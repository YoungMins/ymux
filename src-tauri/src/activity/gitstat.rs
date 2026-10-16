//! Git baseline at turn open, `DiffStat` at turn close.
//!
//! The baseline is HEAD plus the per-file line counts of the working tree
//! against HEAD (untracked files included). At close the same counts are
//! taken against the *baseline* HEAD, and only what moved since is reported:
//! changes the user already had before the prompt do not count as the
//! agent's. All git calls go through `crate::git` (`GIT_OPTIONAL_LOCKS=0`,
//! `CREATE_NO_WINDOW`). Best effort: any failure is `None`. A repo whose own
//! config defines `filter.*` drivers is skipped, since diffing would run them.

use super::turn::DiffStat;
use crate::git;
use std::collections::BTreeMap;
use std::path::Path;

/// `(added, deleted)` per repo-relative path.
pub type Counts = BTreeMap<String, (u32, u32)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBaseline {
    pub root: String,
    /// Full hash of HEAD when the turn opened.
    pub head: String,
    pub files: Counts,
}

fn counts_of(files: &[git::DiffFile]) -> Counts {
    files
        .iter()
        .map(|f| (f.path.clone(), (f.added, f.deleted)))
        .collect()
}

/// What changed between `base` and `now`: a file counts when it is new or its
/// counts differ; lines are the growth over the baseline. A file that was
/// dirty at the baseline and is clean now counts as touched, with no lines.
pub fn delta(base: &Counts, now: &Counts) -> DiffStat {
    let mut files = 0u32;
    let mut ins = 0u32;
    let mut del = 0u32;
    for (path, &(a, d)) in now {
        match base.get(path) {
            Some(&b) if b == (a, d) => {}
            Some(&(ba, bd)) => {
                files = files.saturating_add(1);
                ins = ins.saturating_add(a.saturating_sub(ba));
                del = del.saturating_add(d.saturating_sub(bd));
            }
            None => {
                files = files.saturating_add(1);
                ins = ins.saturating_add(a);
                del = del.saturating_add(d);
            }
        }
    }
    for path in base.keys() {
        if !now.contains_key(path) {
            files = files.saturating_add(1);
        }
    }
    DiffStat {
        files,
        insertions: ins,
        deletions: del,
    }
}

/// Snapshot the repo containing `cwd` (the pane's OSC 7 directory, never a
/// hook claim). `None` outside a repository or on any git failure.
pub fn baseline(cwd: &str) -> Option<GitBaseline> {
    if cwd.trim().is_empty() {
        return None;
    }
    let root = git::repo_root(Path::new(cwd)).ok()?;
    if git::has_local_filter_drivers(&root) {
        return None;
    }
    let head = git::resolve_base(&root, "").ok()?;
    let sum = git::diff_summary(&root, "", false).ok()?;
    Some(GitBaseline {
        root: root.to_string_lossy().into_owned(),
        head,
        files: counts_of(&sum.files),
    })
}

/// The agent's share of the working-tree changes since `b`.
pub fn stat(b: &GitBaseline) -> Option<DiffStat> {
    if git::has_local_filter_drivers(Path::new(&b.root)) {
        return None;
    }
    let sum = git::diff_summary(Path::new(&b.root), &b.head, false).ok()?;
    Some(delta(&b.files, &counts_of(&sum.files)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn c(items: &[(&str, u32, u32)]) -> Counts {
        items
            .iter()
            .map(|(p, a, d)| (p.to_string(), (*a, *d)))
            .collect()
    }

    #[test]
    fn delta_ignores_what_was_already_dirty() {
        let base = c(&[("pre.rs", 5, 1), ("same.rs", 2, 2)]);
        let now = c(&[
            ("pre.rs", 9, 1),
            ("same.rs", 2, 2),
            ("new.rs", 10, 0),
            ("del.rs", 0, 7),
        ]);
        assert_eq!(
            delta(&base, &now),
            DiffStat {
                files: 3,
                insertions: 14,
                deletions: 7
            }
        );
    }

    #[test]
    fn delta_counts_a_reverted_file_without_lines() {
        let base = c(&[("a.rs", 3, 0)]);
        assert_eq!(
            delta(&base, &Counts::new()),
            DiffStat {
                files: 1,
                insertions: 0,
                deletions: 0
            }
        );
        assert_eq!(delta(&Counts::new(), &Counts::new()), DiffStat::default());
    }

    fn git_ok(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Real git in a temp repo: pre-existing dirt is not the agent's, a new
    /// file and an edit are, and a non-repo has no baseline.
    #[test]
    fn real_repo_baseline_and_stat() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        if !git_ok(p, &["init", "-q"]) {
            return; // no git on this machine
        }
        std::fs::write(p.join("a.txt"), "1\n2\n3\n").unwrap();
        std::fs::write(p.join("dirty.txt"), "x\n").unwrap();
        assert!(git_ok(p, &["add", "."]));
        assert!(git_ok(p, &["commit", "-q", "-m", "init"]));
        std::fs::write(p.join("dirty.txt"), "x\ny\n").unwrap(); // before the turn
        let cwd = p.to_string_lossy().into_owned();
        let b = baseline(&cwd).expect("baseline");
        assert_eq!(b.files.get("dirty.txt"), Some(&(1, 0)));
        assert_eq!(stat(&b), Some(DiffStat::default()));

        std::fs::write(p.join("a.txt"), "1\n2\n3\n4\n5\n").unwrap();
        std::fs::write(p.join("new.txt"), "n1\nn2\n").unwrap();
        let st = stat(&b).expect("stat");
        assert_eq!(
            st,
            DiffStat {
                files: 2,
                insertions: 4,
                deletions: 0
            }
        );

        let other = tempfile::tempdir().unwrap();
        assert!(baseline(&other.path().to_string_lossy()).is_none());
        assert!(baseline("").is_none());
    }

    #[test]
    fn repo_local_filter_driver_skips_baseline_and_stat() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        if !git_ok(p, &["init", "-q"]) {
            return; // no git on this machine
        }
        std::fs::write(
            p.join("a.txt"),
            "1
",
        )
        .unwrap();
        assert!(git_ok(p, &["add", "."]));
        assert!(git_ok(p, &["commit", "-q", "-m", "init"]));
        let cwd = p.to_string_lossy().into_owned();
        assert!(!git::has_local_filter_drivers(p));
        let b = baseline(&cwd).expect("no filter: baseline");
        assert!(git_ok(
            p,
            &["config", "--local", "filter.evil.clean", "cat"]
        ));
        assert!(git::has_local_filter_drivers(p));
        assert!(baseline(&cwd).is_none());
        assert!(stat(&b).is_none());
    }
}
