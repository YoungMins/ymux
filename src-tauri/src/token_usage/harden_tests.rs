//! Hardening of the retirement path (round 2): a message is never counted
//! nowhere, a file is retired only once it is known to be gone, an unusable
//! store is never replaced, and a complete summary outlives an unfinished
//! replacement.
use super::collect::collect_with;
use super::disk::Budget;
use super::index::shard_path;
use super::index_tests::{env, fresh, line, roots, snap, total, Env, NOW};
use super::TokenSnapshot;
use std::fs;
use std::path::PathBuf;

fn cp(env: &Env, name: &str) -> PathBuf {
    env.claude.join("projects").join("p").join(name)
}

fn retired_json(env: &Env) -> PathBuf {
    env.index_dir.join("retired.json")
}

/// Run enough refreshes for a two-phase retirement to finish (the shards of
/// retired files are deleted a refresh after the store holding them is
/// written).
pub(super) fn settle_all(index: &mut super::index::Index, env: &Env) -> TokenSnapshot {
    let mut last = snap(index, env);
    for _ in 0..3 {
        last = snap(index, env);
    }
    last
}

// ---- A1: a retiring loser must not depend on the live winner ------------------

#[test]
fn a_retired_loser_does_not_depend_on_the_live_winner_surviving() {
    let env = env();
    fs::write(
        cp(&env, "a.jsonl"),
        line("s", "2026-09-21T00:00:00Z", "m", 10),
    )
    .unwrap();
    fs::write(
        cp(&env, "z.jsonl"),
        line("t", "2026-09-21T00:00:00Z", "m", 30),
    )
    .unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 30);
    fs::remove_file(cp(&env, "a.jsonl")).unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 30);
    // The winner's file is rewritten without that message, then deleted.
    fs::write(
        cp(&env, "z.jsonl"),
        line("t", "2026-09-21T01:00:00Z", "other", 1),
    )
    .unwrap();
    assert_eq!(
        total(&snap(&mut index, &env)),
        31,
        "the message is not lost"
    );
    fs::remove_file(cp(&env, "z.jsonl")).unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 31);
    assert_eq!(total(&snap(&mut fresh(Some(&env.index_dir)), &env)), 31);
}

// ---- A2: a larger copy that shows up after the smaller one retired -----------

#[test]
fn a_larger_copy_indexed_after_a_smaller_one_retired_upgrades_it_in_place() {
    let env = env();
    fs::write(
        cp(&env, "a.jsonl"),
        line("s", "2026-09-21T00:00:00Z", "m", 10),
    )
    .unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 10);
    fs::remove_file(cp(&env, "a.jsonl")).unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 10);
    fs::write(
        cp(&env, "b.jsonl"),
        line("t", "2026-09-21T00:00:00Z", "m", 30),
    )
    .unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 30);
    // A smaller or equal copy changes nothing; the value never goes down.
    fs::write(
        cp(&env, "c.jsonl"),
        line("u", "2026-09-21T00:00:00Z", "m", 5),
    )
    .unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 30);
    fs::remove_file(cp(&env, "b.jsonl")).unwrap();
    fs::remove_file(cp(&env, "c.jsonl")).unwrap();
    assert_eq!(total(&settle_all(&mut index, &env)), 30);
    assert_eq!(total(&snap(&mut fresh(Some(&env.index_dir)), &env)), 30);
}

// ---- C: an unusable store is left alone --------------------------------------

#[test]
fn an_unusable_retired_store_is_left_in_place_and_nothing_is_deleted() {
    let garbage: [&[u8]; 3] = [
        b"not json at all",
        b"{\"format\":1,\"claude_keys\":[1,2",
        b"{\"format\":99,\"from\":\"the future\"}",
    ];
    for bytes in garbage {
        let env = env();
        fs::write(
            cp(&env, "gone.jsonl"),
            line("s", "2026-09-21T00:00:00Z", "g", 10),
        )
        .unwrap();
        fs::write(
            cp(&env, "live.jsonl"),
            line("t", "2026-09-21T00:00:00Z", "l", 20),
        )
        .unwrap();
        let mut first = fresh(Some(&env.index_dir));
        snap(&mut first, &env);
        drop(first);
        fs::write(retired_json(&env), bytes).unwrap();
        let shard = shard_path(&env.index_dir, &cp(&env, "gone.jsonl"));
        let mut index = fresh(Some(&env.index_dir));
        snap(&mut index, &env);
        fs::remove_file(cp(&env, "gone.jsonl")).unwrap();
        // The deleted file is still counted, nothing is written or deleted.
        for _ in 0..4 {
            let got = snap(&mut index, &env);
            assert_eq!(total(&got), 30, "{:?}", String::from_utf8_lossy(bytes));
            assert!(got.retired_unavailable);
            assert!(!got.retired_unavailable_reason.is_empty());
        }
        assert_eq!(fs::read(retired_json(&env)).unwrap(), bytes);
        assert!(
            shard.is_file(),
            "the shard stays while the store is unusable"
        );
        // And across a restart.
        let got = snap(&mut fresh(Some(&env.index_dir)), &env);
        assert_eq!(total(&got), 30);
        assert_eq!(fs::read(retired_json(&env)).unwrap(), bytes);
    }
}

// ---- D: a complete summary outlives an unfinished replacement -----------------

#[test]
fn a_file_deleted_in_the_middle_of_a_reparse_keeps_its_complete_summary() {
    let env = env();
    let old: String = (0..60)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("old{i}"), 2))
        .collect();
    fs::write(cp(&env, "a.jsonl"), &old).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 120);
    // Rewritten with other (and more) content, re-read in small steps.
    let new: String = (0..200)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("new{i}"), 1))
        .collect();
    fs::write(cp(&env, "a.jsonl"), &new).unwrap();
    let part = collect_with(&mut index, &roots(&env), NOW, Budget::bytes(3000));
    assert!(part.indexing);
    fs::remove_file(cp(&env, "a.jsonl")).unwrap();
    let after = snap(&mut index, &env);
    assert!(
        total(&after) >= 120,
        "the complete old summary is kept: {}",
        total(&after)
    );
    assert_eq!(
        total(&snap(&mut fresh(Some(&env.index_dir)), &env)),
        total(&after)
    );
}
