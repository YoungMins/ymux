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
///
/// Taking the rest of the line verbatim is correct, and deliberately so.
/// Unlike `git status`/`git ls-files`, the `worktree list` porcelain format
/// does **not** C-quote: paths and ref names come through as raw UTF-8 with
/// spaces and non-ASCII intact, and `core.quotePath` does not apply to it
/// (verified against git 2.52 with a Korean worktree path and a Korean
/// branch name). So there is no octal-escape decoding to do here. The one
/// spelling that does not survive is a path containing a newline, which
/// would need the `-z` form; nothing in ymux can produce one, so it is left
/// unhandled rather than guessed at.
///
/// Note that git reports the path in *its* spelling -- on Windows,
/// forward slashes and git's own idea of the drive-letter case -- which is
/// not the spelling [`suggested_worktree_path`] produces. Compare the two
/// with [`ypath::same_path`], never with `==`.
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

// ---------------------------------------------------------------------------
// Log and branch listing, for the git pane.
// ---------------------------------------------------------------------------

/// One commit, as the git pane draws it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitInfo {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Author date in Unix-epoch milliseconds.
    pub date_ms: u64,
    /// Parent hashes, in git's order. The graph lanes are computed from
    /// these rather than parsed back out of drawn ASCII.
    pub parents: Vec<String>,
    /// Decorations (`HEAD -> main`, `tag: v1`, …), already split.
    pub refs: Vec<String>,
}

/// `git branch`'s answer, with the current branch pulled out.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BranchList {
    /// Empty on a detached HEAD or in a repo with no commits.
    pub current: String,
    pub local: Vec<String>,
    pub remote: Vec<String>,
}

/// Unit separator: between the fields of one commit.
const US: char = '\u{1f}';
/// Record separator: between commits.
const RS: char = '\u{1e}';

/// The `--format` string [`parse_log_porcelain`] expects.
///
/// `tools/ygit` runs `git log --graph --oneline --decorate` and parses the
/// *drawn ASCII graph* back out with a lane colouriser
/// (`tools/ygit/src/graph.rs`). The GUI does not inherit that: a machine
/// format plus parent hashes is strictly more information and has no
/// escaping hazards.
///
/// `%s` is **last** on purpose. A subject cannot contain a unit separator in
/// practice, but "in practice" is not a parser contract — with the subject
/// last, `splitn` hands it whatever remains and the parser stays total even
/// if one does.
pub const LOG_FORMAT: &str = "%H\u{1f}%h\u{1f}%an\u{1f}%at\u{1f}%P\u{1f}%D\u{1f}%s\u{1e}";

/// Parse the output of `git log --format=`[`LOG_FORMAT`].
///
/// Pure and total: a malformed record is skipped rather than failing the
/// whole listing, and empty input (a repo with no commits) is an empty Vec,
/// not an error.
pub fn parse_log_porcelain(out: &str) -> Vec<CommitInfo> {
    out.split(RS)
        .filter_map(|record| {
            // Records are separated by RS; git also writes a newline after
            // each, which lands at the head of the next record.
            let record = record.trim_start_matches(['\n', '\r']);
            if record.is_empty() {
                return None;
            }
            let mut f = record.splitn(7, US);
            let hash = f.next()?;
            let short = f.next()?;
            let author = f.next()?;
            let date_s = f.next()?;
            let parents = f.next()?;
            let refs = f.next()?;
            // The subject may legally be empty (`git commit --allow-empty-message`),
            // and a record that ends here has none at all.
            let subject = f.next().unwrap_or("");
            if hash.is_empty() {
                return None;
            }
            Some(CommitInfo {
                hash: hash.to_string(),
                short: short.to_string(),
                subject: subject.to_string(),
                author: author.to_string(),
                // `%at` is seconds; a garbled value becomes 0 rather than
                // sinking the record.
                date_ms: date_s
                    .trim()
                    .parse::<u64>()
                    .unwrap_or(0)
                    .saturating_mul(1000),
                parents: parents.split_whitespace().map(str::to_string).collect(),
                refs: refs
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect()
}

/// Strip the two-column marker `git branch` writes in front of a name.
///
/// Ported verbatim from `tools/ygit/src/app.rs`. It strips exactly *one*
/// marker — `* ` (current), `+ ` (checked out in another worktree) or two
/// spaces — and rejects the `(HEAD detached at …)` pseudo-entry, which is
/// prose and not a branch anyone can check out.
///
/// Kept even though [`parse_branch_list`] feeds it `--format=%(refname:short)`
/// output that carries no marker: it is a regression guard for the bug it
/// was written for, where a leftover `+ ` made git answer
/// `fatal: invalid reference: + 워크트리브랜치`.
pub fn branch_name(raw: &str) -> Option<&str> {
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

/// The `--format` string [`parse_branch_list`] expects, for
/// `git branch --list --all --format=…`.
///
/// `%(HEAD)` writes `*` for the current branch and a space otherwise, so the
/// current branch needs no second git call. The **full** `%(refname)` is
/// used rather than `%(refname:short)` because only the full form says
/// whether a branch is local: `refname:short` renders
/// `refs/remotes/origin/main` as `origin/main`, which is indistinguishable
/// from a local branch literally called `origin/main`.
pub const BRANCH_FORMAT: &str = "%(HEAD)%(refname)";

/// Parse `git branch --list --all --format=`[`BRANCH_FORMAT`].
///
/// An `origin/HEAD -> origin/main` symbolic entry is dropped: it is an
/// alias, not a branch to check out.
pub fn parse_branch_list(out: &str) -> BranchList {
    let mut list = BranchList::default();
    for line in out.lines() {
        // `%(HEAD)` occupies exactly one column: `*` or a space. Strip it
        // before `branch_name`, which handles the plain `git branch`
        // spellings (`* `, `+ `, two spaces) rather than this one.
        let is_head = line.starts_with('*');
        let rest = line
            .strip_prefix('*')
            .or_else(|| line.strip_prefix(' '))
            .unwrap_or(line);
        let Some(name) = branch_name(rest) else {
            continue;
        };
        if name.contains("->") {
            continue;
        }
        if let Some(remote) = name.strip_prefix("refs/remotes/") {
            if remote.ends_with("/HEAD") {
                continue;
            }
            list.remote.push(remote.to_string());
        } else if let Some(local) = name.strip_prefix("refs/heads/") {
            if is_head {
                list.current = local.to_string();
            }
            list.local.push(local.to_string());
        }
        // Anything else (`refs/tags/…`, a bare name from an older call
        // site) is not a branch this pane offers.
    }
    list
}

/// Reject a branch name that `git` would read as an option or that carries
/// control characters.
///
/// `--` does not help here: after it, git treats the argument as a
/// *pathspec*, not a branch, so `git checkout -- -x` is a different command
/// rather than a safe one. Nothing in this module ever builds a shell
/// command line — `Command` passes argv directly — so the hazard is only
/// git's own option parsing.
pub fn validate_ref_name(name: &str) -> YmuxResult<()> {
    if name.is_empty() {
        return Err(YmuxError::Git("empty branch name".into()));
    }
    if name.starts_with('-') {
        return Err(YmuxError::Git(format!(
            "refusing a branch name git would read as an option: {name}"
        )));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(YmuxError::Git(
            "refusing a branch name with control characters".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Captured from `git log --format=`[`LOG_FORMAT`] on git 2.52.0 (the
    /// version the `ygit` fixtures were taken on), then hand-extended with
    /// the cases a real repo will eventually produce: a merge with two
    /// parents, an octopus merge with three, several decorations on one
    /// commit, a Korean subject, a subject containing `|` (which `ygit`'s
    /// ASCII-graph parser had to care about and this one does not), and a
    /// commit with no refs.
    const REAL_LOG_OUTPUT: &str = concat!(
        "aa0975780408202f2372554025fcd1348a519d90\u{1f}aa09757\u{1f}YoungMins\u{1f}1790218358\u{1f}",
        "3da4830e1449837606319828506d4858b60f2b9f\u{1f}HEAD -> claude/gui-step1-backend, tag: v0.10.1\u{1f}",
        "feat(textfile): EOL, BOM, stamps and the edit cap\u{1e}\n",
        "3da4830e1449837606319828506d4858b60f2b9f\u{1f}3da4830\u{1f}YoungMins\u{1f}1790218219\u{1f}",
        "a96bc3b2a5802c1d1371f0bf60d0ca47f3becb66 851060e5bbd9dd149fec418a7ea72e603ac965e2\u{1f}\u{1f}",
        "Merge branch 'claude/x' — 한글 제목 with a | pipe\u{1e}\n",
        "a96bc3b2a5802c1d1371f0bf60d0ca47f3becb66\u{1f}a96bc3b\u{1f}YoungMins\u{1f}1790218083\u{1f}",
        "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 3333333333333333333333333333333333333333\u{1f}main, origin/main\u{1f}",
        "octopus\u{1e}\n",
    );

    #[test]
    fn parses_the_machine_log_format() {
        let commits = parse_log_porcelain(REAL_LOG_OUTPUT);
        assert_eq!(commits.len(), 3);

        let c = &commits[0];
        assert_eq!(c.hash, "aa0975780408202f2372554025fcd1348a519d90");
        assert_eq!(c.short, "aa09757");
        assert_eq!(c.author, "YoungMins");
        // `%at` is seconds; the frontend wants milliseconds.
        assert_eq!(c.date_ms, 1_790_218_358_000);
        assert_eq!(c.parents, vec!["3da4830e1449837606319828506d4858b60f2b9f"]);
        assert_eq!(
            c.refs,
            vec!["HEAD -> claude/gui-step1-backend", "tag: v0.10.1"]
        );
        assert_eq!(
            c.subject,
            "feat(textfile): EOL, BOM, stamps and the edit cap"
        );
    }

    /// A merge has two parents and an octopus merge more, and the graph is
    /// drawn from those rather than from parsed ASCII.
    #[test]
    fn log_carries_every_parent_and_a_non_ascii_subject() {
        let commits = parse_log_porcelain(REAL_LOG_OUTPUT);

        let merge = &commits[1];
        assert_eq!(merge.parents.len(), 2);
        assert_eq!(
            merge.subject,
            "Merge branch 'claude/x' — 한글 제목 with a | pipe"
        );
        // A commit with no decorations gets an empty list, not `[""]`.
        assert!(merge.refs.is_empty());

        let octopus = &commits[2];
        assert_eq!(octopus.parents.len(), 3);
        assert_eq!(octopus.refs, vec!["main", "origin/main"]);
    }

    /// Totality: a repo with no commits, and a record git could never write
    /// but a parser must survive.
    #[test]
    fn log_parser_is_total() {
        assert!(parse_log_porcelain("").is_empty());
        assert!(parse_log_porcelain("\n").is_empty());
        assert!(parse_log_porcelain("\u{1e}\n\u{1e}").is_empty());
        // Too few fields: skipped, not panicked on, and the good record
        // beside it still lands.
        let mixed = concat!(
            "short\u{1f}record\u{1e}\n",
            "ffff\u{1f}ff\u{1f}me\u{1f}1000\u{1f}\u{1f}\u{1f}ok\u{1e}\n",
        );
        let got = parse_log_porcelain(mixed);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].subject, "ok");
        // An empty commit message is legal (`--allow-empty-message`).
        let empty_subject =
            parse_log_porcelain("ffff\u{1f}ff\u{1f}me\u{1f}1000\u{1f}\u{1f}\u{1f}\u{1e}");
        assert_eq!(empty_subject.len(), 1);
        assert_eq!(empty_subject[0].subject, "");
        // An unparseable date must not sink the record.
        let bad_date =
            parse_log_porcelain("ffff\u{1f}ff\u{1f}me\u{1f}not-a-number\u{1f}\u{1f}\u{1f}s\u{1e}");
        assert_eq!(bad_date[0].date_ms, 0);
    }

    /// Captured from `git branch --list --all --format=`[`BRANCH_FORMAT`] on
    /// git 2.52.0, including the `origin/HEAD` symbolic alias.
    const REAL_BRANCH_FORMAT_OUTPUT: &str = "\
 refs/heads/claude/agent-resume
*refs/heads/claude/gui-step1-backend
 refs/heads/기능/한글브랜치
 refs/remotes/origin/HEAD
 refs/remotes/origin/main
";

    #[test]
    fn parses_the_branch_format_and_finds_the_current_branch() {
        let list = parse_branch_list(REAL_BRANCH_FORMAT_OUTPUT);
        assert_eq!(list.current, "claude/gui-step1-backend");
        assert_eq!(
            list.local,
            vec![
                "claude/agent-resume",
                "claude/gui-step1-backend",
                "기능/한글브랜치"
            ]
        );
        // `origin/HEAD` is an alias, not a branch to check out.
        assert_eq!(list.remote, vec!["origin/main"]);
    }

    #[test]
    fn branch_list_survives_an_empty_repo_and_a_detached_head() {
        assert_eq!(parse_branch_list(""), BranchList::default());
        // A detached HEAD has no current branch; `git branch` prints prose.
        let detached = "\
*(HEAD detached at b013596)
 refs/heads/main
";
        let list = parse_branch_list(detached);
        assert_eq!(list.current, "");
        assert_eq!(list.local, vec!["main"]);
    }

    // The next three are `tools/ygit/src/app.rs`'s tests, kept verbatim as a
    // regression guard on the marker stripping even though `BRANCH_FORMAT`
    // no longer produces those spellings.
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
        assert_eq!(branch_name("  (no branch)"), None);
        assert_eq!(branch_name(""), None);
    }

    /// Exactly one marker, so a branch whose name really does begin with a
    /// space-padded run keeps it.
    #[test]
    fn branch_name_strips_one_marker_not_a_run() {
        assert_eq!(branch_name("*   spaced"), Some("spaced"));
        assert_eq!(branch_name("    deep"), Some("deep"));
    }

    /// `--` would not help: git reads the argument after it as a pathspec,
    /// so an option-looking branch name has to be refused outright.
    #[test]
    fn ref_names_that_git_would_read_as_options_are_refused() {
        assert!(validate_ref_name("main").is_ok());
        assert!(validate_ref_name("기능/한글브랜치").is_ok());
        assert!(validate_ref_name("feature/a-b").is_ok());
        assert!(validate_ref_name("").is_err());
        assert!(validate_ref_name("--exec=calc.exe").is_err());
        assert!(validate_ref_name("-f").is_err());
        assert!(validate_ref_name("main\nrm -rf /").is_err());
        assert!(validate_ref_name("main\u{1b}[0m").is_err());
    }

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

    /// git prints worktree paths in its own spelling -- forward slashes on
    /// Windows -- while `suggested_worktree_path` builds them with the
    /// platform separator. `==` between the two is false on Windows, so any
    /// "is this the worktree we made?" check has to go through the key.
    #[test]
    fn porcelain_path_and_suggested_path_differ_only_by_spelling() {
        let out = "worktree C:/Users/u/.ymux-worktrees/agent-x
HEAD abc
branch refs/heads/agent/x
";
        let list = parse_worktree_porcelain(out);
        let ours = r"C:\Users\u\.ymux-worktrees\agent-x";
        assert_ne!(
            list[0].path, ours,
            "fixture is wrong: the two spellings must differ byte-wise"
        );
        assert!(ypath::same_path(&list[0].path, ours));
    }

    /// Captured verbatim from real git 2.52 run against a worktree at a
    /// Korean path containing a space, on a Korean branch. `worktree list
    /// --porcelain` does not C-quote and ignores `core.quotePath`, so "the
    /// rest of the line" is the whole path -- spaces, Hangul and all.
    #[test]
    fn porcelain_keeps_non_ascii_and_spaces_verbatim() {
        let out = "worktree C:/tmp/gitexp/한글 워크트리
HEAD b0135962b6fe5c1d1a3325e2f7aee6ff2ca785be
branch refs/heads/워크트리브랜치
";
        let list = parse_worktree_porcelain(out);
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].path, "C:/tmp/gitexp/한글 워크트리",
            "the space must not end the path and the Hangul must not be escaped"
        );
        assert_eq!(list[0].branch, "워크트리브랜치");
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

    /// Whether the worktree git reported at `entry_path` is the one we asked
    /// it to create at `asked`.
    ///
    /// Compares the *leaf*, not the whole path, because git resolves
    /// symlinks when it records a worktree -- verified: adding a worktree
    /// through a Windows junction makes `worktree list --porcelain` report
    /// the junction's target. `init_test_repo` builds under
    /// `std::env::temp_dir()`, which on macOS is `/var/folders/...` and
    /// `/var` is a symlink to `/private/var`, so git's path and ours differ
    /// by a prefix that no comparison key is allowed to fold away
    /// (`ypath::same_path` documents symlinks as out of scope, correctly).
    ///
    /// The leaf still goes through the key, so the separator and case rules
    /// apply to it; `porcelain_path_and_suggested_path_differ_only_by_spelling`
    /// is the deterministic test for the whole-path case.
    /// The leaf is taken from the *key* of the whole asked-for path rather
    /// than from `Path::file_name`, so it inherits that path's case rule --
    /// a bare `Agent-X` on its own proves no Windows syntax and would stay
    /// case-sensitive, which is right for a path and wrong for a component
    /// of `C:\wt\Agent-X`.
    fn worktree_leaf_is(entry_path: &str, asked: &Path) -> bool {
        let asked_key = ypath::comparison_key(&asked.to_string_lossy());
        let Some(leaf) = asked_key.rsplit('/').next().filter(|s| !s.is_empty()) else {
            return false;
        };
        let key = ypath::comparison_key(entry_path);
        key == leaf || key.ends_with(&format!("/{leaf}"))
    }

    /// The macOS shape, pinned down without needing macOS to run it.
    #[test]
    fn worktree_leaf_is_survives_a_resolved_symlink_prefix() {
        let asked = Path::new("/var/folders/t/ymux_git_test/.ymux-worktrees/agent-x");
        assert!(worktree_leaf_is(
            "/private/var/folders/t/ymux_git_test/.ymux-worktrees/agent-x",
            asked
        ));
        // A leaf match, not a substring one.
        assert!(!worktree_leaf_is("/private/var/wt/super-agent-x", asked));
        assert!(!worktree_leaf_is("/private/var/wt/agent-y", asked));
        // Still crosses the Windows separator and case divide.
        let win = Path::new(r"C:\wt\.ymux-worktrees\Agent-X");
        assert!(worktree_leaf_is("C:/wt/.ymux-worktrees/agent-x", win));
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
        // Cross-source comparison: `entry.path` is git's spelling (forward
        // slashes on Windows), `wt_path` is the one this module built with
        // the platform separator. Byte equality fails on Windows.
        assert!(
            worktree_leaf_is(&entry.path, &wt_path),
            "entry path should be the suggested worktree path: git said {}, we asked for {}",
            entry.path,
            wt_path.display()
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
            .find(|e| worktree_leaf_is(&e.path, &wt_path))
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

    /// The `porcelain_keeps_non_ascii_and_spaces_verbatim` fixture, against
    /// the real binary. `core.quotePath` is left at its default (which *is*
    /// quoting -- `git status --short` octal-escapes the same name), to pin
    /// down that it does not reach this format. If a future git ever starts
    /// C-quoting here, the parser needs a decoder and this test is what says
    /// so.
    #[test]
    fn worktree_list_does_not_quote_korean_paths_or_branches() {
        if !git_available() {
            eprintln!(
                "skipping worktree_list_does_not_quote_korean_paths_or_branches: git not on PATH"
            );
            return;
        }
        let repo = init_test_repo("hangul");
        let branch = "기능/한글";
        let wt_path = suggested_worktree_path(&repo, branch, "");

        worktree_add(&repo, branch, &wt_path).expect("worktree_add should succeed");

        let list = worktree_list(&repo).expect("worktree_list should succeed");
        let entry = list
            .iter()
            .find(|e| e.branch == branch)
            .unwrap_or_else(|| panic!("branch should come back unescaped, got {list:?}"));
        assert!(
            !entry.path.contains("\\3"),
            "path looks C-quoted (octal escapes): {}",
            entry.path
        );
        assert!(
            worktree_leaf_is(&entry.path, &wt_path),
            "git said {}, we asked for {}",
            entry.path,
            wt_path.display()
        );

        worktree_remove(&wt_path, false).expect("cleanup: worktree_remove should succeed");
        cleanup_dir(&repo);
        if let Some(parent) = repo.parent() {
            let _ = std::fs::remove_dir(parent.join(".ymux-worktrees"));
        }
    }
}
