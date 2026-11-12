//! Completeness gates for the irreversible step (deleting old transcripts).
//!
//! The logic lives in plain functions that take the roots, the index
//! directory, a fixed `now` and optional seams (a lister, a gone-probe), so
//! the ordinary tests below exercise them on temp fixtures. The `#[ignore]`d
//! tests at the end only bind them to this machine's real paths:
//!
//! 0. `print_deletion_list` (read-only) writes the exact list of files that
//!    may be deleted to `YMUX_DELETION_LIST_OUT`.
//! 1. `build_real_index_in_place` (WRITES the app's index directory; run with
//!    yMux closed) indexes every transcript and proves the result is complete
//!    and reloads identically.
//! 2. `verify_real_index_covers` (read-only) checks that every transcript
//!    older than a cutoff has a current, complete shard.
//! 3. `simulate_real_deletion` (works on a COPY of the index directory)
//!    proves that retiring those files changes no figure.
//!
//! With `YMUX_DELETION_LIST` set, steps 1-3 speak for exactly the files on
//! that list instead of "everything older than N days".
use super::collect::{collect_with, now_for_tests, specs_of};
use super::disk::{self, Budget, Presence, MAX_FILES};
use super::index::{shard_path, FileSummary, Index, INDEX_VERSION};
use super::{ScanRoot, TokenSnapshot};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Everything the frontend derives usage from except file counts.
fn figures(s: &TokenSnapshot) -> String {
    let cards: Vec<_> = s
        .providers
        .iter()
        .map(|p| (p.id, &p.account, p.five_hour, p.seven_day, p.all_time))
        .collect();
    serde_json::to_string(&(cards, &s.rows, &s.hourly)).unwrap()
}

fn tokens_of(s: &TokenSnapshot) -> Vec<(String, String, u64)> {
    s.providers
        .iter()
        .map(|p| (p.id.to_owned(), p.account.clone(), p.all_time.total))
        .collect()
}

fn dir_size(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.metadata().map_or(0, |m| m.len()))
                .sum()
        })
        .unwrap_or(0)
}

// ---- F1: build, and prove the build is complete -----------------------------

#[derive(Debug, Default)]
pub(super) struct BuildReport {
    pub files: usize,
    pub bytes: u64,
    pub events: usize,
    pub tokens: Vec<(String, String, u64)>,
    pub disk_bytes: u64,
    pub ticks: usize,
    pub lines_skipped: usize,
    /// Every file with unparseable lines, and how many.
    pub skipped_files: Vec<(PathBuf, usize)>,
    pub shard_write_failures: usize,
    /// Files that grew between the build's last look and the snapshot (the
    /// build's own figures are then not expected to equal the snapshot's).
    pub changed_during_run: Vec<PathBuf>,
    /// Tokens as of the snapshot, which both compared instances are bounded to.
    pub snapshot_tokens: Vec<(String, String, u64)>,
}

/// A lister that answers from `snapshot` alone.
fn snapshot_lister(snapshot: &[RootListing]) -> super::index::Lister {
    let listed: HashMap<(&'static str, PathBuf), Vec<(PathBuf, disk::Stat)>> = snapshot
        .iter()
        .map(|r| ((r.provider, r.dir.clone()), r.files.clone()))
        .collect();
    Box::new(move |spec, _limit| disk::Listing {
        files: listed
            .get(&(spec.provider, spec.dir.clone()))
            .cloned()
            .unwrap_or_default(),
        ..disk::Listing::default()
    })
}

/// Index every transcript under `roots` into `dir`, then require: no file
/// skipped or unreadable, no listing cut short, every shard written, and a
/// FRESH instance loaded from disk to show the same figures while re-reading
/// nothing. `configure` adjusts both instances (seams, limits).
pub(super) fn build_index(
    roots: &[ScanRoot],
    dir: &Path,
    now: u64,
    configure: &dyn Fn(&mut Index),
) -> Result<BuildReport, Vec<String>> {
    build_index_with(roots, dir, now, configure, &|| {}, &|| {})
}

/// [`build_index`], calling `before_snapshot` / `after_snapshot` around the one
/// listing the comparison is bounded to (tests: a transcript grows right there).
fn build_index_with(
    roots: &[ScanRoot],
    dir: &Path,
    now: u64,
    configure: &dyn Fn(&mut Index),
    before_snapshot: &dyn Fn(),
    after_snapshot: &dyn Fn(),
) -> Result<BuildReport, Vec<String>> {
    let mut problems = Vec::new();
    let mut report = BuildReport::default();
    let mut index = Index::new(Some(dir.to_path_buf()));
    configure(&mut index);
    let last = loop {
        report.ticks += 1;
        let s = collect_with(
            &mut index,
            roots,
            now,
            Budget::new(256 * 1024 * 1024, Some(Duration::from_millis(1500))),
        );
        if !s.indexing {
            break s;
        }
        if report.ticks > 100_000 {
            problems.push("indexing did not converge".to_owned());
            break s;
        }
    };
    index.flush();
    if last.unreadable_files > 0 {
        problems.push(format!("{} unreadable file(s)", last.unreadable_files));
    }
    if last.skipped_entries > 0 {
        problems.push(format!("{} skipped listing entries", last.skipped_entries));
    }
    if last.file_limit_hit {
        problems.push("the file limit was hit".to_owned());
    }
    if last.listing_incomplete {
        problems.push("a listing was incomplete".to_owned());
    }
    if last.retired_unavailable {
        problems.push(format!(
            "the retired store is unavailable: {}",
            last.retired_unavailable_reason
        ));
    }
    if index.shard_writes_owed() > 0 {
        problems.push(format!(
            "{} shard write(s) still owed",
            index.shard_writes_owed()
        ));
    }
    report.shard_write_failures = index.shard_write_failures();
    report.lines_skipped = index.skipped_lines();
    report.skipped_files = index.skipped_by_file();
    let (files, events) = index.live_totals();
    report.files = files;
    report.events = events;
    report.bytes = last.indexed_bytes;
    report.tokens = tokens_of(&last);
    if last.indexed_bytes != last.total_bytes {
        problems.push(format!(
            "indexed {} of {} bytes",
            last.indexed_bytes, last.total_bytes
        ));
    }
    let stats = index.summary_stats();
    let first = figures(&last);
    drop(index);
    report.disk_bytes = dir_size(dir);

    // ONE listing after indexing settled. Both instances below are bounded to
    // it (a pass reads at most the listed length), so a transcript that is
    // written to meanwhile cannot make them differ and the comparison is never
    // skipped: a file that grew after this point is compared at its listed
    // length.
    before_snapshot();
    let snapshot = take_snapshot(roots);
    after_snapshot();
    report.changed_during_run = snapshot
        .iter()
        .flat_map(|root| root.files.iter())
        .filter(|(path, stat)| {
            stats
                .get(path)
                .is_some_and(|(len, mtime)| stat.len != *len || stat.mtime != *mtime)
        })
        .map(|(p, _)| p.clone())
        .collect();
    let mut bounded = Index::new(Some(dir.to_path_buf()));
    configure(&mut bounded);
    bounded.set_lister(snapshot_lister(&snapshot));
    let mut at_snapshot = collect_with(&mut bounded, roots, now, Budget::unlimited());
    for _ in 0..8 {
        if !at_snapshot.indexing {
            break;
        }
        at_snapshot = collect_with(&mut bounded, roots, now, Budget::unlimited());
    }
    bounded.flush();
    if at_snapshot.indexing {
        problems.push("the index did not settle at the snapshot".to_owned());
    }
    if report.changed_during_run.is_empty() && figures(&at_snapshot) != first {
        problems.push("the figures at the snapshot differ from the build's".to_owned());
    }
    report.snapshot_tokens = tokens_of(&at_snapshot);
    let at_snapshot_figures = figures(&at_snapshot);
    drop(bounded);

    // A fresh instance, as the next start of the app, bounded to the same
    // snapshot: identical figures and nothing re-read (every file's shard
    // already covers its snapshot length).
    let mut fresh = Index::new(Some(dir.to_path_buf()));
    configure(&mut fresh);
    fresh.set_lister(snapshot_lister(&snapshot));
    let second = collect_with(&mut fresh, roots, now, Budget::unlimited());
    if fresh.last_read_bytes() != 0 {
        problems.push(format!(
            "a fresh instance re-read {} bytes",
            fresh.last_read_bytes()
        ));
    }
    if figures(&second) != at_snapshot_figures {
        problems.push("a fresh instance shows other figures".to_owned());
    }
    if problems.is_empty() {
        Ok(report)
    } else {
        Err(problems)
    }
}

// ---- F2: does every old transcript have a complete shard? -------------------

#[derive(Debug, Default)]
pub(super) struct CoverageReport {
    pub checked: usize,
    pub bytes: u64, // covered
    pub offenders: Vec<(PathBuf, String)>,
}

/// Which transcripts a gate speaks for. Both gates, and the deletion list the
/// orchestrator acts on, must be about the SAME files: a mtime rule is a
/// different set from "this file's mtime AND its session's main transcript"
/// (see `plan_deletion`), so the list itself can be handed to the gates.
#[derive(Clone, Debug)]
pub(super) enum Selection {
    /// Every transcript whose mtime is before the cutoff (seconds); `None` =
    /// every transcript.
    Mtime(Option<u64>),
    /// Exactly these paths (`YMUX_DELETION_LIST`).
    List(Vec<PathBuf>),
}

/// One root's files as listed at one instant.
pub(super) struct RootListing {
    pub provider: &'static str,
    pub dir: PathBuf,
    pub folders: Vec<PathBuf>,
    pub files: Vec<(PathBuf, disk::Stat)>,
    /// Set when the listing could not be trusted to be complete.
    pub note: Option<String>,
}

/// List every root once. A gate takes ONE of these and uses it throughout, so
/// a transcript that is written to meanwhile cannot make two looks disagree.
pub(super) fn take_snapshot(roots: &[ScanRoot]) -> Vec<RootListing> {
    specs_of(roots)
        .into_iter()
        .map(|spec| {
            let listing = disk::list_folders(&spec.folders, MAX_FILES);
            let note = listing.incomplete.then(|| {
                format!(
                    "the listing of this root is incomplete ({} entries skipped, truncated: {})",
                    listing.skipped, listing.truncated
                )
            });
            RootListing {
                provider: spec.provider,
                dir: spec.dir,
                folders: spec.folders,
                files: listing.files,
                note,
            }
        })
        .collect()
}

pub(super) struct Picked {
    pub path: PathBuf,
    pub provider: &'static str,
    pub stat: disk::Stat,
}

/// The files `selection` names, as the index lists them, plus a reason for
/// every listed path that is not a transcript the index would track (not under
/// a scan root, not a `*.jsonl` file there, not absolute, or gone).
pub(super) fn pick(
    snapshot: &[RootListing],
    selection: &Selection,
) -> (Vec<Picked>, Vec<(PathBuf, String)>) {
    let mut picked = Vec::new();
    let mut bad = Vec::new();
    match selection {
        Selection::Mtime(before) => {
            for root in snapshot {
                for (path, stat) in &root.files {
                    if before.is_some_and(|before| stat.mtime_secs() >= before) {
                        continue;
                    }
                    picked.push(Picked {
                        path: path.clone(),
                        provider: root.provider,
                        stat: *stat,
                    });
                }
            }
        }
        Selection::List(list) => {
            let tracked: HashMap<String, Picked> = snapshot
                .iter()
                .flat_map(|root| {
                    root.files.iter().map(|(path, stat)| {
                        (
                            ypath::comparison_key(&path.to_string_lossy()),
                            Picked {
                                path: path.clone(),
                                provider: root.provider,
                                stat: *stat,
                            },
                        )
                    })
                })
                .collect();
            let mut seen = HashSet::new();
            for path in list {
                let key = ypath::comparison_key(&path.to_string_lossy());
                if !seen.insert(key.clone()) {
                    continue;
                }
                if !path.is_absolute() {
                    bad.push((path.clone(), "not an absolute path".into()));
                } else if let Some(found) = tracked.get(&key) {
                    picked.push(Picked {
                        path: found.path.clone(),
                        provider: found.provider,
                        stat: found.stat,
                    });
                } else {
                    bad.push((
                        path.clone(),
                        "not a transcript the index tracks: it is not a *.jsonl file under a scan root's projects/sessions folder (or it no longer exists)".into(),
                    ));
                }
            }
        }
    }
    (picked, bad)
}

/// Read-only. For every transcript `selection` names check that its shard
/// exists, decodes, is current, belongs to the path and covers exactly the
/// file as it is now (`scanned == len == size`, same mtime). A listed path that
/// is not a transcript the index tracks is an offender. Writes nothing
/// anywhere.
pub(super) fn verify_coverage(
    roots: &[ScanRoot],
    dir: &Path,
    selection: &Selection,
) -> CoverageReport {
    let mut report = CoverageReport::default();
    let snapshot = take_snapshot(roots);
    for root in &snapshot {
        if let Some(note) = &root.note {
            report.offenders.push((root.dir.clone(), note.clone()));
        }
    }
    let (picked, bad) = pick(&snapshot, selection);
    report.offenders.extend(bad);
    for file in picked {
        report.checked += 1;
        match shard_covers(dir, &file.path, file.provider, file.stat) {
            Ok(()) => report.bytes += file.stat.len,
            Err(why) => report.offenders.push((file.path, why)),
        }
    }
    report
}

// ---- the deletion list ---------------------------------------------------------

/// What `print_deletion_list` found.
#[derive(Debug, Default)]
pub(super) struct DeletionPlan {
    pub paths: Vec<PathBuf>,
    pub bytes: u64,
    /// Old files kept back because their session's main transcript is young.
    pub excluded_young_session: usize,
    /// Old files kept back because their session could not be told.
    pub excluded_unclear: usize,
}

/// The files that may be deleted: a transcript whose own mtime is before
/// `cutoff` AND, for a Claude file inside a session directory
/// (`<project>/<session-id>/subagents/*.jsonl`), whose session's main
/// transcript (`<project>/<session-id>.jsonl`) is before `cutoff` or missing.
/// Only `*.jsonl` files the index lists, never anything else.
pub(super) fn plan_deletion(snapshot: &[RootListing], cutoff: u64) -> DeletionPlan {
    let mut plan = DeletionPlan::default();
    for root in snapshot {
        let stats: HashMap<&Path, disk::Stat> =
            root.files.iter().map(|(p, s)| (p.as_path(), *s)).collect();
        for (path, stat) in &root.files {
            if stat.mtime_secs() >= cutoff {
                continue;
            }
            if root.provider == "claude" {
                let main = session_main(&root.folders, path);
                match main {
                    Some(Some(main)) => {
                        let main_mtime = match stats.get(main.as_path()) {
                            Some(s) => Some(s.mtime_secs()),
                            None => match fs::symlink_metadata(&main) {
                                Ok(meta) => Some(disk::Stat::of(&meta).mtime_secs()),
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                                Err(_) => {
                                    plan.excluded_unclear += 1;
                                    continue;
                                }
                            },
                        };
                        if main_mtime.is_some_and(|m| m >= cutoff) {
                            plan.excluded_young_session += 1;
                            continue;
                        }
                    }
                    Some(None) => {}
                    None => {
                        plan.excluded_unclear += 1;
                        continue;
                    }
                }
            }
            plan.paths.push(path.clone());
            plan.bytes = plan.bytes.saturating_add(stat.len);
        }
    }
    plan.paths.sort();
    plan
}

/// For a Claude transcript: `Some(None)` = it is a main transcript (or sits
/// outside any session directory), `Some(Some(main))` = it belongs to the
/// session whose main transcript is `main`, `None` = not under a known folder.
fn session_main(folders: &[PathBuf], path: &Path) -> Option<Option<PathBuf>> {
    let (folder, rel) = folders
        .iter()
        .find_map(|f| path.strip_prefix(f).ok().map(|rel| (f, rel)))?;
    let parts: Vec<_> = rel.components().collect();
    if parts.len() <= 2 {
        return Some(None);
    }
    // <project>/<session-id>/...: the session's main transcript.
    let project = parts[0].as_os_str();
    let session = parts[1].as_os_str().to_string_lossy();
    Some(Some(folder.join(project).join(format!("{session}.jsonl"))))
}

/// One absolute path per line, UTF-8 (a BOM is tolerated). Blank lines are
/// skipped; an empty list is an error: it would vouch for nothing.
pub(super) fn read_deletion_list(path: &Path) -> Result<Vec<PathBuf>, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
    let list: Vec<PathBuf> = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(|l| l.trim_end_matches(['\r', '\n']))
        .filter(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .collect();
    if list.is_empty() {
        return Err(format!("{} lists no path", path.display()));
    }
    Ok(list)
}

/// `YMUX_DELETION_LIST`, if set: the exact files to vouch for.
fn selection_from_env(default: Selection) -> Selection {
    match std::env::var_os("YMUX_DELETION_LIST") {
        Some(file) => match read_deletion_list(Path::new(&file)) {
            Ok(list) => {
                println!(
                    "deletion list {}: {} path(s)",
                    Path::new(&file).display(),
                    list.len()
                );
                Selection::List(list)
            }
            Err(e) => panic!("YMUX_DELETION_LIST: {e}"),
        },
        None => default,
    }
}

/// Problems with the parser-skipped lines of the files in scope (`None` =
/// every file). An unreadable line is data the index could not count, so a
/// deletion is not vouched for unless `allow` says it was looked at.
pub(super) fn skipped_problems(
    skipped: &[(PathBuf, usize)],
    scope: Option<&[PathBuf]>,
    allow: bool,
) -> Vec<String> {
    if allow {
        return Vec::new();
    }
    let in_scope: Option<HashSet<String>> = scope.map(|list| {
        list.iter()
            .map(|p| ypath::comparison_key(&p.to_string_lossy()))
            .collect()
    });
    skipped
        .iter()
        .filter(|(path, n)| {
            *n > 0
                && in_scope.as_ref().map_or(true, |s| {
                    s.contains(&ypath::comparison_key(&path.to_string_lossy()))
                })
        })
        .map(|(path, n)| {
            format!(
                "{} line(s) of {} could not be parsed (YMUX_ALLOW_SKIPPED_LINES=1 accepts that)",
                n,
                path.display()
            )
        })
        .collect()
}

fn shard_covers(dir: &Path, path: &Path, provider: &str, stat: disk::Stat) -> Result<(), String> {
    let shard = shard_path(dir, path);
    let bytes = fs::read(&shard).map_err(|e| format!("no readable shard ({e})"))?;
    let summary: FileSummary =
        serde_json::from_slice(&bytes).map_err(|e| format!("shard does not decode ({e})"))?;
    if summary.version != INDEX_VERSION {
        return Err(format!("shard version {}", summary.version));
    }
    if summary.path != path.to_string_lossy() || summary.provider != provider {
        return Err("shard belongs to another file".into());
    }
    if !summary.consistent() {
        return Err("shard is inconsistent".into());
    }
    if summary.scanned != summary.len {
        return Err(format!(
            "shard scanned {} of {} bytes",
            summary.scanned, summary.len
        ));
    }
    if summary.len != stat.len {
        return Err(format!(
            "shard was made for {} bytes, the file has {}",
            summary.len, stat.len
        ));
    }
    if summary.mtime != stat.mtime {
        return Err("the file was modified after it was indexed".into());
    }
    Ok(())
}

// ---- F3: simulate deleting the old files on a copy --------------------------

#[derive(Debug, Default)]
pub(super) struct SimReport {
    pub victims: usize,
    pub before: String,
    pub after: String,
    pub tokens_before: Vec<(String, String, u64)>,
    pub tokens_after: Vec<(String, String, u64)>,
    pub problems: Vec<String>,
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Prove the plan on a COPY of `real_dir`: copy it to `work`, take the
/// figures, then start a fresh instance on the copy whose listing omits (and
/// whose probe reports gone) every transcript `selection` names, let it
/// retire them, and compare. Transcripts are only listed and stat-ed;
/// `real_dir` is only read.
///
/// ONE listing is taken and both instances use it, so a transcript that is
/// being written to meanwhile cannot make the "before" and "after" figures
/// differ: a pass reads at most the length listed, whatever the file grows to.
pub(super) fn simulate_deletion(
    roots: &[ScanRoot],
    real_dir: &Path,
    work: &Path,
    now: u64,
    selection: &Selection,
) -> SimReport {
    simulate_with(roots, real_dir, work, now, selection, &|| {})
}

/// [`simulate_deletion`], calling `between` after the "before" figures are
/// taken and before the "after" ones (tests: a transcript grows right there).
fn simulate_with(
    roots: &[ScanRoot],
    real_dir: &Path,
    work: &Path,
    now: u64,
    selection: &Selection,
    between: &dyn Fn(),
) -> SimReport {
    let mut report = SimReport::default();
    let snapshot = take_snapshot(roots);
    let (picked, bad) = pick(&snapshot, selection);
    if !bad.is_empty() {
        for (path, why) in bad {
            report.problems.push(format!("{}: {why}", path.display()));
        }
        return report;
    }
    if let Err(e) = copy_dir(real_dir, work) {
        report.problems.push(format!("cannot copy the index: {e}"));
        return report;
    }
    let victims: Arc<HashSet<PathBuf>> = Arc::new(picked.into_iter().map(|p| p.path).collect());
    report.victims = victims.len();
    if victims.is_empty() {
        report
            .problems
            .push("no transcript is selected: nothing to simulate".into());
        return report;
    }
    type Listed = HashMap<(&'static str, PathBuf), Vec<(PathBuf, disk::Stat)>>;
    let listed: Arc<Listed> = Arc::new(
        snapshot
            .into_iter()
            .map(|r| ((r.provider, r.dir), r.files))
            .collect(),
    );
    let lister = |omit: Option<Arc<HashSet<PathBuf>>>| -> super::index::Lister {
        let listed = Arc::clone(&listed);
        Box::new(move |spec, _limit| {
            let mut files = listed
                .get(&(spec.provider, spec.dir.clone()))
                .cloned()
                .unwrap_or_default();
            if let Some(omit) = &omit {
                files.retain(|(p, _)| !omit.contains(p));
            }
            disk::Listing {
                files,
                ..disk::Listing::default()
            }
        })
    };
    let mut before = Index::new(Some(work.to_path_buf()));
    before.sweep_every = Duration::ZERO;
    before.set_lister(lister(None));
    let b = collect_with(&mut before, roots, now, Budget::unlimited());
    before.flush();
    drop(before);
    report.before = figures(&b);
    report.tokens_before = tokens_of(&b);
    between();

    let mut after = Index::new(Some(work.to_path_buf()));
    after.sweep_every = Duration::ZERO;
    after.persist_every = Duration::ZERO;
    after.retired_persist_every = Duration::ZERO;
    after.set_lister(lister(Some(Arc::clone(&victims))));
    let gone = Arc::clone(&victims);
    after.set_probe(Box::new(move |path, root| {
        if gone.contains(path) {
            Presence::Gone
        } else {
            disk::presence(path, root)
        }
    }));
    let mut last = b;
    for _ in 0..8 {
        last = collect_with(&mut after, roots, now, Budget::unlimited());
    }
    after.flush();
    report.after = figures(&last);
    report.tokens_after = tokens_of(&last);
    if report.before != report.after {
        report
            .problems
            .push("the figures changed when the old files were retired".into());
    }
    if tokens_of_hours(&report.before) != tokens_of_hours(&report.after) {
        report.problems.push("the heatmap hours changed".into());
    }
    if !work.join("retired.json").is_file() {
        report.problems.push("retired.json was not written".into());
    }
    let left: Vec<_> = victims
        .iter()
        .filter(|p| shard_path(work, p).exists())
        .collect();
    if !left.is_empty() {
        report.problems.push(format!(
            "{} victim shard(s) are still in the copy",
            left.len()
        ));
    }
    report
}

fn tokens_of_hours(figures: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(figures)
        .map(|v| v[2].clone())
        .unwrap_or_default()
}

// ---- ordinary tests on temp fixtures ----------------------------------------

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use crate::token_usage::index_tests::{env, line, roots, Env};
    use std::time::SystemTime;

    const DAY: u64 = 86400;

    fn age(path: &Path, days: u64) {
        let when = SystemTime::now() - Duration::from_secs(days * DAY);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    fn count(time: &str, total: u64) -> String {
        format!("{{\"timestamp\":\"{time}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{total},\"total_tokens\":{total}}}}}}}}}\n")
    }

    /// Old and recent files of both providers, with a shared message and a
    /// Codex session split in two. Returns (old, recent) paths.
    fn fixture(env: &Env) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let c = |n: &str| env.claude.join("projects").join("p").join(n);
        let x = |n: &str| env.codex.join("sessions").join(n);
        let header = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"sess\",\"cwd\":\"/c\"}}\n";
        let files = [
            (
                c("old1.jsonl"),
                line("s", "2026-09-01T00:00:00Z", "a", 10)
                    + &line("s", "2026-09-01T01:00:00Z", "shared", 5),
            ),
            (c("old2.jsonl"), line("t", "2026-09-02T00:00:00Z", "b", 20)),
            (
                c("new1.jsonl"),
                line("u", "2026-11-10T00:00:00Z", "c", 7)
                    + &line("u", "2026-09-01T01:00:00Z", "shared", 5),
            ),
            (
                x("old.jsonl"),
                format!("{header}{}", count("2026-09-03T00:00:00Z", 40)),
            ),
            (
                x("new.jsonl"),
                format!("{header}{}", count("2026-11-10T01:00:00Z", 90)),
            ),
        ];
        for (p, t) in &files {
            fs::write(p, t).unwrap();
        }
        age(&files[0].0, 60);
        age(&files[1].0, 40);
        age(&files[3].0, 50);
        (
            vec![files[0].0.clone(), files[1].0.clone(), files[3].0.clone()],
            vec![files[2].0.clone(), files[4].0.clone()],
        )
    }

    fn listing_snapshot(dir: &Path) -> Vec<(PathBuf, u64, Option<SystemTime>)> {
        let mut v = Vec::new();
        fn walk(dir: &Path, v: &mut Vec<(PathBuf, u64, Option<SystemTime>)>) {
            if let Ok(rd) = fs::read_dir(dir) {
                for e in rd.flatten() {
                    let m = e.metadata().unwrap();
                    if m.is_dir() {
                        walk(&e.path(), v);
                    } else {
                        v.push((e.path(), m.len(), m.modified().ok()));
                    }
                }
            }
        }
        walk(dir, &mut v);
        v.sort();
        v
    }

    #[test]
    fn the_build_gate_passes_on_a_complete_index_and_reports_it() {
        let env = env();
        fixture(&env);
        let report = build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        assert_eq!(report.files, 5);
        assert!(report.bytes > 0 && report.disk_bytes > 0);
        assert_eq!(report.events, 7);
        assert_eq!(report.shard_write_failures, 0);
        assert!(report.changed_during_run.is_empty());
        let claude: u64 = report
            .tokens
            .iter()
            .filter(|t| t.0 == "claude")
            .map(|t| t.2)
            .sum();
        assert_eq!(claude, 10 + 5 + 20 + 7);
    }

    fn grow(path: &Path, text: &str) {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn the_build_gate_compares_at_the_snapshot_when_a_file_grows_before_it() {
        let env = env();
        fixture(&env);
        let target = env.claude.join("projects/p/new1.jsonl");
        let report = build_index_with(
            &roots(&env),
            &env.index_dir,
            now_for_tests(),
            &|_| {},
            &|| grow(&target, &line("u", "2026-11-10T02:00:00Z", "grown", 1000)),
            &|| {},
        )
        .unwrap();
        assert_eq!(report.changed_during_run, vec![target]);
        let sum = |t: &[(String, String, u64)]| t.iter().map(|x| x.2).sum::<u64>();
        assert_eq!(sum(&report.snapshot_tokens), sum(&report.tokens) + 1000);
    }

    #[test]
    fn the_build_gate_compares_at_the_snapshot_length_when_a_file_grows_after_it() {
        let env = env();
        fixture(&env);
        let target = env.claude.join("projects/p/new1.jsonl");
        let report = build_index_with(
            &roots(&env),
            &env.index_dir,
            now_for_tests(),
            &|_| {},
            &|| {},
            &|| grow(&target, &line("u", "2026-11-10T02:00:00Z", "grown", 1000)),
        )
        .unwrap();
        assert!(report.changed_during_run.is_empty());
        assert_eq!(
            report.snapshot_tokens, report.tokens,
            "the growth after the snapshot is not part of either comparison"
        );
    }

    #[test]
    fn the_build_gate_fails_on_a_truncated_listing() {
        let env = env();
        fixture(&env);
        let problems = build_index(&roots(&env), &env.index_dir, now_for_tests(), &|i| {
            i.file_limit = 1;
        })
        .unwrap_err();
        assert!(problems.iter().any(|p| p.contains("limit")), "{problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("incomplete")),
            "{problems:?}"
        );
    }

    #[test]
    fn the_build_gate_fails_on_an_unreadable_file() {
        let env = env();
        fixture(&env);
        let ghost = env.claude.join("projects/p/ghost.jsonl");
        let problems = build_index(&roots(&env), &env.index_dir, now_for_tests(), &move |i| {
            let ghost = ghost.clone();
            let real = disk::list_folders;
            i.set_lister(Box::new(move |spec, limit| {
                let mut l = real(&spec.folders, limit);
                if spec.provider == "claude" {
                    l.files.push((
                        ghost.clone(),
                        disk::Stat {
                            len: 10,
                            mtime: Some(1),
                        },
                    ));
                }
                l
            }));
        })
        .unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("unreadable")),
            "{problems:?}"
        );
    }

    #[test]
    fn coverage_accepts_a_built_index_and_names_every_offender() {
        let env = env();
        let (old, recent) = fixture(&env);
        build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        let cutoff = now_for_tests() - 14 * DAY;
        let ok = verify_coverage(
            &roots(&env),
            &env.index_dir,
            &Selection::Mtime(Some(cutoff)),
        );
        assert_eq!((ok.checked, ok.offenders.len()), (old.len(), 0), "{ok:?}");
        let all = verify_coverage(&roots(&env), &env.index_dir, &Selection::Mtime(None));
        assert_eq!(all.checked, old.len() + recent.len());
        // A missing shard, a shard of an unfinished file, and a file modified
        // after it was indexed.
        fs::remove_file(shard_path(&env.index_dir, &old[0])).unwrap();
        let p = shard_path(&env.index_dir, &old[1]);
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        value["scanned"] = 1.into();
        fs::write(&p, serde_json::to_vec(&value).unwrap()).unwrap();
        crate::token_usage::index_tests::append(&old[2], &count("2026-09-04T00:00:00Z", 41));
        age(&old[2], 50);
        let bad = verify_coverage(
            &roots(&env),
            &env.index_dir,
            &Selection::Mtime(Some(cutoff)),
        );
        let names: Vec<&PathBuf> = bad.offenders.iter().map(|(p, _)| p).collect();
        assert_eq!(bad.offenders.len(), 3, "{bad:?}");
        for p in &old {
            assert!(names.contains(&p), "{p:?} not named: {bad:?}");
        }
    }

    #[test]
    fn coverage_flags_a_changed_mtime_alone_and_a_changed_size_alone() {
        let env = env();
        let (old, _) = fixture(&env);
        build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        let cutoff = now_for_tests() - 14 * DAY;
        let mtime_of = |p: &Path| fs::metadata(p).unwrap().modified().unwrap();
        let set = |p: &Path, t: SystemTime| {
            fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        // Same bytes, later mtime.
        let original = mtime_of(&old[0]);
        set(&old[0], original - Duration::from_secs(5));
        let bad = verify_coverage(
            &roots(&env),
            &env.index_dir,
            &Selection::Mtime(Some(cutoff)),
        );
        assert_eq!(bad.offenders.len(), 1, "{bad:?}");
        assert!(bad.offenders[0].1.contains("modified"), "{bad:?}");
        set(&old[0], original);
        assert!(verify_coverage(
            &roots(&env),
            &env.index_dir,
            &Selection::Mtime(Some(cutoff))
        )
        .offenders
        .is_empty());
        // More bytes, the very same mtime.
        let original = mtime_of(&old[1]);
        crate::token_usage::index_tests::append(
            &old[1],
            &line("t", "2026-09-02T05:00:00Z", "z", 1),
        );
        set(&old[1], original);
        let bad = verify_coverage(
            &roots(&env),
            &env.index_dir,
            &Selection::Mtime(Some(cutoff)),
        );
        assert_eq!(bad.offenders.len(), 1, "{bad:?}");
        assert!(bad.offenders[0].1.contains("bytes"), "{bad:?}");
    }

    #[test]
    fn coverage_writes_nothing() {
        let env = env();
        fixture(&env);
        build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        let before = (
            listing_snapshot(&env.index_dir),
            listing_snapshot(&env.claude),
            listing_snapshot(&env.codex),
        );
        verify_coverage(&roots(&env), &env.index_dir, &Selection::Mtime(None));
        let after = (
            listing_snapshot(&env.index_dir),
            listing_snapshot(&env.claude),
            listing_snapshot(&env.codex),
        );
        assert_eq!(before, after);
    }

    #[test]
    fn the_simulated_deletion_changes_no_figure_and_touches_nothing_real() {
        let env = env();
        let (old, _) = fixture(&env);
        build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        let before = (
            listing_snapshot(&env.index_dir),
            listing_snapshot(&env.claude),
            listing_snapshot(&env.codex),
        );
        let work = env._dir.path().join("work");
        let report = simulate_deletion(
            &roots(&env),
            &env.index_dir,
            &work,
            now_for_tests(),
            &Selection::Mtime(Some(now_for_tests() - 14 * DAY)),
        );
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.victims, old.len());
        assert_eq!(report.tokens_before, report.tokens_after);
        assert!(work.join("retired.json").is_file());
        let after = (
            listing_snapshot(&env.index_dir),
            listing_snapshot(&env.claude),
            listing_snapshot(&env.codex),
        );
        assert_eq!(
            before, after,
            "the real index and the transcripts are untouched"
        );
        for p in &old {
            assert!(p.is_file());
        }
    }

    #[test]
    fn the_simulation_refuses_to_pass_when_nothing_is_old_enough() {
        let env = env();
        fixture(&env);
        build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        let work = env._dir.path().join("work");
        let report = simulate_deletion(
            &roots(&env),
            &env.index_dir,
            &work,
            now_for_tests(),
            &Selection::Mtime(Some(now_for_tests() - 3650 * DAY)),
        );
        assert!(!report.problems.is_empty());
    }

    // ---- the explicit deletion list ------------------------------------------

    /// A Claude session with a main transcript and subagent transcripts, in
    /// the layout Claude Code writes: `<project>/<id>.jsonl` plus
    /// `<project>/<id>/subagents/*.jsonl`.
    fn sessions(env: &Env) -> Vec<(String, PathBuf, u64)> {
        let project = env.claude.join("projects").join("p");
        let mut out = Vec::new();
        // (name, main age, subagent age): the subagent file is always old.
        for (id, main_age, main_exists) in
            [("old", 60, true), ("young", 1, true), ("orphan", 0, false)]
        {
            let main = project.join(format!("{id}.jsonl"));
            if main_exists {
                fs::write(
                    &main,
                    line(id, "2026-09-01T00:00:00Z", &format!("{id}-m"), 3),
                )
                .unwrap();
                age(&main, main_age);
            }
            let sub = project.join(id).join("subagents");
            fs::create_dir_all(&sub).unwrap();
            let file = sub.join("agent-1.jsonl");
            fs::write(
                &file,
                line(id, "2026-09-01T00:00:00Z", &format!("{id}-s"), 4),
            )
            .unwrap();
            age(&file, 60);
            fs::write(sub.join("notes.txt"), "not a transcript").unwrap();
            out.push((id.to_owned(), file, 60));
        }
        out
    }

    #[test]
    fn the_plan_needs_the_file_old_and_its_session_old_or_missing() {
        let env = env();
        let (old, _) = fixture(&env);
        let subs = sessions(&env);
        let project = env.claude.join("projects").join("p");
        let snapshot = take_snapshot(&roots(&env));
        let plan = plan_deletion(&snapshot, now_for_tests() - 14 * DAY);
        let has = |p: &Path| plan.paths.iter().any(|q| q == p);
        // The old session: its main file and its subagent file.
        assert!(has(&project.join("old.jsonl")));
        assert!(has(&subs[0].1));
        // The young session's main file is young, so its old subagent file
        // stays; a session whose main transcript is missing may go.
        assert!(!has(&project.join("young.jsonl")));
        assert!(!has(&subs[1].1));
        assert!(has(&subs[2].1));
        assert_eq!(plan.excluded_young_session, 1);
        assert_eq!(plan.excluded_unclear, 0);
        // The fixture's own old files (both providers) are in as well.
        for p in &old {
            assert!(has(p), "{p:?}");
        }
        // Only transcripts: nothing but *.jsonl files, no directory.
        assert!(plan
            .paths
            .iter()
            .all(|p| p.is_file() && p.extension().is_some_and(|e| e == "jsonl")));
        let bytes: u64 = plan
            .paths
            .iter()
            .map(|p| fs::metadata(p).unwrap().len())
            .sum();
        assert_eq!(plan.bytes, bytes);
    }

    #[test]
    fn both_gates_check_exactly_the_listed_files() {
        let env = env();
        let (old, recent) = fixture(&env);
        build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        // Break the shard of a file that is NOT on the list and one that is.
        fs::remove_file(shard_path(&env.index_dir, &recent[0])).unwrap();
        let listed = vec![old[0].clone(), old[1].clone()];
        let ok = verify_coverage(
            &roots(&env),
            &env.index_dir,
            &Selection::List(listed.clone()),
        );
        assert_eq!((ok.checked, ok.offenders.len()), (2, 0), "{ok:?}");
        fs::remove_file(shard_path(&env.index_dir, &old[1])).unwrap();
        let bad = verify_coverage(&roots(&env), &env.index_dir, &Selection::List(listed));
        assert_eq!(bad.offenders.len(), 1, "{bad:?}");
        assert_eq!(bad.offenders[0].0, old[1]);
    }

    #[test]
    fn a_listed_path_the_index_would_not_track_fails_the_gate() {
        let env = env();
        let (old, _) = fixture(&env);
        build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        let project = env.claude.join("projects").join("p");
        fs::write(project.join("MEMORY.md"), "notes").unwrap();
        let outside = env._dir.path().join("elsewhere.jsonl");
        fs::write(&outside, line("o", "2026-09-01T00:00:00Z", "o", 1)).unwrap();
        for stray in [
            project.join("MEMORY.md"),
            project.clone(),
            outside,
            project.join("missing.jsonl"),
            PathBuf::from("relative.jsonl"),
        ] {
            let report = verify_coverage(
                &roots(&env),
                &env.index_dir,
                &Selection::List(vec![old[0].clone(), stray.clone()]),
            );
            assert_eq!(report.offenders.len(), 1, "{stray:?}: {report:?}");
            assert_eq!(report.offenders[0].0, stray);
            let work = env._dir.path().join("work");
            let sim = simulate_deletion(
                &roots(&env),
                &env.index_dir,
                &work,
                now_for_tests(),
                &Selection::List(vec![old[0].clone(), stray.clone()]),
            );
            assert!(!sim.problems.is_empty(), "{stray:?}");
            let _ = fs::remove_dir_all(&work);
        }
    }

    #[test]
    fn the_simulation_retires_exactly_the_listed_files() {
        let env = env();
        let (old, recent) = fixture(&env);
        build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        // A recent file is on the list; an old one is not.
        let listed = vec![old[0].clone(), recent[0].clone()];
        let work = env._dir.path().join("work");
        let report = simulate_deletion(
            &roots(&env),
            &env.index_dir,
            &work,
            now_for_tests(),
            &Selection::List(listed),
        );
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.victims, 2);
        assert!(!shard_path(&work, &old[0]).exists());
        assert!(!shard_path(&work, &recent[0]).exists());
        assert!(shard_path(&work, &old[1]).exists(), "not on the list");
    }

    #[test]
    fn the_simulation_is_not_disturbed_by_a_file_that_grows_meanwhile() {
        let env = env();
        let (old, _) = fixture(&env);
        build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        // A live transcript is appended to after the index was built; both
        // instances must read the same length of it (the snapshot's).
        let live = env.claude.join("projects").join("p").join("new1.jsonl");
        let grow = |text: &str| crate::token_usage::index_tests::append(&live, text);
        grow(&line("u", "2026-11-11T00:00:00Z", "grown", 9));
        let work = env._dir.path().join("work");
        let report = simulate_with(
            &roots(&env),
            &env.index_dir,
            &work,
            now_for_tests(),
            &Selection::List(old),
            // ... and again between the two looks.
            &|| grow(&line("u", "2026-11-12T00:00:00Z", "grown_more", 11)),
        );
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.before, report.after);
    }

    #[test]
    fn a_deletion_list_file_is_read_as_utf8_lines() {
        let env = env();
        let file = env._dir.path().join("list.txt");
        fs::write(&file, "\u{feff}C:\\a b\\ü.jsonl\r\n\r\n/x/y.jsonl\n").unwrap();
        let list = read_deletion_list(&file).unwrap();
        assert_eq!(
            list,
            vec![
                PathBuf::from("C:\\a b\\ü.jsonl"),
                PathBuf::from("/x/y.jsonl")
            ]
        );
        fs::write(&file, "\n \n").unwrap();
        assert!(read_deletion_list(&file).is_err());
        fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_deletion_list(&file).is_err());
        assert!(read_deletion_list(&env._dir.path().join("none.txt")).is_err());
    }

    #[test]
    fn skipped_lines_fail_the_build_for_files_in_scope_unless_allowed() {
        let a = PathBuf::from("/s/a.jsonl");
        let b = PathBuf::from("/s/b.jsonl");
        let skipped = vec![(a.clone(), 3), (b.clone(), 0)];
        assert_eq!(skipped_problems(&skipped, None, false).len(), 1);
        assert!(skipped_problems(&skipped, None, true).is_empty());
        assert!(skipped_problems(&skipped, Some(std::slice::from_ref(&b)), false).is_empty());
        assert_eq!(skipped_problems(&skipped, Some(&[a, b]), false).len(), 1);
    }

    #[test]
    fn the_build_reports_skipped_lines_per_file() {
        let env = env();
        fixture(&env);
        let bad = env.claude.join("projects").join("p").join("old1.jsonl");
        crate::token_usage::index_tests::append(&bad, "{ this is not json\n");
        let report = build_index(&roots(&env), &env.index_dir, now_for_tests(), &|_| {}).unwrap();
        assert_eq!(report.skipped_files, vec![(bad, 1)]);
        assert_eq!(report.lines_skipped, 1);
    }
}

// ---- this machine ----------------------------------------------------------

fn older_than_days() -> u64 {
    std::env::var("YMUX_VERIFY_OLDER_THAN_DAYS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(14)
}

/// The app's index directory and the roots it indexes: `~/.claude`,
/// `~/.codex` and every account profile in `<config_dir>/ymux/config.toml`
/// (parsed read-only; the config store itself is not used because loading it
/// can write), resolved by the same `account_dirs` / `scan_roots` the app
/// uses. Environment overrides the running app was started with count only if
/// this shell has them too.
fn real_setup() -> (PathBuf, Vec<ScanRoot>) {
    use crate::accounts::account_dirs;
    let dir = super::collect::index_dir().expect("no config directory");
    let accounts = {
        let path = crate::config::store::config_path();
        match fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str::<crate::config::model::Config>(&text)
                    .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
                    .accounts
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("cannot read {}: {e}", path.display()),
        }
    };
    let dirs = account_dirs(
        &accounts,
        |k| std::env::var(k).ok(),
        dirs::home_dir().as_deref(),
    );
    let roots = super::scan_roots(&dirs);
    println!("index dir: {}", dir.display());
    for r in &roots {
        println!("root {} [{}]: {}", r.provider, r.owner, r.dir.display());
    }
    (dir, roots)
}

/// Step 1. Builds the index the app will use from this machine's real history
/// and proves it complete: no skipped or unreadable file, no cut-short
/// listing, every shard written, and a fresh instance loaded from disk shows
/// the same figures having read nothing. WRITES inside the index directory
/// only; transcripts are only read. Run it with yMux closed:
/// `cargo test --no-default-features --lib -p ymux build_real_index_in_place -- --ignored --nocapture`
#[test]
#[ignore = "indexes this machine's real history into the app's real index directory"]
fn build_real_index_in_place() {
    let (dir, roots) = real_setup();
    let started = std::time::Instant::now();
    match build_index(&roots, &dir, now_for_tests(), &|_| {}) {
        Ok(r) => {
            println!("elapsed: {:?} in {} ticks", started.elapsed(), r.ticks);
            println!("files indexed: {}", r.files);
            println!("bytes indexed: {}", r.bytes);
            println!("events (live): {}", r.events);
            for (provider, account, total) in &r.tokens {
                println!("tokens {provider} [{account}]: {total}");
            }
            println!(
                "index on disk: {} bytes ({} MB)",
                r.disk_bytes,
                r.disk_bytes >> 20
            );
            println!("lines the parser skipped: {}", r.lines_skipped);
            for (path, n) in &r.skipped_files {
                println!("SKIPPED LINES: {n} in {}", path.display());
            }
            println!(
                "failed shard writes (all retried to success): {}",
                r.shard_write_failures
            );
            if !r.changed_during_run.is_empty() {
                println!("files that grew before the snapshot the comparison used:");
                for p in &r.changed_during_run {
                    println!("  {}", p.display());
                }
            }
            // An unparseable line is history the index could not count: a
            // deletion is not vouched for until someone looked. Only the
            // files on the deletion list count when there is one.
            let scope = std::env::var_os("YMUX_DELETION_LIST")
                .map(|f| read_deletion_list(Path::new(&f)).unwrap_or_else(|e| panic!("{e}")));
            let allow = std::env::var("YMUX_ALLOW_SKIPPED_LINES").is_ok_and(|v| v == "1");
            let problems = skipped_problems(&r.skipped_files, scope.as_deref(), allow);
            assert!(problems.is_empty(), "{}", problems.join("\n"));
        }
        Err(problems) => panic!("the index is NOT complete:\n{}", problems.join("\n")),
    }
}

/// Step 0 (read-only; writes only the file named by `YMUX_DELETION_LIST_OUT`).
/// Computes the files that may be deleted: a transcript whose own mtime is
/// older than `YMUX_VERIFY_OLDER_THAN_DAYS` (default 14) AND, for a Claude
/// file in a session directory, whose session's main transcript is older or
/// missing. Prints the count, the bytes and how many old files were kept back
/// because their session is still young; only `*.jsonl` files the index lists
/// are ever on it:
/// `$env:YMUX_DELETION_LIST_OUT = "<file>"; cargo test --no-default-features --lib -p ymux print_deletion_list -- --ignored --nocapture`
#[test]
#[ignore = "lists this machine's deletable transcripts into the file named by YMUX_DELETION_LIST_OUT"]
fn print_deletion_list() {
    let out = std::env::var_os("YMUX_DELETION_LIST_OUT")
        .expect("set YMUX_DELETION_LIST_OUT to the file the list is written to");
    let (_, roots) = real_setup();
    let days = older_than_days();
    let cutoff = now_for_tests().saturating_sub(days * 86400);
    let plan = plan_deletion(&take_snapshot(&roots), cutoff);
    let mut text = plan
        .paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    fs::write(&out, text)
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", Path::new(&out).display()));
    println!(
        "transcripts older than {days} days on the list: {}",
        plan.paths.len()
    );
    println!("bytes: {} ({} MB)", plan.bytes, plan.bytes >> 20);
    println!(
        "old files kept back because their session is still young: {}",
        plan.excluded_young_session
    );
    println!(
        "old files kept back because their session could not be told: {}",
        plan.excluded_unclear
    );
    println!("written to {}", Path::new(&out).display());
}

/// Step 2 (read-only; writes nothing anywhere). Every transcript older than
/// `YMUX_VERIFY_OLDER_THAN_DAYS` (default 14), or every transcript with
/// `YMUX_VERIFY_ALL=1`, must have a shard that is current, belongs to it and
/// covers it exactly:
/// `cargo test --no-default-features --lib -p ymux verify_real_index_covers -- --ignored --nocapture`
#[test]
#[ignore = "read-only check of this machine's real index against its real transcripts"]
fn verify_real_index_covers() {
    let (dir, roots) = real_setup();
    let all = std::env::var("YMUX_VERIFY_ALL").is_ok_and(|v| v == "1");
    let days = older_than_days();
    let cutoff = (!all).then(|| now_for_tests().saturating_sub(days * 86400));
    let selection = selection_from_env(Selection::Mtime(cutoff));
    let report = verify_coverage(&roots, &dir, &selection);
    match &selection {
        Selection::List(_) => println!("checking exactly the files of the deletion list"),
        Selection::Mtime(_) if all => println!("checking EVERY transcript"),
        Selection::Mtime(_) => {
            println!("checking transcripts last modified more than {days} days ago");
        }
    }
    println!("files checked: {}", report.checked);
    println!(
        "bytes covered: {} ({} MB)",
        report.bytes,
        report.bytes >> 20
    );
    for (path, why) in &report.offenders {
        println!("NOT COVERED: {} -- {why}", path.display());
    }
    assert!(
        report.offenders.is_empty(),
        "{} file(s) are not fully covered",
        report.offenders.len()
    );
}

/// Step 3. Proves the plan on a COPY: the real index directory is copied to a
/// temp directory, a fresh instance there is told that every transcript older
/// than the cutoff is gone, and the figures must not change. No real file is
/// modified; transcripts are only listed.
/// `cargo test --no-default-features --lib -p ymux simulate_real_deletion -- --ignored --nocapture`
#[test]
#[ignore = "simulates deleting old transcripts on a copy of this machine's real index"]
fn simulate_real_deletion() {
    let (dir, roots) = real_setup();
    let work = tempfile::tempdir().unwrap();
    let now = now_for_tests();
    let selection = selection_from_env(Selection::Mtime(Some(
        now.saturating_sub(older_than_days() * 86400),
    )));
    let report = simulate_deletion(&roots, &dir, &work.path().join("copy"), now, &selection);
    println!("simulated-deleted transcripts: {}", report.victims);
    println!("before: {:?}", report.tokens_before);
    println!("after:  {:?}", report.tokens_after);
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    println!("identical figures, identical heatmap hours, retired.json written, victim shards gone from the copy");
}
