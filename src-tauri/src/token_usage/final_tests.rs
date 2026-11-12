//! Final hardening round: a stale root is probed before it is retired, the
//! last complete summary is used while a re-parse runs, corrupt data from disk
//! cannot panic, and two processes cannot overwrite each other's retirements.
use super::aggregate::aggregate;
use super::collect::{collect_with, lock_recovering};
use super::disk::Budget;
use super::index::shard_path;
use super::index_tests::{env, fresh, line, roots, snap, total, Env, NOW};
use super::retired::{decode_retired, migrate_retired, RETIRED_FORMAT};
use super::{scan_roots, ScanRoot};
use crate::accounts::AgentDirs;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn cp(env: &Env, name: &str) -> PathBuf {
    env.claude.join("projects").join("p").join(name)
}

// ---- item 6: nothing read from disk may panic ---------------------------------

#[test]
fn a_corrupt_huge_retired_hour_is_skipped_instead_of_panicking() {
    let json = format!(
        "{{\"format\":{RETIRED_FORMAT},\"hours\":[{{\"hour\":{},\"account\":\"\",\"provider\":\"claude\",\"total\":5}},{{\"hour\":{},\"account\":\"\",\"provider\":\"claude\",\"total\":7}}]}}",
        u64::MAX,
        u64::MAX / 3600 + 1,
    );
    let retired = decode_retired(json.as_bytes(), RETIRED_FORMAT, &migrate_retired).unwrap();
    let env = env();
    let mut snapshot = snap(&mut fresh(None), &env);
    aggregate(&mut snapshot, &[], &[], &retired);
    assert!(snapshot.hourly.is_empty(), "{:?}", snapshot.hourly);
}

#[test]
fn a_poisoned_lock_is_recovered_not_fatal() {
    let m = Arc::new(Mutex::new(5));
    let m2 = Arc::clone(&m);
    let _ = std::thread::spawn(move || {
        let _guard = m2.lock().unwrap();
        panic!("poison on purpose");
    })
    .join();
    assert!(m.lock().is_err());
    assert_eq!(*lock_recovering(&m), 5);
}

// ---- item 5: the complete summary is used until the re-parse finishes ----------

#[test]
fn figures_do_not_dip_while_a_file_is_re_parsed_from_scratch() {
    let env = env();
    let path = cp(&env, "a.jsonl");
    let old: String = (0..100)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("old{i}"), 2))
        .collect();
    fs::write(&path, &old).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 200);
    // Rewritten (not appended to) and longer: parsed from scratch.
    let new: String = (0..150)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("new{i}"), 3))
        .collect();
    fs::write(&path, &new).unwrap();
    let part = collect_with(&mut index, &roots(&env), NOW, Budget::bytes(4000));
    assert!(part.indexing);
    assert_eq!(index.superseded_count(), 1);
    assert_eq!(total(&part), 200, "the old complete summary stays in use");
    // A retirement in that window folds the same, larger-or-equal figure.
    fs::remove_file(&path).unwrap();
    let after = snap(&mut index, &env);
    assert!(total(&after) >= 200, "{}", total(&after));
}

// ---- item 4: a root that is no longer configured is probed, not retired --------

fn two_roots(env: &Env) -> (Vec<ScanRoot>, PathBuf) {
    let b = env._dir.path().join("b");
    fs::create_dir_all(b.join("claude/projects/p")).unwrap();
    fs::create_dir_all(b.join("codex/sessions")).unwrap();
    let dirs = |claude: PathBuf, codex: PathBuf| AgentDirs {
        claude: claude.display().to_string(),
        codex: codex.display().to_string(),
        claude_env: None,
    };
    let both = scan_roots(&[
        (String::new(), dirs(env.claude.clone(), env.codex.clone())),
        ("b".into(), dirs(b.join("claude"), b.join("codex"))),
    ]);
    (both, b)
}

#[test]
fn files_under_a_root_that_is_merely_not_configured_any_more_are_kept_and_counted() {
    let env = env();
    let (both, b) = two_roots(&env);
    fs::write(
        cp(&env, "a.jsonl"),
        line("s", "2026-09-21T00:00:00Z", "a", 10),
    )
    .unwrap();
    let bfile = b.join("claude").join("projects").join("p").join("x.jsonl");
    fs::write(&bfile, line("t", "2026-09-21T00:00:00Z", "x", 20)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    let first = collect_with(&mut index, &both, NOW, Budget::unlimited());
    assert_eq!(total(&first), 30);
    let only_a = roots(&env);
    for _ in 0..3 {
        let s = collect_with(&mut index, &only_a, NOW, Budget::unlimited());
        assert_eq!(total(&s), 30);
    }
    assert_eq!(index.retired().key_count(), 0, "nothing was retired");
    assert!(index.summary(&bfile).is_some());
    assert!(shard_path(&env.index_dir, &bfile).exists());
    // The root is configured again: nothing double counts.
    let back = collect_with(&mut index, &both, NOW, Budget::unlimited());
    assert_eq!(total(&back), 30);
    // Now the files really are gone (their root folder exists, or its parent):
    // they are retired, and the figure still holds.
    collect_with(&mut index, &only_a, NOW, Budget::unlimited());
    fs::remove_dir_all(b.join("claude")).unwrap();
    fs::remove_dir_all(b.join("codex")).unwrap();
    for _ in 0..3 {
        let s = collect_with(&mut index, &only_a, NOW, Budget::unlimited());
        assert_eq!(total(&s), 30);
    }
    assert!(index.retired().key_count() > 0);
}

// ---- item 3: two processes on one index directory -----------------------------

fn age_file(path: &std::path::Path, secs: u64) {
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

fn store_keys(env: &Env) -> usize {
    let bytes = fs::read(env.index_dir.join("retired.json")).unwrap();
    decode_retired(&bytes, RETIRED_FORMAT, &migrate_retired)
        .unwrap()
        .key_count()
}

/// `p` retires `x` and finishes both phases (its shard is deleted) before `q`,
/// which never knew `x`, retires `y` and commits. Whichever instance is which,
/// and whichever phase 2 comes first, no history is lost.
fn two_writers(p: usize, q: usize, p_finishes_first: bool, skip_sync: bool) {
    let env = env();
    fs::write(
        cp(&env, "keep.jsonl"),
        line("k", "2026-09-21T00:00:00Z", "keep", 40),
    )
    .unwrap();
    let mut ix = vec![fresh(Some(&env.index_dir)), fresh(Some(&env.index_dir))];
    for index in &mut ix {
        // A process that races without the lock (older build, lost lock).
        index.use_lock = false;
        snap(index, &env);
        // Either the look before the refresh sees the other's store, or (the
        // race) only the check at the commit does.
        index.skip_sync = skip_sync;
    }
    let x = cp(&env, "x.jsonl");
    fs::write(&x, line("s", "2026-09-21T00:00:00Z", "x", 10)).unwrap();
    snap(&mut ix[p], &env);
    fs::remove_file(&x).unwrap();
    if p_finishes_first {
        for _ in 0..4 {
            snap(&mut ix[p], &env);
        }
        assert!(
            !shard_path(&env.index_dir, &x).exists(),
            "x's shard is gone"
        );
    } else {
        snap(&mut ix[p], &env);
    }
    let y = cp(&env, "y.jsonl");
    fs::write(&y, line("t", "2026-09-21T00:00:00Z", "y", 20)).unwrap();
    snap(&mut ix[q], &env);
    fs::remove_file(&y).unwrap();
    for _ in 0..4 {
        snap(&mut ix[q], &env);
        snap(&mut ix[p], &env);
    }
    assert_eq!(store_keys(&env), 2, "both retirements are in the store");
    let mut restarted = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut restarted, &env)), 70);
    assert_eq!(total(&snap(&mut ix[q], &env)), 70);
    if !skip_sync {
        // (Without the look, p still shows its own older view.)
        assert_eq!(total(&snap(&mut ix[p], &env)), 70);
    }
}

#[test]
fn a_commit_never_overwrites_a_store_another_process_wrote() {
    for (p, q) in [(0, 1), (1, 0)] {
        for p_finishes_first in [true, false] {
            for skip_sync in [false, true] {
                two_writers(p, q, p_finishes_first, skip_sync);
            }
        }
    }
}

#[test]
fn a_second_process_retires_nothing_while_the_lock_is_held() {
    let env = env();
    let a = cp(&env, "a.jsonl");
    fs::write(&a, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    fs::write(
        cp(&env, "b.jsonl"),
        line("t", "2026-09-21T00:00:00Z", "b", 20),
    )
    .unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 30);
    let lock = env.index_dir.join("retired.lock");
    assert!(!lock.exists(), "released at the end of a refresh");
    fs::write(&lock, "someone else").unwrap();
    fs::remove_file(&a).unwrap();
    for _ in 0..4 {
        assert_eq!(total(&snap(&mut index, &env)), 30);
    }
    assert!(index.summary(&a).is_some(), "still live, still counted");
    assert!(!env.index_dir.join("retired.json").exists());
    assert_eq!(fs::read_to_string(&lock).unwrap(), "someone else");
    // The holder finishes.
    fs::remove_file(&lock).unwrap();
    for _ in 0..4 {
        assert_eq!(total(&snap(&mut index, &env)), 30);
    }
    assert!(index.summary(&a).is_none());
    assert_eq!(store_keys(&env), 1);
    assert!(!lock.exists());
}

#[test]
fn a_stale_lock_never_wedges_the_index() {
    let env = env();
    let a = cp(&env, "a.jsonl");
    fs::write(&a, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    snap(&mut index, &env);
    let lock = env.index_dir.join("retired.lock");
    fs::write(&lock, "a process that died").unwrap();
    age_file(&lock, 10 * 60);
    fs::remove_file(&a).unwrap();
    for _ in 0..4 {
        assert_eq!(total(&snap(&mut index, &env)), 10);
    }
    assert!(index.summary(&a).is_none(), "the stale lock was taken over");
    assert_eq!(store_keys(&env), 1);
    assert!(!lock.exists());
    // A lock from the future (clock set back) is judged by its distance too.
    fs::write(&lock, "from the future").unwrap();
    let ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(ahead)
        .unwrap();
    let b = cp(&env, "b.jsonl");
    fs::write(&b, line("t", "2026-09-21T00:00:00Z", "b", 5)).unwrap();
    snap(&mut index, &env);
    fs::remove_file(&b).unwrap();
    for _ in 0..4 {
        assert_eq!(total(&snap(&mut index, &env)), 15);
    }
    assert_eq!(store_keys(&env), 2);
}

// ---- review: the per-refresh look at retired.json is a stat, not a read -------

#[test]
fn idle_refreshes_do_not_read_the_retired_store_and_an_external_change_is_adopted() {
    let env = env();
    let keep = cp(&env, "keep.jsonl");
    fs::write(&keep, line("k", "2026-09-21T00:00:00Z", "keep", 40)).unwrap();
    let x = cp(&env, "x.jsonl");
    fs::write(&x, line("s", "2026-09-21T00:00:00Z", "x", 10)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    snap(&mut index, &env);
    fs::remove_file(&x).unwrap();
    for _ in 0..4 {
        snap(&mut index, &env);
    }
    assert_eq!(store_keys(&env), 1, "the store exists");
    let reads = index.sync_reads;
    for _ in 0..10 {
        assert_eq!(total(&snap(&mut index, &env)), 50);
    }
    assert_eq!(
        index.sync_reads, reads,
        "idle refreshes only stat the store"
    );
    // Another process retires `y` and writes the store: length differs.
    let mut other = fresh(Some(&env.index_dir));
    snap(&mut other, &env);
    let y = cp(&env, "y.jsonl");
    fs::write(&y, line("t", "2026-09-21T00:00:00Z", "y", 20)).unwrap();
    snap(&mut other, &env);
    fs::remove_file(&y).unwrap();
    for _ in 0..4 {
        snap(&mut other, &env);
    }
    assert_eq!(store_keys(&env), 2);
    snap(&mut index, &env);
    assert!(index.sync_reads > reads, "the changed store was read");
    assert_eq!(index.retired().key_count(), 2, "and adopted");
}

// ---- review: a shard under no configured root is probed before it is retired ---

fn orphaned_b_file(env: &Env) -> (PathBuf, PathBuf) {
    let (both, b) = two_roots(env);
    fs::write(
        cp(env, "a.jsonl"),
        line("s", "2026-09-21T00:00:00Z", "a", 10),
    )
    .unwrap();
    let bfile = b.join("claude").join("projects").join("p").join("x.jsonl");
    fs::write(&bfile, line("t", "2026-09-21T00:00:00Z", "x", 20)).unwrap();
    let mut first = fresh(Some(&env.index_dir));
    assert_eq!(
        total(&collect_with(&mut first, &both, NOW, Budget::unlimited())),
        30
    );
    drop(first);
    (bfile, b)
}

#[test]
fn a_startup_orphan_that_still_exists_is_held_and_counted_not_retired() {
    let env = env();
    let (bfile, _b) = orphaned_b_file(&env);
    let only_a = roots(&env);
    let mut index = fresh(Some(&env.index_dir));
    for _ in 0..4 {
        assert_eq!(
            total(&collect_with(&mut index, &only_a, NOW, Budget::unlimited())),
            30
        );
    }
    assert_eq!(index.retired().key_count(), 0, "nothing was retired");
    assert!(index.summary(&bfile).is_some());
    assert!(shard_path(&env.index_dir, &bfile).exists());
    // Unknown is held too.
    let mut unsure = fresh(Some(&env.index_dir));
    unsure.set_probe(Box::new(|_, _| crate::token_usage::disk::Presence::Unknown));
    for _ in 0..4 {
        assert_eq!(
            total(&collect_with(
                &mut unsure,
                &only_a,
                NOW,
                Budget::unlimited()
            )),
            30
        );
    }
    assert_eq!(unsure.retired().key_count(), 0);
}

#[test]
fn a_startup_orphan_that_is_confirmed_gone_is_retired() {
    let env = env();
    let (bfile, _b) = orphaned_b_file(&env);
    fs::remove_file(&bfile).unwrap();
    let only_a = roots(&env);
    let mut index = fresh(Some(&env.index_dir));
    for _ in 0..4 {
        assert_eq!(
            total(&collect_with(&mut index, &only_a, NOW, Budget::unlimited())),
            30
        );
    }
    assert!(index.retired().key_count() > 0, "retired once gone");
    assert!(index.summary(&bfile).is_none());
}
