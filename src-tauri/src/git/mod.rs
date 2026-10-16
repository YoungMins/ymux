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
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct WorktreeEntry {
    pub path: String,
    /// Empty on a detached HEAD (and on a bare entry).
    pub branch: String,
    /// The commit checked out there (`HEAD <sha>`); empty for a bare entry.
    pub head: String,
    pub detached: bool,
    pub bare: bool,
    /// `git worktree lock`ed: git refuses to remove it without a double
    /// `--force`, which the git pane never passes.
    pub locked: bool,
    /// Its directory is gone; `git worktree prune` would drop the entry.
    pub prunable: bool,
    /// The main worktree — always the first entry git lists. It cannot be
    /// removed, only the linked ones can.
    pub main: bool,
    /// The worktree containing the directory the list was asked from.
    /// Decided here with `ypath::same_path` (rule 15), never in TypeScript.
    pub current: bool,
}

/// A `git` invocation in `cwd`, and the only way this module builds one.
///
/// On Windows a release ymux is a GUI-subsystem process with no console,
/// so every console child it spawns gets a *new* console window unless told
/// otherwise — a black window flashing up on each call. The git pane calls
/// git on every refresh, window focus and followed `cd`, so without
/// `CREATE_NO_WINDOW` the flashing is continuous. `tauri dev` builds keep a
/// console and never show it, which is how it goes unnoticed.
///
/// `GIT_OPTIONAL_LOCKS=0` (the env form of `--no-optional-locks`): a pane
/// that refreshes on focus must not take `index.lock` for a `git status`
/// stat-cache refresh while the user runs `git commit` in a terminal pane.
///
/// `-c core.fsmonitor=false`: a repository can configure `core.fsmonitor` as a
/// command, which git runs on `status`/`diff`. ymux runs git unattended in
/// whatever directory an agent works in, so a repo-supplied hook must never
/// execute from here.
fn git_command(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["-c", "core.fsmonitor=false"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Run a git subcommand in `cwd`, returning stdout on success or a
/// `YmuxError::Git` wrapping stderr on failure.
fn run_git(cwd: &Path, args: &[&str]) -> YmuxResult<String> {
    let out = git_command(cwd)
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
    git_command(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Does the repo's own config define a `filter.*` driver (clean/smudge/
/// process)? `git diff` and `git status` run those drivers, i.e. a command the
/// repo chose, so best-effort background callers skip such repos. Anything
/// other than a clean "no match" (exit 1) also answers `true`.
pub fn has_local_filter_drivers(cwd: &Path) -> bool {
    match git_command(cwd)
        .args(["config", "--local", "--get-regexp", r"^filter\."])
        .output()
    {
        Ok(o) if o.status.code() == Some(1) => false,
        Ok(o) if o.status.success() => !o.stdout.iter().all(u8::is_ascii_whitespace),
        _ => true,
    }
}

/// Resolve the top-level directory of the repo containing `cwd`.
pub fn repo_root(cwd: &Path) -> YmuxResult<PathBuf> {
    let out = run_git(cwd, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

/// Add a worktree at `path`. Attaches to `branch` if it already exists as a
/// local branch, otherwise creates it (`git worktree add -b`).
pub fn worktree_add(repo: &Path, branch: &str, path: &Path) -> YmuxResult<()> {
    // The git pane types this name in; refuse one git would read as an
    // option before it reaches `-b`.
    validate_ref_name(branch)?;
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

/// Create a worktree at `path` on a **new** branch `branch` starting at
/// `start` (a ref or a sha). Unlike [`worktree_add`] it never attaches to an
/// existing branch: a fan-out lane must own its branch, so a clash is an error.
pub fn worktree_add_new(repo: &Path, branch: &str, path: &Path, start: &str) -> YmuxResult<()> {
    // Both reach git as argv entries; a leading `-` would be an option.
    validate_ref_name(branch)?;
    validate_ref_name(start)?;
    let heads_ref = format!("refs/heads/{branch}");
    if run_git(repo, &["show-ref", "--verify", "--quiet", "--", &heads_ref]).is_ok() {
        return Err(YmuxError::Git(format!("branch already exists: {branch}")));
    }
    let path_s = path.to_string_lossy();
    run_git(
        repo,
        &["worktree", "add", "-b", branch, "--", &path_s, start],
    )
    .map(|_| ())
}

/// Resolve `rev` to a full commit sha.
pub fn resolve_commit(cwd: &Path, rev: &str) -> YmuxResult<String> {
    validate_ref_name(rev)?;
    let spec = format!("{rev}^{{commit}}");
    let out = run_git(cwd, &["rev-parse", "--verify", "--quiet", &spec])?;
    Ok(out.trim().to_string())
}

/// Fan-out lane metadata kept in git config on the lane's branch.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct LaneMeta {
    /// The sha the lane branched from; empty when absent.
    pub base: String,
    /// The fan-out run id; empty when absent.
    pub fanout: String,
}

fn valid_run_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn valid_sha(s: &str) -> bool {
    (40..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Record `base` (a full sha) and `run_id` on `branch` as
/// `branch.<b>.ymuxBase` / `branch.<b>.ymuxFanout`.
pub fn lane_meta_set(repo: &Path, branch: &str, base: &str, run_id: &str) -> YmuxResult<()> {
    validate_ref_name(branch)?;
    if !valid_sha(base) {
        return Err(YmuxError::Git(format!("not a full commit sha: {base}")));
    }
    if !valid_run_id(run_id) {
        return Err(YmuxError::Git(format!("invalid fan-out run id: {run_id}")));
    }
    run_git(
        repo,
        &["config", &format!("branch.{branch}.ymuxBase"), base],
    )?;
    run_git(
        repo,
        &["config", &format!("branch.{branch}.ymuxFanout"), run_id],
    )?;
    Ok(())
}

/// Lane metadata of the branch checked out at `cwd`; empty strings when HEAD
/// is detached or the keys are unset.
pub fn lane_meta(cwd: &Path) -> YmuxResult<LaneMeta> {
    if !is_git_repo(cwd) {
        return Err(YmuxError::NotARepo(cwd.to_string_lossy().into_owned()));
    }
    let Some(branch) = current_branch(cwd) else {
        return Ok(LaneMeta::default());
    };
    let get = |key: &str| -> String {
        git_command(cwd)
            .args(["config", "--get", &format!("branch.{branch}.{key}")])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    Ok(LaneMeta {
        base: get("ymuxBase"),
        fanout: get("ymuxFanout"),
    })
}

/// Short name of the branch HEAD points at in `cwd`; `None` when detached.
pub fn current_branch(cwd: &Path) -> Option<String> {
    let out = git_command(cwd)
        .args(["symbolic-ref", "-q", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Create a fan-out lane: a worktree on new branch `branch` at `path`,
/// started at `start`, tagged with the resolved start sha and `run_id`.
pub fn worktree_add_lane(
    repo: &Path,
    branch: &str,
    path: &Path,
    start: &str,
    run_id: &str,
) -> YmuxResult<()> {
    if !valid_run_id(run_id) {
        return Err(YmuxError::Git(format!("invalid fan-out run id: {run_id}")));
    }
    let sha = resolve_commit(repo, start)?;
    worktree_add_new(repo, branch, path, &sha)?;
    lane_meta_set(repo, branch, &sha, run_id).map_err(|e| untagged_error(path, &e))
}

/// The worktree exists but its lane tags could not be written: say where it
/// is, so the user can use or remove it.
fn untagged_error(path: &Path, cause: &YmuxError) -> YmuxError {
    YmuxError::Git(format!(
        "worktree created but not tagged: {} ({cause})",
        path.display()
    ))
}

/// Commits reachable from HEAD but not from `base` (`git rev-list --count
/// <base>..HEAD`). `base` must be a hex sha (4-64 digits): it is placed in a
/// revision range, so nothing else may reach git.
pub fn commits_ahead(cwd: &Path, base: &str) -> YmuxResult<u32> {
    if !(4..=64).contains(&base.len()) || !base.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(YmuxError::Git(format!("not a commit sha: {base}")));
    }
    let range = format!("{base}..HEAD");
    let out = run_git(cwd, &["rev-list", "--count", &range])?;
    out.trim()
        .parse()
        .map_err(|_| YmuxError::Git(format!("unexpected rev-list output: {}", out.trim())))
}

/// Delete a fully merged local branch (`git branch -d`, never `-D`). Refuses
/// the branch checked out at `cwd`.
pub fn branch_delete(cwd: &Path, branch: &str) -> YmuxResult<()> {
    validate_ref_name(branch)?;
    if current_branch(cwd).as_deref() == Some(branch) {
        return Err(YmuxError::Git(format!(
            "refusing to delete the current branch: {branch}"
        )));
    }
    run_git(cwd, &["branch", "-d", "--", branch]).map(|_| ())
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

/// List all worktrees registered against `repo`, with `current` set on the
/// one whose path is `repo` itself.
pub fn worktree_list(repo: &Path) -> YmuxResult<Vec<WorktreeEntry>> {
    let out = run_git(repo, &["worktree", "list", "--porcelain"])?;
    let mut list = parse_worktree_porcelain(&out);
    // Ask git for the root rather than trusting the caller's spelling of
    // it. git reports worktree paths with symlinks resolved, and a shell's
    // `$PWD` often isn't: on macOS every temp dir is `/var/…` while git says
    // `/private/var/…`, and a user's `~/dev -> /Volumes/…` does the same.
    // `ypath` compares spellings, not inodes, so both sides must be git's.
    let root = repo_root(repo)?;
    mark_current(&mut list, &root.to_string_lossy());
    Ok(list)
}

/// Flag the entry that is `root`, which must be git's own `--show-toplevel`
/// answer — [`worktree_list`] fetches it — so both sides are git's spelling.
/// `ypath::same_path` still does the comparing (rule 15): git's drive-letter
/// case and separators aren't guaranteed to match its own earlier output.
pub fn mark_current(list: &mut [WorktreeEntry], root: &str) {
    for e in list.iter_mut() {
        e.current = ypath::same_path(&e.path, root);
    }
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
///
/// The flag lines (`detached`, `bare`, `locked [reason]`,
/// `prunable [reason]`) are matched as a word, so a reason after it does not
/// matter. The first entry is the main worktree.
pub fn parse_worktree_porcelain(out: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut cur: Option<WorktreeEntry> = None;
    let flag = |line: &str, word: &str| {
        line == word || line.strip_prefix(word).is_some_and(|r| r.starts_with(' '))
    };
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(done) = cur.take() {
                entries.push(done);
            }
            cur = Some(WorktreeEntry {
                path: p.to_string(),
                ..WorktreeEntry::default()
            });
            continue;
        }
        let Some(e) = cur.as_mut() else { continue };
        if let Some(b) = line.strip_prefix("branch ") {
            e.branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
        } else if let Some(h) = line.strip_prefix("HEAD ") {
            e.head = h.to_string();
        } else if flag(line, "detached") {
            e.detached = true;
        } else if flag(line, "bare") {
            e.bare = true;
        } else if flag(line, "locked") {
            e.locked = true;
        } else if flag(line, "prunable") {
            e.prunable = true;
        } else if line.is_empty() {
            if let Some(done) = cur.take() {
                entries.push(done);
            }
        }
    }
    if let Some(done) = cur.take() {
        entries.push(done);
    }
    if let Some(first) = entries.first_mut() {
        first.main = true;
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
    /// Local branches checked out in *another* worktree, → that worktree's
    /// path (git's spelling). `git checkout` of one of these fails ("already
    /// used by worktree at …"); this is what plain `git branch` marks `+ `.
    pub held: std::collections::BTreeMap<String, String>,
    /// The repository's remote names (`git remote`). A remote name may
    /// contain `/` (`team/fork`), so `team/fork/x` cannot be split into
    /// remote and branch without this list.
    pub remotes: Vec<String>,
}

/// Unit separator: between the fields of one commit.
const US: char = '\u{1f}';
/// Record separator: between commits.
const RS: char = '\u{1e}';

/// The `--format` string [`parse_log_porcelain`] expects.
///
/// The retired `ygit` TUI ran `git log --graph --oneline --decorate` and
/// parsed the *drawn ASCII graph* back out with a lane colouriser. The GUI does not inherit that: a machine
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
/// Ported verbatim from the retired `ygit` TUI. It strips exactly *one*
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
///
/// `%(worktreepath)` (git 2.23+) follows a unit separator (`%1f`): the path
/// of the worktree a local branch is checked out in, empty otherwise. It is
/// the `--format` spelling of plain `git branch`'s `+ ` marker.
pub const BRANCH_FORMAT: &str = "%(HEAD)%(refname)%1f%(worktreepath)";

/// Parse `git branch --list --all --format=`[`BRANCH_FORMAT`].
///
/// An `origin/HEAD -> origin/main` symbolic entry is dropped: it is an
/// alias, not a branch to check out. A line without the worktree field (an
/// older format) parses as a branch checked out nowhere.
pub fn parse_branch_list(out: &str) -> BranchList {
    let mut list = BranchList::default();
    for line in out.lines() {
        let (line, worktree) = line.split_once(US).unwrap_or((line, ""));
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
            } else if !worktree.is_empty() {
                // `%(HEAD)` is this worktree's HEAD, so a branch with a
                // worktree path that is not HEAD is held by another one.
                list.held.insert(local.to_string(), worktree.to_string());
            }
            list.local.push(local.to_string());
        }
        // Anything else (`refs/tags/…`, a bare name from an older call
        // site) is not a branch this pane offers.
    }
    list
}

/// Most commits one `git_log` call will fetch, however many the caller asks
/// for. The pane paginates; an unbounded request is a mistake, not a
/// feature.
pub const MAX_LOG_LIMIT: u32 = 2000;

/// Does this repository have any commits yet?
///
/// `git log` fails outright in a fresh `git init`, which is a normal state
/// and not an error the pane should show.
fn has_commits(cwd: &Path) -> bool {
    git_command(cwd)
        .args(["rev-parse", "--quiet", "--verify", "HEAD"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Commits reachable from any ref, newest first.
///
/// An empty repository is `Ok(vec![])`, not an error.
pub fn log(cwd: &Path, limit: u32, skip: u32) -> YmuxResult<Vec<CommitInfo>> {
    if !is_git_repo(cwd) {
        return Err(YmuxError::NotARepo(cwd.to_string_lossy().into_owned()));
    }
    if !has_commits(cwd) {
        return Ok(Vec::new());
    }
    let max = format!("--max-count={}", limit.clamp(1, MAX_LOG_LIMIT));
    let skip = format!("--skip={skip}");
    let fmt = format!("--format={LOG_FORMAT}");
    let out = run_git(cwd, &["log", "--all", "--date-order", &max, &skip, &fmt])?;
    Ok(parse_log_porcelain(&out))
}

/// Local and remote branches, with the current one named.
pub fn branches(cwd: &Path) -> YmuxResult<BranchList> {
    if !is_git_repo(cwd) {
        return Err(YmuxError::NotARepo(cwd.to_string_lossy().into_owned()));
    }
    let fmt = format!("--format={BRANCH_FORMAT}");
    let out = run_git(cwd, &["branch", "--list", "--all", &fmt])?;
    let mut list = parse_branch_list(&out);
    list.remotes = run_git(cwd, &["remote"])?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    if !list.held.is_empty() {
        let root = repo_root(cwd)?;
        drop_held_at(&mut list, &root.to_string_lossy());
    }
    Ok(list)
}

/// Forget "held" entries whose worktree is `root` itself.
///
/// [`parse_branch_list`] calls a branch held when it has a worktree path but
/// is not `%(HEAD)`. Mid-rebase or mid-bisect HEAD is detached, so the branch
/// being worked on has this worktree's own path and no `*` — it is not held
/// by *another* worktree, and offering "show that worktree" would point the
/// pane at itself.
pub fn drop_held_at(list: &mut BranchList, root: &str) {
    list.held.retain(|_, path| !ypath::same_path(path, root));
}

/// Check out `branch` in the worktree containing `cwd`.
///
/// This runs the repository's `post-checkout` hook and any smudge filters
/// it configures — exactly as typing `git checkout` in a terminal pane
/// would, and for the same reason: it is the user's own repository. The
/// name is validated ([`validate_ref_name`]) so it cannot be read as a git
/// option, and `Command` passes argv directly, so no shell parses it.
pub fn checkout(cwd: &Path, branch: &str) -> YmuxResult<()> {
    validate_ref_name(branch)?;
    if !is_git_repo(cwd) {
        return Err(YmuxError::NotARepo(cwd.to_string_lossy().into_owned()));
    }
    // The trailing `--` is the retired `ygit` TUI's: it ends the
    // revision list, so a branch whose name is also a file in the tree is
    // still read as the branch, never as "restore this file".
    run_git(cwd, &["checkout", branch, "--"]).map(|_| ())
}

/// Create a local branch tracking the remote-tracking `remote_branch`
/// (`origin/feature`) and check it out — `git checkout --track`.
///
/// Plain `git checkout origin/feature` would leave HEAD *detached*, and
/// anything committed there is reachable from no branch. The pane only calls
/// this when no local branch of that name exists; git refuses otherwise.
pub fn checkout_track(cwd: &Path, remote_branch: &str) -> YmuxResult<()> {
    validate_ref_name(remote_branch)?;
    if !is_git_repo(cwd) {
        return Err(YmuxError::NotARepo(cwd.to_string_lossy().into_owned()));
    }
    run_git(cwd, &["checkout", "--track", remote_branch, "--"]).map(|_| ())
}

/// One changed path from `git status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct StatusEntry {
    /// git's two-column `XY` code: `" M"`, `"A "`, `"??"`, `"R "`, …
    pub code: String,
    pub path: String,
    /// The old path of a rename or copy; empty otherwise.
    pub orig: String,
}

/// What a checkout or a worktree removal would touch — asked for just
/// before the confirmation, never on a plain refresh.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct WorkStatus {
    /// HEAD is not on a branch.
    pub detached: bool,
    /// Modified, staged, deleted, renamed and untracked paths (every
    /// untracked file, not collapsed to its directory).
    pub changes: Vec<StatusEntry>,
    /// Ignored files and directories (collapsed: `target/`, not its
    /// contents). Only filled when asked for: `git worktree remove` deletes
    /// these without `--force` and without a word.
    pub ignored: Vec<String>,
    /// On a detached HEAD: commits reachable from HEAD and from no branch,
    /// tag or remote-tracking ref — what leaving HEAD would strand (they
    /// survive only in the reflog). At most [`MAX_ORPHANS`].
    pub orphans: Vec<CommitInfo>,
    /// More than [`MAX_ORPHANS`] exist.
    pub more_orphans: bool,
}

/// Most stranded commits a [`WorkStatus`] lists.
pub const MAX_ORPHANS: usize = 50;

/// Parse `git status --porcelain=v1 -z`.
///
/// `-z` is not optional: without it git C-quotes any path with non-ASCII
/// bytes (`"\355\225\234\352\270\200.txt"`), and the confirmation would name
/// files nobody can recognise. In `-z` form a rename or copy is
/// `XY new\0old\0` — two fields, the second of which must be consumed or it
/// would be misread as the next record. `!!` (ignored) records are kept, with
/// their code, for the caller to split off.
pub fn parse_status_z(out: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    let mut fields = out.split('\0');
    while let Some(rec) = fields.next() {
        // `XY PATH`: two code columns, a space, then the path.
        if rec.len() < 4 || !rec.is_char_boundary(2) || rec.as_bytes()[2] != b' ' {
            continue;
        }
        let code = &rec[..2];
        let path = &rec[3..];
        let orig = if code.contains(['R', 'C']) {
            fields.next().unwrap_or("")
        } else {
            ""
        };
        entries.push(StatusEntry {
            code: code.to_string(),
            path: path.to_string(),
            orig: orig.to_string(),
        });
    }
    entries
}

/// Is HEAD detached? An unborn branch (fresh `git init`) is not.
fn head_detached(cwd: &Path) -> bool {
    git_command(cwd)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(false)
}

/// See [`WorkStatus`]. `include_ignored` adds one more `git status` call.
pub fn work_status(cwd: &Path, include_ignored: bool) -> YmuxResult<WorkStatus> {
    if !is_git_repo(cwd) {
        return Err(YmuxError::NotARepo(cwd.to_string_lossy().into_owned()));
    }
    let out = run_git(
        cwd,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut status = WorkStatus {
        changes: parse_status_z(&out),
        ..WorkStatus::default()
    };
    if include_ignored {
        let out = run_git(
            cwd,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=normal",
                "--ignored=matching",
            ],
        )?;
        status.ignored = parse_status_z(&out)
            .into_iter()
            .filter(|e| e.code == "!!")
            .map(|e| e.path)
            .collect();
    }
    if has_commits(cwd) && head_detached(cwd) {
        status.detached = true;
        let max = format!("--max-count={}", MAX_ORPHANS + 1);
        let fmt = format!("--format={LOG_FORMAT}");
        let out = run_git(
            cwd,
            &[
                "log",
                &max,
                &fmt,
                "HEAD",
                "--not",
                "--branches",
                "--tags",
                "--remotes",
                "--",
            ],
        )?;
        let mut orphans = parse_log_porcelain(&out);
        status.more_orphans = orphans.len() > MAX_ORPHANS;
        orphans.truncate(MAX_ORPHANS);
        status.orphans = orphans;
    }
    Ok(status)
}

/// [`repo_root`], but reporting a non-repository as [`YmuxError::NotARepo`]
/// so the pane can show its empty state rather than a git error string.
pub fn repo_root_checked(cwd: &Path) -> YmuxResult<String> {
    if !is_git_repo(cwd) {
        return Err(YmuxError::NotARepo(cwd.to_string_lossy().into_owned()));
    }
    Ok(repo_root(cwd)?.to_string_lossy().into_owned())
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

// ---------------------------------------------------------------------------
// Diff review: `git diff` against a base, plus synthesized untracked files.
// ---------------------------------------------------------------------------

/// Most files a [`DiffSummary`] lists; the rest set `truncated`.
pub const MAX_DIFF_FILES: usize = 3000;
/// Most patch bytes read for one file; the rest sets `truncated`.
pub const MAX_FILE_PATCH_BYTES: usize = 512 * 1024;
/// A diff line longer than this many chars is clipped (minified bundles).
pub const MAX_LINE_CHARS: usize = 2000;
/// Largest untracked file shown as an all-added hunk.
pub const MAX_UNTRACKED_BYTES: usize = 256 * 1024;

/// One changed file of a [`DiffSummary`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct DiffFile {
    pub path: String,
    /// The old path of a rename; empty otherwise.
    pub orig: String,
    pub added: u32,
    pub deleted: u32,
    pub binary: bool,
    pub untracked: bool,
    /// An untracked file whose line count was not taken (the summary's
    /// read budget ran out); `added` is 0 and the UI shows "?".
    pub counts_unknown: bool,
}

/// What the diff overlay lists: every file that differs from `base_rev`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct DiffSummary {
    pub root: String,
    /// The commit (or the empty tree, in an unborn repo) the working tree
    /// was compared against, as a full hash. Handed back to
    /// [`diff_file`] so both calls agree even if HEAD moves in between.
    pub base_rev: String,
    pub files: Vec<DiffFile>,
    /// More than [`MAX_DIFF_FILES`] files differ.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    Ctx,
    Add,
    Del,
    /// `\ No newline at end of file`: describes the line above it.
    Noeol,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// 1-based line number in the old file; `None` for an added line.
    pub old_no: Option<u32>,
    /// 1-based line number in the new file; `None` for a deleted line.
    pub new_no: Option<u32>,
    /// Without the `+`/`-`/space marker and without a trailing `\r`.
    pub text: String,
    /// The line ended in `\r\n`.
    pub cr: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Git's function-context text after the closing `@@`; may be empty.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct FileDiff {
    pub hunks: Vec<Hunk>,
    pub binary: bool,
    /// The patch (or the untracked file) was cut at a size cap.
    pub truncated: bool,
}

/// Parse `git diff --numstat -z`.
///
/// A plain record is `ADDED\tDELETED\tPATH\0`; a binary file shows `-\t-`
/// for the counts. A rename or copy is `ADDED\tDELETED\t\0OLD\0NEW\0`: the
/// path field after the tabs is empty and the two paths follow as their own
/// NUL-terminated fields, which must be consumed or they would be misread
/// as records.
pub fn parse_numstat_z(out: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut fields = out.split('\0');
    while let Some(rec) = fields.next() {
        let mut parts = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let binary = a == "-" && d == "-";
        let (path, orig) = if path.is_empty() {
            let old = fields.next().unwrap_or("");
            let new = fields.next().unwrap_or("");
            (new, old)
        } else {
            (path, "")
        };
        if path.is_empty() {
            continue;
        }
        files.push(DiffFile {
            path: path.to_string(),
            orig: orig.to_string(),
            added: a.parse().unwrap_or(0),
            deleted: d.parse().unwrap_or(0),
            binary,
            untracked: false,
            counts_unknown: false,
        });
    }
    files
}

/// `@@ -a[,b] +c[,d] @@ header`.
fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, header) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |s: &str| -> Option<(u32, u32)> {
        match s.split_once(',') {
            Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (old_start, old_lines) = range(old)?;
    let (new_start, new_lines) = range(new)?;
    Some(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        header: header.trim().to_string(),
        lines: Vec::new(),
    })
}

/// Clip `s` to [`MAX_LINE_CHARS`] chars, marking the cut with `…`.
fn clip_line(s: &str) -> String {
    match s.char_indices().nth(MAX_LINE_CHARS) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Parse one file's unified patch into hunks.
///
/// Everything before the first `@@` (the `diff --git`, `index`, `---`,
/// `+++` and rename headers) is skipped. Line numbers are counted from the
/// hunk header, so a patch cut mid-hunk still numbers what it has. A
/// trailing `\r` moves into [`DiffLine::cr`] so the view can show CRLF
/// files without a `^M` on every line.
pub fn parse_hunks(patch: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut in_file = false;
    for raw in patch.split('\n') {
        // A pathspec can match several files (a deleted `f` and a new
        // `f/g`): only the first section is the file asked for.
        if raw.starts_with("diff --git ") {
            if in_file || !hunks.is_empty() {
                break;
            }
            in_file = true;
            continue;
        }
        if raw.starts_with("@@ ") {
            if let Some(h) = parse_hunk_header(raw.trim_end_matches('\r')) {
                old_no = h.old_start;
                new_no = h.new_start;
                hunks.push(h);
                continue;
            }
        }
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };
        // Once the header's counts are met, only a no-newline marker can
        // still belong to the hunk.
        let full =
            old_no - hunk.old_start >= hunk.old_lines && new_no - hunk.new_start >= hunk.new_lines;
        if full && raw.as_bytes().first() != Some(&b'\\') {
            continue;
        }
        let (text, cr) = match raw.strip_suffix('\r') {
            Some(t) => (t, true),
            None => (raw, false),
        };
        let line = match raw.as_bytes().first() {
            Some(b'+') => {
                let l = DiffLine {
                    kind: DiffLineKind::Add,
                    old_no: None,
                    new_no: Some(new_no),
                    text: clip_line(&text[1..]),
                    cr,
                };
                new_no += 1;
                l
            }
            Some(b'-') => {
                let l = DiffLine {
                    kind: DiffLineKind::Del,
                    old_no: Some(old_no),
                    new_no: None,
                    text: clip_line(&text[1..]),
                    cr,
                };
                old_no += 1;
                l
            }
            Some(b'\\') => DiffLine {
                kind: DiffLineKind::Noeol,
                old_no: None,
                new_no: None,
                text: text.to_string(),
                cr: false,
            },
            _ => {
                // An empty line is a context line (an editor stripped the
                // lone space); the hunk is known not to be full here.
                let l = DiffLine {
                    kind: DiffLineKind::Ctx,
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                    text: clip_line(text.get(1..).unwrap_or("")),
                    cr,
                };
                old_no += 1;
                new_no += 1;
                l
            }
        };
        hunk.lines.push(line);
    }
    hunks
}

/// The patch up to (not including) the second `diff --git` line.
fn first_section(patch: &str) -> &str {
    match patch.find("\ndiff --git ") {
        Some(i) => &patch[..i],
        None => patch,
    }
}

/// Line count of one untracked file, from a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UntrackedCount {
    Text(u32),
    Binary,
    /// The shared read budget ran out first.
    Unknown,
}

/// Bytes the untracked files of one summary may cost in reads, in total.
const SUMMARY_UNTRACKED_BUDGET: u64 = 16 * 1024 * 1024;
/// Leading bytes sniffed for a NUL.
const BINARY_SNIFF: usize = 8 * 1024;

/// Count the lines of `r` without holding it: sniff the first 8 KiB for a
/// NUL, then stream the rest, charging every byte read to `budget`.
fn count_untracked<R: std::io::Read>(mut r: R, budget: &mut u64) -> UntrackedCount {
    let mut head = vec![0u8; BINARY_SNIFF];
    let mut n = 0;
    while n < head.len() {
        match r.read(&mut head[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    if n == 0 {
        return UntrackedCount::Text(0);
    }
    let mut charge = |k: usize| -> bool {
        if k as u64 > *budget {
            *budget = 0;
            return false;
        }
        *budget -= k as u64;
        true
    };
    if !charge(n) {
        return UntrackedCount::Unknown;
    }
    if head[..n].contains(&0u8) {
        return UntrackedCount::Binary;
    }
    let nl = |b: &[u8]| b.iter().filter(|c| **c == b'\n').count() as u32;
    let mut lines = nl(&head[..n]);
    let mut last = head[n - 1];
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(k) => {
                if !charge(k) {
                    return UntrackedCount::Unknown;
                }
                lines = lines.saturating_add(nl(&buf[..k]));
                last = buf[k - 1];
            }
        }
    }
    UntrackedCount::Text(lines + u32::from(last != b'\n'))
}

/// The all-added diff of an untracked file whose first `bytes` we hold.
/// `total_len` is its size on disk; more than [`MAX_UNTRACKED_BYTES`] cuts
/// the text at the last whole line and sets `truncated`.
pub fn untracked_diff_from_bytes(bytes: &[u8], total_len: u64) -> FileDiff {
    if crate::fsx::is_probably_binary(bytes) {
        return FileDiff {
            binary: true,
            ..FileDiff::default()
        };
    }
    let truncated = total_len > MAX_UNTRACKED_BYTES as u64 || bytes.len() > MAX_UNTRACKED_BYTES;
    let mut bytes = &bytes[..bytes.len().min(MAX_UNTRACKED_BYTES)];
    if truncated {
        if let Some(i) = bytes.iter().rposition(|b| *b == b'\n') {
            bytes = &bytes[..=i];
        }
    }
    let text = String::from_utf8_lossy(bytes);
    if text.is_empty() {
        return FileDiff {
            truncated,
            ..FileDiff::default()
        };
    }
    let ends_nl = text.ends_with('\n');
    let body = text.strip_suffix('\n').unwrap_or(&text);
    let mut lines: Vec<DiffLine> = body
        .split('\n')
        .enumerate()
        .map(|(i, l)| {
            let (t, cr) = match l.strip_suffix('\r') {
                Some(t) => (t, true),
                None => (l, false),
            };
            DiffLine {
                kind: DiffLineKind::Add,
                old_no: None,
                new_no: Some(i as u32 + 1),
                text: clip_line(t),
                cr,
            }
        })
        .collect();
    let n = lines.len() as u32;
    if !ends_nl && !truncated {
        lines.push(DiffLine {
            kind: DiffLineKind::Noeol,
            old_no: None,
            new_no: None,
            text: "\\ No newline at end of file".to_string(),
            cr: false,
        });
    }
    FileDiff {
        hunks: vec![Hunk {
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: n,
            header: String::new(),
            lines,
        }],
        binary: false,
        truncated,
    }
}

/// Is `rel` a path an untracked-file read may follow under `root`?
/// Relative, no `..`, and — by `fsx::within_each`, the boundary test the
/// file dock uses — inside `root` once joined.
fn untracked_path_ok(root: &Path, rel: &str) -> bool {
    let p = Path::new(rel);
    if rel.is_empty() || p.is_absolute() || rel.contains('\0') {
        return false;
    }
    if rel.split(['/', '\\']).any(|c| c == "..") {
        return false;
    }
    let joined = root.join(p).to_string_lossy().into_owned();
    crate::fsx::within_each(&root.to_string_lossy(), &[joined])[0]
}

/// What an untracked path is, once it is known to stay inside the repo.
enum UntrackedSrc {
    /// A symlink, shown as its target text and never followed.
    Link(String),
    File(PathBuf, u64),
    Other,
}

/// Does `rel` under `root` stay inside `root` after resolving symlinks in
/// its *parent* directories? The leaf itself is not followed (a symlink
/// leaf is shown as text), so only the parent is canonicalized.
fn parent_stays_inside(root: &Path, full: &Path) -> bool {
    let (Ok(canon_root), Some(name), Some(parent)) =
        (root.canonicalize(), full.file_name(), full.parent())
    else {
        return false;
    };
    let Ok(canon_parent) = parent.canonicalize() else {
        return false;
    };
    crate::fsx::is_within(
        &canon_root.to_string_lossy(),
        &canon_parent.join(name).to_string_lossy(),
    )
}

fn classify_untracked(root: &Path, rel: &str) -> YmuxResult<UntrackedSrc> {
    if !untracked_path_ok(root, rel) || !parent_stays_inside(root, &root.join(rel)) {
        return Err(YmuxError::Git(format!(
            "path outside the repository: {rel}"
        )));
    }
    let full = root.join(rel);
    let meta = std::fs::symlink_metadata(&full).map_err(YmuxError::Io)?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&full).map_err(YmuxError::Io)?;
        return Ok(UntrackedSrc::Link(target.to_string_lossy().into_owned()));
    }
    if !meta.is_file() {
        return Ok(UntrackedSrc::Other);
    }
    Ok(UntrackedSrc::File(full, meta.len()))
}

/// Read an untracked file for [`untracked_diff_from_bytes`]. A symlink is
/// shown as its target text and never followed (it may point out of `root`).
fn read_untracked(root: &Path, rel: &str) -> YmuxResult<FileDiff> {
    match classify_untracked(root, rel)? {
        UntrackedSrc::Link(t) => Ok(untracked_diff_from_bytes(
            format!("{t}\n").as_bytes(),
            t.len() as u64 + 1,
        )),
        UntrackedSrc::Other => Ok(FileDiff::default()),
        UntrackedSrc::File(full, len) => {
            use std::io::Read;
            let mut buf = Vec::new();
            std::fs::File::open(&full)
                .map_err(YmuxError::Io)?
                .take(MAX_UNTRACKED_BYTES as u64 + 1)
                .read_to_end(&mut buf)
                .map_err(YmuxError::Io)?;
            Ok(untracked_diff_from_bytes(&buf, len))
        }
    }
}

/// [`count_untracked`] for one path of the summary: same containment and
/// symlink rules as [`read_untracked`], but streamed under `budget`.
fn count_untracked_file(root: &Path, rel: &str, budget: &mut u64) -> YmuxResult<UntrackedCount> {
    Ok(match classify_untracked(root, rel)? {
        UntrackedSrc::Link(_) => UntrackedCount::Text(1),
        UntrackedSrc::Other => UntrackedCount::Text(0),
        UntrackedSrc::File(full, _) => {
            count_untracked(std::fs::File::open(&full).map_err(YmuxError::Io)?, budget)
        }
    })
}

/// The hash of the empty tree. Asked of git rather than hardcoded: a
/// SHA-256 repository has a different one.
fn empty_tree(cwd: &Path) -> YmuxResult<String> {
    let out = git_command(cwd)
        .args(["hash-object", "-t", "tree", "--stdin"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(YmuxError::Io)?;
    if !out.status.success() {
        return Err(YmuxError::Git(format!(
            "git hash-object: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The commit a diff is taken against, as a full hash.
///
/// Empty `base` is HEAD — or the empty tree in an unborn repository, so
/// everything staged shows as added. A named `base` is the merge-base of it
/// and HEAD: the changes *this* branch made, not the base's own progress.
pub fn resolve_base(cwd: &Path, base: &str) -> YmuxResult<String> {
    let base = base.trim();
    if base.is_empty() {
        if !has_commits(cwd) {
            return empty_tree(cwd);
        }
        return Ok(run_git(cwd, &["rev-parse", "--verify", "HEAD"])?
            .trim()
            .to_string());
    }
    validate_ref_name(base)?;
    Ok(run_git(cwd, &["merge-base", base, "HEAD"])?
        .trim()
        .to_string())
}

/// The pinned prefix of every diff call: unquoted paths, no pathspec magic,
/// no external diff driver or textconv (both run user-configured programs),
/// fixed prefixes whatever `diff.noprefix` says.
fn diff_args<'a>(ignore_cr: bool, rest: &[&'a str]) -> Vec<&'a str> {
    let mut a = vec![
        "-c",
        "core.quotepath=off",
        "--literal-pathspecs",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "-M",
    ];
    if ignore_cr {
        a.push("--ignore-cr-at-eol");
    }
    a.extend_from_slice(rest);
    a
}

/// Files that differ between `base` (see [`resolve_base`]) and the working
/// tree, untracked ones included.
pub fn diff_summary(cwd: &Path, base: &str, ignore_cr: bool) -> YmuxResult<DiffSummary> {
    let root = repo_root_checked(cwd)?;
    let rootp = Path::new(&root);
    let base_rev = resolve_base(rootp, base)?;
    let out = run_git(
        rootp,
        &diff_args(ignore_cr, &["--numstat", "-z", &base_rev, "--"]),
    )?;
    let mut files = parse_numstat_z(&out);
    let others = run_git(rootp, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut budget = SUMMARY_UNTRACKED_BUDGET;
    for rel in others.split('\0').filter(|p| !p.is_empty()) {
        // A nested repository is listed as `dir/`; it has no content to show.
        if rel.ends_with('/') || files.len() > MAX_DIFF_FILES {
            continue;
        }
        let mut f = DiffFile {
            path: rel.to_string(),
            untracked: true,
            ..DiffFile::default()
        };
        match count_untracked_file(rootp, rel, &mut budget) {
            Ok(UntrackedCount::Text(n)) => f.added = n,
            Ok(UntrackedCount::Binary) => f.binary = true,
            Ok(UntrackedCount::Unknown) => f.counts_unknown = true,
            Err(_) => {}
        }
        files.push(f);
    }
    let truncated = files.len() > MAX_DIFF_FILES;
    files.truncate(MAX_DIFF_FILES);
    Ok(DiffSummary {
        root,
        base_rev,
        files,
        truncated,
    })
}

/// Run git and keep at most `cap` bytes of stdout; the rest is dropped and
/// git is killed. Returns whether the output was cut.
fn run_git_capped(cwd: &Path, args: &[&str], cap: usize) -> YmuxResult<(Vec<u8>, bool)> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = git_command(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(YmuxError::Io)?;
    let mut buf = Vec::new();
    if let Some(out) = child.stdout.take() {
        out.take(cap as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(YmuxError::Io)?;
    }
    let cut = buf.len() > cap;
    if cut {
        let _ = child.kill();
        buf.truncate(cap);
    }
    let status = child.wait().map_err(YmuxError::Io)?;
    if !cut && !status.success() {
        return Err(YmuxError::Git(format!("git diff failed ({status})")));
    }
    Ok((buf, cut))
}

/// Is `s` a hash [`DiffSummary::base_rev`] could have produced?
fn is_hex_rev(s: &str) -> bool {
    (4..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// One file's hunks against `base_rev`. `orig` is passed as a second
/// pathspec so a rename is shown as one; `untracked` files are read from
/// disk instead.
pub fn diff_file(
    cwd: &Path,
    base_rev: &str,
    path: &str,
    orig: &str,
    untracked: bool,
    ignore_cr: bool,
) -> YmuxResult<FileDiff> {
    let root = repo_root(cwd)?;
    if untracked {
        return read_untracked(&root, path);
    }
    if !is_hex_rev(base_rev) {
        return Err(YmuxError::Git(format!("not a commit hash: {base_rev}")));
    }
    if path.is_empty() || path.contains('\0') || orig.contains('\0') {
        return Err(YmuxError::Git("bad path".into()));
    }
    let mut rest = vec!["-U3", base_rev, "--", path];
    if !orig.is_empty() {
        rest.push(orig);
    }
    let (bytes, truncated) =
        run_git_capped(&root, &diff_args(ignore_cr, &rest), MAX_FILE_PATCH_BYTES)?;
    let mut patch = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        if let Some(i) = patch.rfind('\n') {
            patch.truncate(i + 1);
        }
    }
    // The pathspec may also match `path/...`; keep the file asked for.
    let patch = first_section(&patch);
    let hunks = parse_hunks(patch);
    let binary = hunks.is_empty() && patch.lines().any(|l| l.starts_with("Binary files "));
    Ok(FileDiff {
        hunks,
        binary,
        truncated,
    })
}

/// Which of `cwds` (pane id, shell cwd) work in the worktree `root`?
///
/// Each cwd belongs to the **most specific** worktree containing it — the
/// longest comparison key — because a linked worktree is often nested under
/// the main one (`repo/.worktrees/x`), and a pane there is not a pane of
/// `repo`. Containment and the final equality are `ypath`'s (rule 15).
pub fn pick_agent_panes(
    root: &str,
    worktrees: &[String],
    cwds: &[(uuid::Uuid, String)],
) -> Vec<uuid::Uuid> {
    cwds.iter()
        .filter(|(_, cwd)| {
            worktrees
                .iter()
                .filter(|wt| crate::fsx::is_within(wt, cwd))
                .max_by_key(|wt| ypath::comparison_key(wt).len())
                .is_some_and(|wt| ypath::same_path(wt, root))
        })
        .map(|(id, _)| *id)
        .collect()
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

    // The next three are the retired `ygit` TUI's tests, kept verbatim as a
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
        assert!(list[1].detached);
        assert_eq!(list[1].head, "def");
    }

    /// Every flag line git 2.52 writes, with and without a reason, and the
    /// first entry marked as the main worktree.
    #[test]
    fn porcelain_flags_and_the_main_worktree() {
        let out = "\
worktree /r
HEAD 1111
branch refs/heads/main

worktree /wt/locked
HEAD 2222
branch refs/heads/a
locked

worktree /wt/locked-why
HEAD 3333
detached
locked moved to a USB disk

worktree /wt/gone
HEAD 4444
branch refs/heads/b
prunable gitdir file points to non-existent location

worktree /wt/branch-named-lockedish
HEAD 5555
branch refs/heads/lockedish
";
        let list = parse_worktree_porcelain(out);
        assert_eq!(list.len(), 5);
        assert!(list[0].main && !list[1].main && !list[4].main);
        assert_eq!(list[0].head, "1111");
        assert!(list[1].locked && !list[1].detached);
        assert!(
            list[2].locked && list[2].detached,
            "a reason after the flag"
        );
        assert!(list[3].prunable && !list[3].locked);
        // Flags are words: `branch refs/heads/lockedish` is not `locked`.
        assert!(!list[4].locked && !list[4].prunable);
        assert_eq!(list[4].branch, "lockedish");
    }

    /// `current` crosses the spellings rule 15 is about: git's forward
    /// slashes and drive case against a backslashed path.
    #[test]
    fn worktree_list_finds_current_from_a_path_git_spells_differently() {
        if !git_available() {
            return;
        }
        // A subdirectory is the portable stand-in for macOS's `/var` ->
        // `/private/var`: the path handed in is not git's spelling of the
        // root, and the current worktree must still be recognised.
        let repo = init_test_repo("spelled");
        let sub = repo.join("한글 하위");
        std::fs::create_dir_all(&sub).unwrap();
        let wts = worktree_list(&sub).unwrap();
        assert_eq!(wts.len(), 1);
        assert!(wts[0].main && wts[0].current, "{wts:?}");
    }

    #[test]
    fn mark_current_compares_paths_by_key_not_bytes() {
        let mut list = parse_worktree_porcelain(
            "worktree C:/Repo\nHEAD 1\nbranch refs/heads/main\n\nworktree C:/wt/한글 트리\nHEAD 2\nbranch refs/heads/x\n",
        );
        mark_current(&mut list, r"c:\wt\한글 트리");
        assert!(!list[0].current);
        assert!(list[1].current);
    }

    /// The `-z` status format: a rename is two NUL fields (new, then old),
    /// Hangul and spaces arrive unquoted, and ignored records keep `!!`.
    #[test]
    fn status_z_parses_renames_hangul_and_ignored() {
        let out = concat!(
            " M src/main.rs\0",
            "R  새 이름.txt\0old name.txt\0",
            "?? 한글 폴더/메모.md\0",
            "A  added.rs\0",
            "!! target/\0",
        );
        let got = parse_status_z(out);
        assert_eq!(got.len(), 5, "{got:?}");
        assert_eq!(got[0].code, " M");
        assert_eq!(got[0].path, "src/main.rs");
        assert_eq!(got[1].code, "R ");
        assert_eq!(got[1].path, "새 이름.txt");
        assert_eq!(got[1].orig, "old name.txt");
        // The old name was consumed, not misread as a record.
        assert_eq!(got[2].path, "한글 폴더/메모.md");
        assert_eq!(got[3].code, "A ");
        assert_eq!(got[4].code, "!!");
        assert_eq!(got[4].path, "target/");
        // Total: nothing, and junk.
        assert!(parse_status_z("").is_empty());
        assert!(parse_status_z("\0\0x\0").is_empty());
    }

    /// Mid-rebase or mid-bisect HEAD is detached, so `%(HEAD)` marks nothing,
    /// yet git still reports the branch being rebased at *this* worktree's
    /// path. That is not "held by another worktree" — dropped by comparing
    /// with the root through `ypath` (git's `C:/` spelling vs ours).
    #[test]
    fn a_branch_at_this_worktrees_own_path_is_not_held() {
        let out = "\
 refs/heads/rebasing\u{1f}C:/Repo/한
 refs/heads/elsewhere\u{1f}C:/wt/other
";
        let mut list = parse_branch_list(out);
        assert_eq!(list.held.len(), 2);
        drop_held_at(&mut list, r"c:\repo\한");
        assert_eq!(list.held.len(), 1);
        assert!(list.held.contains_key("elsewhere"));
        assert_eq!(list.current, "", "still detached: no current branch");
    }

    /// `%(worktreepath)` marks a branch another worktree holds; the current
    /// branch's own worktree path does not count as "held".
    #[test]
    fn branch_list_records_branches_held_by_other_worktrees() {
        let out = "\
*refs/heads/main\u{1f}C:/repo
 refs/heads/워크트리브랜치\u{1f}C:/tmp/한글 워크트리
 refs/heads/free\u{1f}
 refs/remotes/origin/main\u{1f}
";
        let list = parse_branch_list(out);
        assert_eq!(list.current, "main");
        assert_eq!(list.local, vec!["main", "워크트리브랜치", "free"]);
        assert_eq!(list.held.len(), 1);
        assert_eq!(
            list.held.get("워크트리브랜치").map(String::as_str),
            Some("C:/tmp/한글 워크트리")
        );
        assert_eq!(list.remote, vec!["origin/main"]);
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

    /// Against a real `git`, because the point is that [`LOG_FORMAT`] and
    /// [`BRANCH_FORMAT`] are what the installed git actually accepts — a
    /// fixture cannot catch a format string git rejects.
    #[test]
    fn log_branches_and_checkout_against_real_git() {
        let repo = init_test_repo("log_round_trip");

        let run = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .expect("run git");
            assert!(
                out.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        // A branch with a Korean name and a subject containing a pipe and a
        // unit-separator-adjacent character.
        run(&["checkout", "-q", "-b", "기능/한글브랜치"]);
        std::fs::write(repo.join("g.txt"), "x\n").unwrap();
        run(&["add", "g.txt"]);
        run(&["commit", "-q", "-m", "두 번째 | pipe in subject"]);

        let commits = log(&repo, 10, 0).unwrap();
        assert_eq!(commits.len(), 2, "both commits come back");
        assert_eq!(commits[0].subject, "두 번째 | pipe in subject");
        assert_eq!(commits[0].author, "Test");
        assert_eq!(commits[0].parents.len(), 1);
        assert_eq!(commits[0].parents[0], commits[1].hash);
        assert!(commits[0].date_ms > 0);
        assert!(commits[0].hash.starts_with(&commits[0].short));

        // `skip` and `limit` are honoured.
        assert_eq!(log(&repo, 1, 0).unwrap().len(), 1);
        let skipped = log(&repo, 10, 1).unwrap();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].hash, commits[1].hash);

        let list = branches(&repo).unwrap();
        assert_eq!(list.current, "기능/한글브랜치");
        assert!(list.local.contains(&"기능/한글브랜치".to_string()));
        assert!(list.remote.is_empty(), "no remotes in a fresh init");

        // Checkout round-trips, including back to a Korean branch name.
        let other = list
            .local
            .iter()
            .find(|b| *b != "기능/한글브랜치")
            .expect("init_test_repo leaves a default branch")
            .clone();
        checkout(&repo, &other).unwrap();
        assert_eq!(branches(&repo).unwrap().current, other);
        checkout(&repo, "기능/한글브랜치").unwrap();
        assert_eq!(branches(&repo).unwrap().current, "기능/한글브랜치");

        // A name git would read as an option never reaches git.
        assert!(checkout(&repo, "--help").is_err());

        cleanup_dir(&repo);
    }

    /// A fresh `git init` has no commits, which `git log` treats as a fatal
    /// error. The pane's empty state is not an error.
    #[test]
    fn an_empty_repository_logs_as_empty_not_as_an_error() {
        let dir = std::env::temp_dir().join(format!(
            "ymux_git_test_empty_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Command::new("git")
            .current_dir(&dir)
            .args(["init", "-q"])
            .output()
            .unwrap()
            .status
            .success());

        assert_eq!(log(&dir, 10, 0).unwrap(), Vec::new());
        assert_eq!(branches(&dir).unwrap(), BranchList::default());
        assert!(repo_root_checked(&dir).is_ok());

        cleanup_dir(&dir);
    }

    /// Outside a repository every one of these is `NotARepo`, so the pane
    /// can branch on it instead of matching a git error string.
    #[test]
    fn outside_a_repository_everything_is_not_a_repo() {
        let dir = std::env::temp_dir().join(format!(
            "ymux_git_test_norepo_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // A temp dir can itself sit inside a repository on some machines;
        // only assert when git agrees it does not.
        if !is_git_repo(&dir) {
            assert_eq!(log(&dir, 10, 0).unwrap_err().kind(), "not_a_repo");
            assert_eq!(branches(&dir).unwrap_err().kind(), "not_a_repo");
            assert_eq!(checkout(&dir, "main").unwrap_err().kind(), "not_a_repo");
            assert_eq!(repo_root_checked(&dir).unwrap_err().kind(), "not_a_repo");
        }

        cleanup_dir(&dir);
    }

    #[test]
    fn untagged_error_names_the_worktree_path() {
        let e = untagged_error(Path::new("/w/lane"), &YmuxError::Git("boom".into()));
        let msg = e.to_string();
        assert!(msg.contains("worktree created but not tagged: "), "{msg}");
        assert!(msg.contains("lane") && msg.contains("boom"), "{msg}");
    }

    #[test]
    fn commits_ahead_counts_and_validates_base() {
        if !git_available() {
            eprintln!("skipping commits_ahead test: git not on PATH");
            return;
        }
        let repo = init_test_repo("ahead");
        let base = git_ok(&repo, &["rev-parse", "HEAD"]).trim().to_string();
        assert_eq!(commits_ahead(&repo, &base).unwrap(), 0);
        for i in 0..3 {
            std::fs::write(repo.join(format!("a{i}.txt")), "x\n").unwrap();
            git_ok(&repo, &["add", "."]);
            git_ok(&repo, &["commit", "-q", "-m", &format!("c{i}")]);
        }
        assert_eq!(commits_ahead(&repo, &base).unwrap(), 3);
        // Past the old 100-commit log window the count is still exact.
        assert_eq!(commits_ahead(&repo, &base[..7]).unwrap(), 3);
        // Anything but a hex sha never reaches git.
        for bad in [
            "",
            "abc",
            "--all",
            "HEAD",
            "main",
            "zzzzzzz",
            "abcd..",
            "abcd efgh",
        ] {
            assert!(commits_ahead(&repo, bad).is_err(), "{bad}");
        }
        // Unknown but well-formed sha: git's error, not a count.
        assert!(commits_ahead(&repo, "deadbeefdeadbeef").is_err());
        cleanup_dir(&repo);
    }

    #[test]
    fn fanout_lane_meta_branch_exists_and_delete_rules() {
        if !git_available() {
            eprintln!("skipping fanout lane test: git not on PATH");
            return;
        }
        let repo = init_test_repo("lane");
        let wt_base = PathBuf::from(format!("{}_wt", repo.display()));
        let head = git_ok(&repo, &["rev-parse", "HEAD"]).trim().to_string();
        let main_branch = git_ok(&repo, &["symbolic-ref", "--short", "HEAD"])
            .trim()
            .to_string();

        // Create a lane from a ref name; meta stores the resolved sha.
        let path = suggested_worktree_path(&repo, "lane/a", &wt_base.to_string_lossy());
        worktree_add_lane(&repo, "lane/a", &path, &main_branch, "run-1").expect("create lane");
        let meta = lane_meta(&path).unwrap();
        assert_eq!(meta.base, head);
        assert_eq!(meta.fanout, "run-1");
        // The main worktree's branch has no meta.
        assert_eq!(lane_meta(&repo).unwrap(), LaneMeta::default());

        // An existing branch is an error, and nothing is created.
        let path2 = suggested_worktree_path(&repo, "lane/b", &wt_base.to_string_lossy());
        let err = worktree_add_new(&repo, "lane/a", &path2, &head).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert!(!path2.exists());
        // Bad names / run ids are refused before git runs.
        assert!(worktree_add_new(&repo, "-x", &path2, &head).is_err());
        assert!(worktree_add_new(&repo, "lane/b", &path2, "--force").is_err());
        assert!(worktree_add_lane(&repo, "lane/b", &path2, &head, "bad id!").is_err());
        assert!(!path2.exists());
        // A full hex sha is accepted as `start`.
        worktree_add_new(&repo, "lane/c", &path2, &head).expect("sha start");

        // An unmerged branch survives `-d`.
        std::fs::write(path.join("g.txt"), "x\n").unwrap();
        git_ok(&path, &["add", "g.txt"]);
        git_ok(&path, &["commit", "-q", "-m", "work"]);
        worktree_remove(&path, false).unwrap();
        assert!(branch_delete(&repo, "lane/a").is_err());
        assert!(git_ok(&repo, &["branch", "--list", "lane/a"]).contains("lane/a"));
        // The current branch is refused outright.
        let err = branch_delete(&repo, &main_branch).unwrap_err();
        assert!(err.to_string().contains("current branch"), "{err}");
        // A merged one goes.
        worktree_remove(&path2, false).unwrap();
        branch_delete(&repo, "lane/c").expect("merged branch deletes");

        cleanup_dir(&repo);
        cleanup_dir(&wt_base);
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

    /// Run git in `dir` for a test's setup, asserting it succeeds.
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

    /// A sibling of `repo` for a worktree or clone, removed by the test.
    fn sibling(repo: &Path, suffix: &str) -> PathBuf {
        repo.with_file_name(format!(
            "{}_{suffix}",
            repo.file_name().unwrap().to_string_lossy()
        ))
    }

    /// ygit's `+ ` bug, against real git: a Korean branch checked out in a
    /// linked worktree at a Korean path with a space, while the main worktree
    /// sits on a detached HEAD. The branch list names it cleanly (no `+ `,
    /// no `(HEAD detached …)` pseudo-branch), records who holds it, and a
    /// checkout of it fails as a git error instead of doing anything odd.
    #[test]
    fn held_branch_and_detached_head_against_real_git() {
        if !git_available() {
            return;
        }
        let repo = init_test_repo("held");
        let wt = sibling(&repo, "wt");
        let wt_path = wt.join("한글 워크트리");
        worktree_add(&repo, "워크트리브랜치", &wt_path).expect("worktree add");
        git_ok(&repo, &["checkout", "-q", "--detach"]);

        let list = branches(&repo).unwrap();
        assert_eq!(list.current, "", "detached: no current branch");
        assert!(
            list.local
                .iter()
                .all(|b| !b.starts_with('(') && !b.starts_with('+')),
            "{list:?}"
        );
        assert!(list.local.contains(&"워크트리브랜치".to_string()));
        let held = list
            .held
            .get("워크트리브랜치")
            .expect("held by the linked worktree");
        assert!(worktree_leaf_is(held, &wt_path), "{held}");
        assert_eq!(list.held.len(), 1, "{list:?}");

        let wts = worktree_list(&repo).unwrap();
        assert_eq!(wts.len(), 2);
        assert!(wts[0].main && wts[0].current && wts[0].detached);
        assert!(!wts[1].main && !wts[1].current && !wts[1].detached);
        assert_eq!(wts[1].branch, "워크트리브랜치");
        assert!(worktree_leaf_is(&wts[1].path, &wt_path));
        // Asked from inside the linked worktree, *it* is the current one.
        let from_wt = worktree_list(&repo_root(&wt_path).unwrap()).unwrap();
        assert!(!from_wt[0].current && from_wt[1].current, "{from_wt:?}");

        let err = checkout(&repo, "워크트리브랜치").unwrap_err();
        assert_eq!(err.kind(), "git", "{err}");

        worktree_remove(&wt_path, false).expect("cleanup");
        cleanup_dir(&repo);
        cleanup_dir(&wt);
    }

    /// What the confirmations are built from: the changes a checkout carries
    /// (Hangul unquoted, a rename with its old name), and on a detached HEAD
    /// the commits no ref reaches.
    #[test]
    fn work_status_lists_changes_and_stranded_commits() {
        if !git_available() {
            return;
        }
        let repo = init_test_repo("status");
        let clean = work_status(&repo, false).unwrap();
        assert!(clean.changes.is_empty() && !clean.detached && clean.orphans.is_empty());

        git_ok(&repo, &["checkout", "-q", "--detach"]);
        std::fs::write(repo.join("h.txt"), "x\n").unwrap();
        git_ok(&repo, &["add", "h.txt"]);
        git_ok(&repo, &["commit", "-q", "-m", "떠돌이 커밋"]);
        std::fs::write(repo.join("메모 파일.txt"), "x\n").unwrap();
        git_ok(&repo, &["mv", "f.txt", "새 f.txt"]);

        let s = work_status(&repo, false).unwrap();
        assert!(s.detached);
        assert_eq!(s.orphans.len(), 1);
        assert_eq!(s.orphans[0].subject, "떠돌이 커밋");
        assert!(!s.more_orphans);
        let untracked = s
            .changes
            .iter()
            .find(|e| e.code == "??")
            .expect("untracked");
        assert_eq!(untracked.path, "메모 파일.txt");
        let renamed = s
            .changes
            .iter()
            .find(|e| e.code.starts_with('R'))
            .expect("rename");
        assert_eq!(
            (renamed.path.as_str(), renamed.orig.as_str()),
            ("새 f.txt", "f.txt")
        );

        cleanup_dir(&repo);
    }

    /// Why the removal confirmation lists ignored files: a *non-forced*
    /// `git worktree remove` of a worktree with no changes still deletes its
    /// ignored build output, without asking.
    #[test]
    fn worktree_remove_deletes_ignored_files_without_force() {
        if !git_available() {
            return;
        }
        let repo = init_test_repo("ignored");
        let wt = sibling(&repo, "wt");
        let wt_path = wt.join("w");
        worktree_add(&repo, "agent/ign", &wt_path).expect("worktree add");
        // `info/exclude` lives in the common dir, so it applies to both.
        std::fs::write(repo.join(".git").join("info").join("exclude"), "target/\n").unwrap();
        std::fs::create_dir_all(wt_path.join("target")).unwrap();
        std::fs::write(wt_path.join("target").join("out.bin"), "build\n").unwrap();

        let s = work_status(&wt_path, true).unwrap();
        assert!(s.changes.is_empty(), "{s:?}");
        assert_eq!(s.ignored, vec!["target/"]);

        worktree_remove(&wt_path, false).expect("git removes it without --force");
        assert!(!wt_path.join("target").join("out.bin").exists());

        cleanup_dir(&repo);
        cleanup_dir(&wt);
    }

    /// What the removal dialog must say about a detached worktree's own
    /// commits: its HEAD reflog lives in the worktree's admin dir
    /// (`.git/worktrees/<id>/logs/HEAD`), which `git worktree remove`
    /// deletes. Unlike a checkout, the reflog cannot bring them back — the
    /// objects merely linger, reachable from nothing, until gc.
    #[test]
    fn removing_a_detached_worktree_takes_its_reflog_with_it() {
        if !git_available() {
            return;
        }
        let repo = init_test_repo("strand");
        let wt = sibling(&repo, "wt");
        let wt_path = wt.join("d");
        worktree_add(&repo, "agent/strand", &wt_path).expect("worktree add");
        git_ok(&wt_path, &["checkout", "-q", "--detach"]);
        std::fs::write(wt_path.join("s.txt"), "x\n").unwrap();
        git_ok(&wt_path, &["add", "s.txt"]);
        git_ok(&wt_path, &["commit", "-q", "-m", "stranded"]);
        let hash = git_ok(&wt_path, &["rev-parse", "HEAD"]).trim().to_string();

        let s = work_status(&wt_path, true).unwrap();
        assert!(s.detached);
        assert_eq!(s.orphans.len(), 1);

        let admin = repo.join(".git").join("worktrees");
        assert!(admin.read_dir().unwrap().next().is_some());
        worktree_remove(&wt_path, false).expect("clean, so no force needed");

        assert!(
            admin
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(true),
            "the worktree's admin dir — its HEAD reflog — is gone"
        );
        let all = git_ok(&repo, &["log", "--all", "--reflog", "--format=%H"]);
        assert!(!all.contains(&hash), "no ref or reflog reaches it any more");
        git_ok(&repo, &["cat-file", "-e", &hash]);

        cleanup_dir(&repo);
        cleanup_dir(&wt);
    }

    /// `checkout_track` makes a local branch from a remote one instead of
    /// detaching HEAD on `origin/<name>`.
    #[test]
    fn checkout_track_creates_a_local_tracking_branch() {
        if !git_available() {
            return;
        }
        let repo = init_test_repo("track");
        git_ok(&repo, &["branch", "기능/원격"]);
        let clone = sibling(&repo, "clone");
        git_ok(
            repo.parent().unwrap(),
            &[
                "clone",
                "-q",
                &repo.to_string_lossy(),
                &clone.to_string_lossy(),
            ],
        );
        let before = branches(&clone).unwrap();
        assert!(
            before.remote.contains(&"origin/기능/원격".to_string()),
            "{before:?}"
        );
        assert!(!before.local.contains(&"기능/원격".to_string()));
        assert_eq!(before.remotes, vec!["origin"]);

        // A remote whose own name has a slash: the pane can only split
        // `team/fork/기능/원격` correctly if it knows the remote names.
        git_ok(
            &clone,
            &["remote", "add", "team/fork", &repo.to_string_lossy()],
        );
        git_ok(&clone, &["fetch", "-q", "team/fork"]);
        let forked = branches(&clone).unwrap();
        assert_eq!(forked.remotes, vec!["origin", "team/fork"]);
        assert!(forked.remote.contains(&"team/fork/기능/원격".to_string()));

        checkout_track(&clone, "origin/기능/원격").unwrap();
        let after = branches(&clone).unwrap();
        assert_eq!(after.current, "기능/원격");
        assert!(!work_status(&clone, false).unwrap().detached);
        assert!(checkout_track(&clone, "--orphan").is_err());

        cleanup_dir(&clone);
        cleanup_dir(&repo);
    }

    // --- Diff review -------------------------------------------------------

    #[test]
    fn numstat_plain_binary_and_odd_paths() {
        let out = "3\t1\tsrc/a.rs\0-\t-\tlogo.png\0\
                   2\t0\t한글 dir/파일 이름.txt\0\
                   0\t0\tdir with space/x y.md\0";
        let f = parse_numstat_z(out);
        assert_eq!(f.len(), 4);
        assert_eq!(
            (f[0].path.as_str(), f[0].added, f[0].deleted, f[0].binary),
            ("src/a.rs", 3, 1, false)
        );
        assert!(f[1].binary);
        assert_eq!((f[1].added, f[1].deleted), (0, 0));
        assert_eq!(f[2].path, "한글 dir/파일 이름.txt");
        assert_eq!(f[3].path, "dir with space/x y.md");
        assert!(f.iter().all(|d| d.orig.is_empty() && !d.untracked));
    }

    #[test]
    fn numstat_rename_consumes_both_path_fields() {
        // The rename's two paths must not be misread as records, and the
        // record after it must still parse.
        let out = "5\t2\t\0old/이름 a.txt\0new/이름 b.txt\x001\t1\tz.txt\0";
        let f = parse_numstat_z(out);
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(f[0].path, "new/이름 b.txt");
        assert_eq!(f[0].orig, "old/이름 a.txt");
        assert_eq!((f[0].added, f[0].deleted), (5, 2));
        assert_eq!(f[1].path, "z.txt");
        assert!(parse_numstat_z("").is_empty());
    }

    const PATCH: &str = "diff --git a/x.txt b/x.txt\n\
index 111..222 100644\n\
--- a/x.txt\n\
+++ b/x.txt\n\
@@ -1,4 +1,4 @@ fn main() {\n \
one\n\
-two\n\
+TWO\n \
three\n\
@@ -20 +20,2 @@\n \
tail\n\
+more\n";

    #[test]
    fn hunks_number_lines_and_keep_header_text() {
        let h = parse_hunks(PATCH);
        assert_eq!(h.len(), 2);
        assert_eq!(
            (
                h[0].old_start,
                h[0].old_lines,
                h[0].new_start,
                h[0].new_lines
            ),
            (1, 4, 1, 4)
        );
        assert_eq!(h[0].header, "fn main() {");
        let l = &h[0].lines;
        assert_eq!(l.len(), 4);
        assert_eq!(
            (l[0].kind, l[0].old_no, l[0].new_no),
            (DiffLineKind::Ctx, Some(1), Some(1))
        );
        assert_eq!(
            (l[1].kind, l[1].old_no, l[1].new_no),
            (DiffLineKind::Del, Some(2), None)
        );
        assert_eq!(l[1].text, "two");
        assert_eq!(
            (l[2].kind, l[2].old_no, l[2].new_no),
            (DiffLineKind::Add, None, Some(2))
        );
        assert_eq!((l[3].old_no, l[3].new_no), (Some(3), Some(3)));
        // `@@ -20 +20,2 @@`: a missing count means 1.
        assert_eq!((h[1].old_lines, h[1].new_lines), (1, 2));
        assert_eq!(h[1].header, "");
        assert_eq!(h[1].lines.len(), 2);
        assert_eq!(h[1].lines[1].new_no, Some(21));
        assert!(parse_hunks("diff --git a/b b/b\nBinary files a/b and b/b differ\n").is_empty());
    }

    #[test]
    fn hunks_move_a_trailing_cr_into_the_flag() {
        let h = parse_hunks("@@ -1,2 +1,2 @@\n a\r\n-b\r\n+c\r\n");
        let l = &h[0].lines;
        assert_eq!(l.len(), 3);
        assert!(l.iter().all(|x| x.cr));
        assert_eq!(l[0].text, "a");
        assert_eq!(l[2].text, "c");
    }

    #[test]
    fn hunks_keep_the_no_newline_marker_without_numbering_it() {
        let h = parse_hunks(
            "@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n\\ No newline at end of file\n",
        );
        let l = &h[0].lines;
        assert_eq!(l.len(), 4);
        assert_eq!(l[1].kind, DiffLineKind::Noeol);
        assert_eq!((l[1].old_no, l[1].new_no), (None, None));
        assert_eq!(l[1].text, "\\ No newline at end of file");
        assert_eq!(l[2].new_no, Some(1));
        assert_eq!(l[3].kind, DiffLineKind::Noeol);
    }

    #[test]
    fn hunks_do_not_mistake_a_dashed_line_for_a_header_and_clip_long_lines() {
        // A deleted line whose text starts with `-- ` reads `--- ...`.
        let long = "x".repeat(MAX_LINE_CHARS + 50);
        let patch = format!("@@ -1,2 +1,1 @@\n--- not a header\n-- also\n+{long}\n");
        let h = parse_hunks(&patch);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].lines.len(), 3);
        assert_eq!(h[0].lines[0].kind, DiffLineKind::Del);
        assert_eq!(h[0].lines[0].text, "-- not a header");
        let t = &h[0].lines[2].text;
        assert_eq!(t.chars().count(), MAX_LINE_CHARS + 1);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn hunks_stop_at_a_second_file_section_and_ignore_overrun() {
        // `git diff -- f` also matches `f/g`: a deleted file `f` followed by
        // a new file `f/g`. Only the first section is the file asked for.
        let patch = concat!(
            "diff --git a/f b/f\ndeleted file mode 100644\n--- a/f\n+++ /dev/null\n",
            "@@ -1,2 +0,0 @@\n-one\n-two\n",
            "diff --git a/f/g b/f/g\nnew file mode 100644\n--- /dev/null\n+++ b/f/g\n",
            "@@ -0,0 +1 @@\n+inner\n",
        );
        let h = parse_hunks(patch);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].lines.len(), 2);
        // Lines past the counted hunk (anything but a no-newline marker) are
        // not part of it.
        let h = parse_hunks(concat!(
            "@@ -1 +1 @@\n-a\n+b\n+stray\n-stray\n context\n",
            "\\ No newline at end of file\n",
        ));
        assert_eq!(h[0].lines.len(), 3);
        assert_eq!(h[0].lines[2].kind, DiffLineKind::Noeol);
    }

    #[test]
    fn first_section_cuts_at_the_next_file_header() {
        let p =
            "diff --git a/f b/f\nBinary files a/f and /dev/null differ\ndiff --git a/f/g b/f/g\n";
        assert_eq!(
            first_section(p),
            "diff --git a/f b/f\nBinary files a/f and /dev/null differ"
        );
        assert_eq!(first_section("no header\n"), "no header\n");
    }

    #[test]
    fn stream_count_budget_and_binary_sniff() {
        let mut budget = 100u64;
        assert_eq!(
            count_untracked(&b"a\nb\nc"[..], &mut budget),
            UntrackedCount::Text(3)
        );
        assert_eq!(budget, 95);
        assert_eq!(
            count_untracked(&b"a\0b"[..], &mut budget),
            UntrackedCount::Binary
        );
        let mut budget = 4u64;
        assert_eq!(
            count_untracked(&b"a\nb\nc\nd\ne\n"[..], &mut budget),
            UntrackedCount::Unknown
        );
        assert_eq!(count_untracked(&b""[..], &mut 10), UntrackedCount::Text(0));
        assert_eq!(
            count_untracked(&b"x\n"[..], &mut 10),
            UntrackedCount::Text(1)
        );
    }

    #[test]
    fn untracked_text_binary_cap_and_missing_eol() {
        let d = untracked_diff_from_bytes(b"a\r\nb\r\n", 6);
        let h = &d.hunks[0];
        assert_eq!((h.old_lines, h.new_start, h.new_lines), (0, 1, 2));
        assert!(h.lines.iter().all(|l| l.kind == DiffLineKind::Add && l.cr));
        assert_eq!(h.lines[1].new_no, Some(2));

        let d = untracked_diff_from_bytes("한글\nend".as_bytes(), 9);
        let h = &d.hunks[0];
        assert_eq!(h.new_lines, 2);
        assert_eq!(h.lines.last().unwrap().kind, DiffLineKind::Noeol);

        assert!(untracked_diff_from_bytes(b"ab\0cd", 5).binary);
        assert!(untracked_diff_from_bytes(b"", 0).hunks.is_empty());

        let big = "line\n".repeat(MAX_UNTRACKED_BYTES / 5 + 100);
        let d = untracked_diff_from_bytes(big.as_bytes(), big.len() as u64);
        assert!(d.truncated);
        assert_eq!(d.hunks[0].new_lines as usize, MAX_UNTRACKED_BYTES / 5);
    }

    #[test]
    fn untracked_paths_cannot_leave_the_root() {
        let root = Path::new(if cfg!(windows) { "C:\\repo" } else { "/repo" });
        assert!(untracked_path_ok(root, "a/b.txt"));
        assert!(untracked_path_ok(root, "한글 dir/파일.txt"));
        assert!(!untracked_path_ok(root, "../x"));
        assert!(!untracked_path_ok(root, "a/../../x"));
        assert!(!untracked_path_ok(root, "a\\..\\..\\x"));
        assert!(!untracked_path_ok(root, ""));
        assert!(!untracked_path_ok(root, "/etc/passwd"));
        assert!(!untracked_path_ok(root, "C:\\Windows\\x"));
    }

    fn pane(n: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(n)
    }

    #[test]
    fn pick_prefers_the_most_specific_nested_worktree() {
        let wts = vec!["/srv/repo".to_string(), "/srv/repo/.wt/feat".to_string()];
        let cwds = vec![
            (pane(1), "/srv/repo".to_string()),
            (pane(2), "/srv/repo/src/deep".to_string()),
            (pane(3), "/srv/repo/.wt/feat".to_string()),
            (pane(4), "/srv/repo/.wt/feat/src".to_string()),
            (pane(5), "/srv/other".to_string()),
            (pane(6), "/srv/repository".to_string()),
        ];
        assert_eq!(
            pick_agent_panes("/srv/repo", &wts, &cwds),
            vec![pane(1), pane(2)]
        );
        assert_eq!(
            pick_agent_panes("/srv/repo/.wt/feat", &wts, &cwds),
            vec![pane(3), pane(4)]
        );
        assert!(pick_agent_panes("/srv/repo", &wts, &[]).is_empty());
    }

    #[test]
    fn pick_compares_spellings_by_path_syntax() {
        // Windows: drive-letter case and separators; macOS: NFD vs NFC.
        let wts = vec!["C:/Repo".to_string(), "C:/Repo/.wt/한글".to_string()];
        let cwds = vec![
            (pane(1), "c:\\repo\\src".to_string()),
            (pane(2), "c:\\REPO\\.wt\\한글\\x".to_string()),
        ];
        assert_eq!(pick_agent_panes("c:\\repo", &wts, &cwds), vec![pane(1)]);
        assert_eq!(
            pick_agent_panes("C:\\Repo\\.wt\\한글", &wts, &cwds),
            vec![pane(2)]
        );

        let nfd = "/Users/me/\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}".to_string();
        let nfc = "/Users/me/한글".to_string();
        let got = pick_agent_panes(
            &nfc,
            std::slice::from_ref(&nfc),
            &[(pane(7), format!("{nfd}/sub"))],
        );
        assert_eq!(got, vec![pane(7)]);

        // POSIX paths are case-sensitive: `/srv/A` is not `/srv/a`.
        let got = pick_agent_panes(
            "/srv/a",
            &["/srv/a".to_string()],
            &[(pane(8), "/srv/A/x".to_string())],
        );
        assert!(got.is_empty());
    }

    fn write_file(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn diff_round_trip_against_real_git() {
        if !git_available() {
            return;
        }
        let repo = init_test_repo("diff_round_trip");
        git_ok(&repo, &["config", "core.autocrlf", "false"]);
        let body: String = (1..=12).map(|i| format!("line {i}\n")).collect();
        write_file(&repo, "한글 dir/파일 이름.txt", b"a\nb\nc\n");
        write_file(&repo, "crlf.txt", b"x\r\ny\r\n");
        write_file(&repo, "eol.txt", b"p\nq\n");
        write_file(&repo, "bin.dat", &[0, 1, 2, 3]);
        write_file(&repo, "old name.txt", body.as_bytes());
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "base"]);
        let first = git_ok(&repo, &["rev-parse", "HEAD"]).trim().to_string();

        write_file(&repo, "f.txt", b"hello\nworld");
        write_file(&repo, "한글 dir/파일 이름.txt", b"a\nB\nc\n");
        write_file(&repo, "crlf.txt", b"x\r\nz\r\n");
        write_file(&repo, "eol.txt", b"p\r\nq\r\n");
        write_file(&repo, "bin.dat", &[0, 9, 9, 3]);
        git_ok(&repo, &["mv", "old name.txt", "new name.txt"]);
        write_file(
            &repo,
            "new name.txt",
            body.replace("line 6", "LINE 6").as_bytes(),
        );
        write_file(&repo, "새 파일.txt", "첫 줄\nno eol".as_bytes());
        write_file(&repo, "raw.bin", &[0, 0, 1]);

        let s = diff_summary(&repo, "", false).unwrap();
        assert_eq!(s.base_rev, first);
        assert!(!s.truncated);
        let by = |p: &str| {
            s.files
                .iter()
                .find(|f| f.path == p)
                .unwrap_or_else(|| panic!("{p} in {:?}", s.files))
        };
        assert_eq!((by("f.txt").added, by("f.txt").deleted), (1, 0));
        assert_eq!(by("한글 dir/파일 이름.txt").added, 1);
        assert!(by("bin.dat").binary);
        let r = by("new name.txt");
        assert_eq!(r.orig, "old name.txt");
        assert_eq!((r.added, r.deleted), (1, 1));
        assert!(by("새 파일.txt").untracked);
        assert_eq!(by("새 파일.txt").added, 2);
        assert!(by("raw.bin").untracked && by("raw.bin").binary);

        // f.txt: the new last line has no newline.
        let d = diff_file(&repo, &s.base_rev, "f.txt", "", false, false).unwrap();
        assert!(!d.binary && !d.truncated);
        let kinds: Vec<_> = d.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert!(kinds.contains(&DiffLineKind::Noeol), "{kinds:?}");

        let d = diff_file(
            &repo,
            &s.base_rev,
            "한글 dir/파일 이름.txt",
            "",
            false,
            false,
        )
        .unwrap();
        assert!(d.hunks[0]
            .lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Add && l.text == "B"));

        let d = diff_file(&repo, &s.base_rev, "crlf.txt", "", false, false).unwrap();
        assert!(d.hunks[0].lines.iter().all(|l| l.cr));

        let d = diff_file(&repo, &s.base_rev, "bin.dat", "", false, false).unwrap();
        assert!(d.binary && d.hunks.is_empty());

        let d = diff_file(
            &repo,
            &s.base_rev,
            "new name.txt",
            "old name.txt",
            false,
            false,
        )
        .unwrap();
        assert_eq!(d.hunks.len(), 1);
        assert!(d.hunks[0].lines.iter().any(|l| l.text == "LINE 6"));

        let d = diff_file(&repo, &s.base_rev, "새 파일.txt", "", true, false).unwrap();
        assert_eq!(d.hunks[0].lines[0].text, "첫 줄");
        assert!(
            diff_file(&repo, &s.base_rev, "raw.bin", "", true, false)
                .unwrap()
                .binary
        );

        // A CRLF-only change is noise with ignore-cr, a hunk without.
        let eol = |ignore| diff_file(&repo, &s.base_rev, "eol.txt", "", false, ignore).unwrap();
        assert!(!eol(false).hunks.is_empty());
        assert!(eol(true).hunks.is_empty());

        // Guards: a traversing untracked path and a non-hash base.
        assert!(diff_file(&repo, &s.base_rev, "../x", "", true, false).is_err());
        assert!(diff_file(&repo, "--output=x", "f.txt", "", false, false).is_err());
        assert!(diff_summary(&repo, "--output=x", false).is_err());

        // A named base is the merge-base with HEAD.
        git_ok(&repo, &["branch", "basebranch", &first]);
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "second"]);
        let s2 = diff_summary(&repo, "basebranch", false).unwrap();
        assert_eq!(s2.base_rev, first);
        assert!(s2.files.iter().any(|f| f.path == "f.txt"));
        assert!(!s2.files.iter().any(|f| f.untracked));
        let s3 = diff_summary(&repo, "", false).unwrap();
        assert_ne!(s3.base_rev, first);
        assert!(s3.files.is_empty());

        cleanup_dir(&repo);
    }

    #[test]
    fn a_deleted_file_is_not_mixed_up_with_a_new_directory_of_the_same_name() {
        if !git_available() {
            return;
        }
        let repo = init_test_repo("file_to_dir");
        git_ok(&repo, &["config", "core.autocrlf", "false"]);
        write_file(&repo, "f", b"one\ntwo\n");
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "base"]);
        std::fs::remove_file(repo.join("f")).unwrap();
        write_file(&repo, "f/g", b"inner\n");
        git_ok(&repo, &["add", "-A"]);
        let s = diff_summary(&repo, "", false).unwrap();
        let d = diff_file(&repo, &s.base_rev, "f", "", false, false).unwrap();
        assert_eq!(d.hunks.len(), 1);
        assert!(d.hunks[0].lines.iter().all(|l| l.kind == DiffLineKind::Del));
        assert_eq!(d.hunks[0].lines.len(), 2);
        let d = diff_file(&repo, &s.base_rev, "f/g", "", false, false).unwrap();
        assert_eq!(d.hunks[0].lines.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn untracked_reads_do_not_follow_a_symlinked_parent_out_of_the_repo() {
        let base = std::env::temp_dir().join(format!("ymux_untracked_link_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        let outside = base.join("outside");
        write_file(&outside, "secret.txt", b"secret\n");
        std::fs::create_dir_all(&repo).unwrap();
        std::os::unix::fs::symlink(&outside, repo.join("link")).unwrap();
        write_file(&repo, "ok.txt", b"fine\n");
        assert!(read_untracked(&repo, "link/secret.txt").is_err());
        let mut budget = 100;
        assert!(count_untracked_file(&repo, "link/secret.txt", &mut budget).is_err());
        assert!(read_untracked(&repo, "ok.txt").is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn diff_in_an_unborn_repo_shows_the_index_as_added() {
        if !git_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ymux_git_test_unborn_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git_ok(&dir, &["init", "-q"]);
        write_file(&dir, "staged.txt", b"one\ntwo\n");
        git_ok(&dir, &["add", "staged.txt"]);
        write_file(&dir, "loose.txt", b"x\n");

        let s = diff_summary(&dir, "", false).unwrap();
        assert!(s.base_rev.len() >= 40, "{}", s.base_rev);
        let staged = s.files.iter().find(|f| f.path == "staged.txt").unwrap();
        assert_eq!(
            (staged.added, staged.deleted, staged.untracked),
            (2, 0, false)
        );
        assert!(s.files.iter().any(|f| f.path == "loose.txt" && f.untracked));
        let d = diff_file(&dir, &s.base_rev, "staged.txt", "", false, false).unwrap();
        assert_eq!(d.hunks[0].lines.len(), 2);

        cleanup_dir(&dir);
    }

    /// A repo-configured `core.fsmonitor` command must not run from ymux's
    /// unattended git calls.
    #[test]
    fn git_command_never_runs_a_repo_fsmonitor_hook() {
        if !git_available() {
            return;
        }
        let repo = init_test_repo("fsmonitor-off");
        let marker = repo.join("fsmonitor-ran.marker");
        let script = repo.join("fsmon.sh");
        let fwd = |p: &Path| p.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho ran > '{}'\nexit 1\n", fwd(&marker)),
        )
        .unwrap();
        git_ok(
            &repo,
            &[
                "config",
                "core.fsmonitor",
                &format!("sh '{}'", fwd(&script)),
            ],
        );
        // Control: plain git does run it (else this test proves nothing).
        let _ = Command::new("git")
            .current_dir(&repo)
            .args(["status", "--porcelain"])
            .output();
        let control = marker.exists();
        let _ = std::fs::remove_file(&marker);
        let out = git_command(&repo)
            .args(["status", "--porcelain"])
            .output()
            .expect("run git");
        assert!(out.status.success());
        assert!(!marker.exists(), "fsmonitor hook ran under git_command");
        if !control {
            eprintln!("note: this git never ran the fsmonitor control");
        }
        cleanup_dir(&repo);
    }
}
