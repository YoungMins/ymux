//! Usage history must survive the deletion of the transcripts it came from:
//! no figure may change when a file, a whole root or everything is deleted,
//! however the index is restarted in between.
use super::collect::collect_with;
use super::disk::Budget;
use super::index::{decode_shard, shard_path, Decoded, FileSummary, Index, INDEX_VERSION};
use super::index_tests::{env, fresh, line, roots, snap, total, Env, NOW};
use super::retired::{decode_retired, Retired, StoreError, RETIRED_FORMAT};
use super::TokenSnapshot;
use std::fs;
use std::path::PathBuf;

/// Every figure the frontend derives usage from, except the file counts
/// (which legitimately drop when files are deleted).
fn figures(s: &TokenSnapshot) -> String {
    let cards: Vec<_> = s
        .providers
        .iter()
        .map(|p| (p.id, &p.account, p.five_hour, p.seven_day, p.all_time))
        .collect();
    serde_json::to_string(&(cards, &s.rows, &s.hourly)).unwrap()
}

fn count(time: &str, total: u64) -> String {
    format!("{{\"timestamp\":\"{time}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{total},\"total_tokens\":{total}}}}}}}}}\n")
}

const HEADER: &str = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"sess\",\"cwd\":\"/c\"}}\n";

fn linep(time: &str, id: &str, input: u64, cwd: &str) -> String {
    line("s", time, id, input).replace("/proj", cwd)
}

/// Claude and Codex files whose events span the five-hour window, the
/// seven-day window, the heatmap and the all-time-only tier, with a message
/// shared between two files and one Codex session split across three.
fn seed(env: &Env) -> Vec<PathBuf> {
    let c = |name: &str| env.claude.join("projects").join("p").join(name);
    let x = |name: &str| env.codex.join("sessions").join(name);
    let files = vec![
        (
            c("a.jsonl"),
            [
                linep("2026-06-01T00:00:00Z", "a_old", 100, "/old"),
                linep("2026-09-21T00:00:00Z", "a_day", 10, "/proj"),
                linep("2026-09-21T01:00:00Z", "shared", 5, "/proj"),
            ]
            .concat(),
        ),
        (
            c("b.jsonl"),
            [
                linep("2026-09-21T11:00:00Z", "b_hot", 7, "/proj"),
                linep("2026-09-21T01:00:00Z", "shared", 5, "/proj"),
                linep("2026-09-18T05:00:00Z", "b_week", 3, "/other"),
            ]
            .concat(),
        ),
        (
            x("c1.jsonl"),
            format!("{HEADER}{}", count("2026-09-21T00:00:00Z", 10)),
        ),
        (
            x("c2.jsonl"),
            format!("{HEADER}{}", count("2026-09-21T01:00:00Z", 25)),
        ),
        (
            x("c3.jsonl"),
            format!("{HEADER}{}", count("2026-09-21T11:30:00Z", 40)),
        ),
    ];
    for (path, text) in &files {
        fs::write(path, text).unwrap();
    }
    files.into_iter().map(|(p, _)| p).collect()
}

const SEED_TOTAL: u64 = 100 + 10 + 5 + 7 + 3 + 40;

fn restarted(env: &Env) -> Index {
    fresh(Some(&env.index_dir))
}

#[test]
fn deleting_any_one_file_changes_no_figure() {
    for victim in 0..5 {
        let env = env();
        let paths = seed(&env);
        let mut index = restarted(&env);
        let before = snap(&mut index, &env);
        assert_eq!(total(&before), SEED_TOTAL);
        fs::remove_file(&paths[victim]).unwrap();
        let after = snap(&mut index, &env);
        assert_eq!(figures(&after), figures(&before), "victim {victim}");
        // And after a restart, and after another one.
        for _ in 0..2 {
            let mut again = restarted(&env);
            assert_eq!(
                figures(&snap(&mut again, &env)),
                figures(&before),
                "victim {victim}"
            );
        }
    }
}

#[test]
fn deleting_every_file_one_by_one_and_all_at_once_changes_no_figure() {
    let env = env();
    let paths = seed(&env);
    let mut index = restarted(&env);
    let before = snap(&mut index, &env);
    for path in &paths {
        fs::remove_file(path).unwrap();
        assert_eq!(figures(&snap(&mut index, &env)), figures(&before));
    }
    assert_eq!(before.providers[0].files, 2);
    let after = snap(&mut index, &env);
    assert_eq!((after.providers[0].files, after.providers[1].files), (0, 0));
    assert_eq!(index.live_totals(), (0, 0));

    let all_at_once = super::index_tests::env();
    let paths = seed(&all_at_once);
    let mut index = restarted(&all_at_once);
    let before = snap(&mut index, &all_at_once);
    for path in &paths {
        fs::remove_file(path).unwrap();
    }
    assert_eq!(figures(&snap(&mut index, &all_at_once)), figures(&before));
    assert_eq!(
        figures(&snap(&mut restarted(&all_at_once), &all_at_once)),
        figures(&before)
    );
}

#[test]
fn deleting_a_whole_root_changes_no_figure_and_restart_still_shows_it() {
    let env = env();
    seed(&env);
    let mut index = restarted(&env);
    let before = snap(&mut index, &env);
    fs::remove_dir_all(&env.claude).unwrap();
    fs::remove_dir_all(&env.codex).unwrap();
    assert_eq!(figures(&snap(&mut index, &env)), figures(&before));
    assert_eq!(figures(&snap(&mut restarted(&env), &env)), figures(&before));
    // The old shards were folded into the retired store, not kept.
    let mut index = restarted(&env);
    super::harden_tests::settle_all(&mut index, &env);
    let shards = fs::read_dir(&env.index_dir)
        .unwrap()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with("retired.json"))
        .count();
    assert_eq!(shards, 0);
}

#[test]
fn a_root_deleted_while_closed_is_retired_at_the_next_start() {
    let env = env();
    seed(&env);
    let before = {
        let mut index = restarted(&env);
        snap(&mut index, &env)
    };
    fs::remove_dir_all(&env.claude).unwrap();
    fs::remove_dir_all(&env.codex).unwrap();
    for _ in 0..2 {
        assert_eq!(figures(&snap(&mut restarted(&env), &env)), figures(&before));
    }
}

#[test]
fn a_retired_claude_message_still_dedups_against_a_live_copy() {
    for delete_fork_first in [true, false] {
        let env = env();
        let p = |n: &str| env.claude.join("projects").join("p").join(n);
        fs::write(
            p("old.jsonl"),
            line("old", "2026-09-21T00:00:00Z", "msg_1", 10),
        )
        .unwrap();
        fs::write(
            p("fork.jsonl"),
            [
                line("fork", "2026-09-21T00:00:00Z", "msg_1", 10),
                line("fork", "2026-09-21T01:00:00Z", "msg_2", 4),
            ]
            .concat(),
        )
        .unwrap();
        let mut index = restarted(&env);
        let before = snap(&mut index, &env);
        assert_eq!(total(&before), 14);
        let order = if delete_fork_first {
            ["fork.jsonl", "old.jsonl"]
        } else {
            ["old.jsonl", "fork.jsonl"]
        };
        for name in order {
            fs::remove_file(p(name)).unwrap();
            assert_eq!(figures(&snap(&mut index, &env)), figures(&before), "{name}");
        }
    }
    // The copy shows up only after the original is retired.
    let env = env();
    let p = |n: &str| env.claude.join("projects").join("p").join(n);
    fs::write(
        p("old.jsonl"),
        line("old", "2026-09-21T00:00:00Z", "msg_1", 10),
    )
    .unwrap();
    let mut index = restarted(&env);
    snap(&mut index, &env);
    fs::remove_file(p("old.jsonl")).unwrap();
    snap(&mut index, &env);
    fs::write(
        p("later_fork.jsonl"),
        [
            line("fork", "2026-09-21T00:00:00Z", "msg_1", 10),
            line("fork", "2026-09-21T01:00:00Z", "msg_2", 4),
        ]
        .concat(),
    )
    .unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 14);
    assert_eq!(total(&snap(&mut restarted(&env), &env)), 14);
}

#[test]
fn a_larger_live_copy_wins_over_a_smaller_retired_loser_without_double_counting() {
    let env = env();
    let p = |n: &str| env.claude.join("projects").join("p").join(n);
    // `a` lists first but is the smaller copy: retiring it must not count it.
    fs::write(p("a.jsonl"), line("s", "2026-09-21T00:00:00Z", "m", 10)).unwrap();
    fs::write(p("z.jsonl"), line("s", "2026-09-21T00:00:00Z", "m", 30)).unwrap();
    let mut index = restarted(&env);
    let before = snap(&mut index, &env);
    assert_eq!(total(&before), 30);
    fs::remove_file(p("a.jsonl")).unwrap();
    assert_eq!(figures(&snap(&mut index, &env)), figures(&before));
    fs::remove_file(p("z.jsonl")).unwrap();
    assert_eq!(figures(&snap(&mut index, &env)), figures(&before));
}

#[test]
fn a_reappearing_file_is_not_counted_twice() {
    let env = env();
    let paths = seed(&env);
    let texts: Vec<String> = paths
        .iter()
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();
    let mut index = restarted(&env);
    let before = snap(&mut index, &env);
    for path in &paths {
        fs::remove_file(path).unwrap();
    }
    assert_eq!(figures(&snap(&mut index, &env)), figures(&before));
    // The unplugged directory comes back, in this process and after a restart.
    for path_and_text in paths.iter().zip(&texts) {
        fs::write(path_and_text.0, path_and_text.1).unwrap();
    }
    assert_eq!(figures(&snap(&mut index, &env)), figures(&before));
    assert_eq!(figures(&snap(&mut restarted(&env), &env)), figures(&before));
    // Gone and back again once more: still the same.
    for path in &paths {
        fs::remove_file(path).unwrap();
    }
    assert_eq!(figures(&snap(&mut restarted(&env), &env)), figures(&before));
}

#[test]
fn a_reappearing_file_with_other_content_upgrades_the_larger_copy_and_adds_the_new_message() {
    let env = env();
    let p = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(
        &p,
        [
            line("s", "2026-09-21T00:00:00Z", "kept", 10),
            line("s", "2026-09-21T01:00:00Z", "kept2", 20),
        ]
        .concat(),
    )
    .unwrap();
    let mut index = restarted(&env);
    assert_eq!(total(&snap(&mut index, &env)), 30);
    fs::remove_file(&p).unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 30);
    // Same path, different content: "kept" again, now larger (99 against the
    // 10 that was folded) and one new message. The difference of the larger
    // copy is added to the key's own bucket: 30 + 89 + 5.
    fs::write(
        &p,
        [
            line("s", "2026-09-21T00:00:00Z", "kept", 99),
            line("s", "2026-09-21T02:00:00Z", "fresh", 5),
        ]
        .concat(),
    )
    .unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 124);
    fs::remove_file(&p).unwrap();
    assert_eq!(total(&snap(&mut restarted(&env), &env)), 124);
}

#[test]
fn a_file_indexed_only_in_part_keeps_what_was_indexed_when_it_vanishes() {
    let env = env();
    let p = env.claude.join("projects").join("p").join("big.jsonl");
    let text: String = (0..200)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("m{i}"), 2))
        .collect();
    fs::write(&p, &text).unwrap();
    let mut index = restarted(&env);
    let part = collect_with(&mut index, &roots(&env), NOW, Budget::bytes(4000));
    assert!(part.indexing);
    let indexed = total(&part);
    assert!(indexed > 0 && indexed < 400, "{indexed}");
    fs::remove_file(&p).unwrap();
    let after = snap(&mut index, &env);
    assert_eq!(total(&after), indexed);
    assert!(!after.indexing);
    assert_eq!(total(&snap(&mut restarted(&env), &env)), indexed);
}

#[test]
fn a_file_deleted_before_it_was_ever_indexed_is_unknown() {
    let env = env();
    let p = env.claude.join("projects").join("p").join("never.jsonl");
    fs::write(&p, line("s", "2026-09-21T00:00:00Z", "m", 10)).unwrap();
    let mut index = restarted(&env);
    // The listing sees it, the budget runs out before it is read.
    let none = collect_with(&mut index, &roots(&env), NOW, Budget::bytes(0));
    assert_eq!(total(&none), 0);
    fs::remove_file(&p).unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 0);
    assert_eq!(total(&snap(&mut restarted(&env), &env)), 0);
}

#[test]
fn old_retired_history_is_folded_and_figures_do_not_change() {
    let env = env();
    let p = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(
        &p,
        [
            line("s", "2026-09-19T00:00:00Z", "two_days_old", 10),
            line("s", "2026-09-21T11:00:00Z", "fresh", 4),
        ]
        .concat(),
    )
    .unwrap();
    let mut index = restarted(&env);
    snap(&mut index, &env);
    fs::remove_file(&p).unwrap();
    let after = snap(&mut index, &env);
    assert_eq!(total(&after), 14);
    assert_eq!(index.retired().recent().len(), 2);
    assert_eq!(after.rows[0].five_hour.total, 4);
    // Ten days later nothing is in a window any more, but all-time and the
    // heatmap still hold it, now as aggregates.
    let later = NOW + 10 * 86400;
    let aged = collect_with(&mut index, &roots(&env), later, Budget::unlimited());
    assert_eq!(total(&aged), 14);
    assert_eq!(aged.rows[0].seven_day.total, 0);
    assert_eq!(aged.hourly.iter().map(|b| b.total).sum::<u64>(), 14);
    assert!(index.retired().recent().is_empty());
    assert_eq!(index.retired().rollup().len(), 1);
    // The fold was persisted.
    let again = collect_with(
        &mut restarted(&env),
        &roots(&env),
        later,
        Budget::unlimited(),
    );
    assert_eq!(figures(&again), figures(&aged));
}

#[test]
fn retired_history_beyond_the_heatmap_window_stays_in_all_time_only() {
    let env = env();
    let p = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(&p, line("s", "2026-09-21T00:00:00Z", "m", 10)).unwrap();
    let mut index = restarted(&env);
    snap(&mut index, &env);
    fs::remove_file(&p).unwrap();
    snap(&mut index, &env);
    let much_later = NOW + 400 * 86400;
    let s = collect_with(&mut index, &roots(&env), much_later, Budget::unlimited());
    assert_eq!(total(&s), 10);
    assert!(s.hourly.is_empty());
    assert_eq!(s.rows[0].all_time.total, 10);
}

#[test]
fn a_codex_session_continuing_after_its_start_was_deleted_counts_only_the_increase() {
    let env = env();
    let x = |n: &str| env.codex.join("sessions").join(n);
    fs::write(
        x("a.jsonl"),
        format!("{HEADER}{}", count("2026-09-21T00:00:00Z", 10)),
    )
    .unwrap();
    let mut index = restarted(&env);
    assert_eq!(total(&snap(&mut index, &env)), 10);
    fs::remove_file(x("a.jsonl")).unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 10);
    // The session is resumed in a new file whose counter carries on.
    fs::write(
        x("b.jsonl"),
        format!(
            "{HEADER}{}{}",
            count("2026-09-21T00:00:00Z", 10),
            count("2026-09-21T02:00:00Z", 18)
        ),
    )
    .unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 18);
    assert_eq!(total(&snap(&mut restarted(&env), &env)), 18);
}

// ---- durability -----------------------------------------------------------

fn retired_path(env: &Env) -> PathBuf {
    env.index_dir.join("retired.json")
}

fn aside(env: &Env, label: &str) -> Vec<PathBuf> {
    fs::read_dir(&env.index_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("retired.json.{label}-")))
        })
        .collect()
}

/// An index with one retired file (10 tokens) and one live file (20).
fn with_one_retired(env: &Env) -> TokenSnapshot {
    let p = |n: &str| env.claude.join("projects").join("p").join(n);
    fs::write(p("gone.jsonl"), line("s", "2026-09-21T00:00:00Z", "g", 10)).unwrap();
    fs::write(p("live.jsonl"), line("t", "2026-09-21T00:00:00Z", "l", 20)).unwrap();
    let mut index = restarted(env);
    snap(&mut index, env);
    fs::remove_file(p("gone.jsonl")).unwrap();
    let s = snap(&mut index, env);
    assert_eq!(total(&s), 30);
    assert!(retired_path(env).is_file());
    s
}

#[test]
fn the_retired_store_is_written_before_the_shards_it_replaces_are_deleted() {
    let env = env();
    let p = |n: &str| env.claude.join("projects").join("p").join(n);
    fs::write(p("a.jsonl"), line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    fs::write(p("b.jsonl"), line("s", "2026-09-21T00:00:00Z", "b", 20)).unwrap();
    let mut index = restarted(&env);
    snap(&mut index, &env);
    let shard = shard_path(&env.index_dir, &p("a.jsonl"));
    assert!(shard.is_file());
    // The store cannot be written (a directory is in the way) ...
    fs::create_dir(retired_path(&env)).unwrap();
    fs::remove_file(p("a.jsonl")).unwrap();
    assert_eq!(
        total(&snap(&mut index, &env)),
        30,
        "still counted in memory"
    );
    assert!(
        shard.is_file(),
        "the shard stays until the store is on disk"
    );
    // ... until it can.
    fs::remove_dir(retired_path(&env)).unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 30);
    assert!(retired_path(&env).is_file());
    // The shard goes on a later refresh, after the store is read back.
    assert!(shard.is_file());
    assert_eq!(
        total(&super::harden_tests::settle_all(&mut index, &env)),
        30
    );
    assert!(!shard.exists());
    assert_eq!(total(&snap(&mut restarted(&env), &env)), 30);
}

#[test]
fn a_crash_between_the_store_write_and_the_shard_delete_does_not_double_count() {
    let env = env();
    let p = |n: &str| env.claude.join("projects").join("p").join(n);
    let c = env.codex.join("sessions").join("c.jsonl");
    fs::write(p("a.jsonl"), line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    fs::write(&c, format!("{HEADER}{}", count("2026-09-21T00:00:00Z", 7))).unwrap();
    let mut index = restarted(&env);
    snap(&mut index, &env);
    let saved: Vec<(PathBuf, Vec<u8>)> = [p("a.jsonl"), c.clone()]
        .iter()
        .map(|f| {
            let s = shard_path(&env.index_dir, f);
            let b = fs::read(&s).unwrap();
            (s, b)
        })
        .collect();
    fs::remove_file(p("a.jsonl")).unwrap();
    fs::remove_file(&c).unwrap();
    let before = snap(&mut index, &env);
    assert_eq!(total(&before), 17);
    for (shard, bytes) in &saved {
        fs::write(shard, bytes).unwrap();
    }
    let mut again = restarted(&env);
    let got = snap(&mut again, &env);
    assert_eq!(figures(&got), figures(&before));
    super::harden_tests::settle_all(&mut again, &env);
    assert_eq!(figures(&snap(&mut again, &env)), figures(&before));
    assert!(saved.iter().all(|(s, _)| !s.exists()));
}

#[test]
fn orphan_shards_are_kept_aside_or_left_never_deleted() {
    let env = env();
    let live = env.claude.join("projects").join("p").join("live.jsonl");
    fs::write(&live, line("t", "2026-09-21T00:00:00Z", "l", 20)).unwrap();
    let gone = env.claude.join("projects").join("p").join("gone.jsonl");
    fs::write(&gone, line("s", "2026-09-21T00:00:00Z", "g", 10)).unwrap();
    let mut index = restarted(&env);
    snap(&mut index, &env);
    let shard = shard_path(&env.index_dir, &gone);
    let good = fs::read(&shard).unwrap();
    fs::remove_file(&gone).unwrap();

    // A future version's shard of a deleted file is left exactly as it is.
    let mut value: serde_json::Value = serde_json::from_slice(&good).unwrap();
    value["version"] = (INDEX_VERSION + 1).into();
    let future = serde_json::to_vec(&value).unwrap();
    fs::write(&shard, &future).unwrap();
    assert_eq!(total(&snap(&mut restarted(&env), &env)), 20);
    assert_eq!(fs::read(&shard).unwrap(), future);

    // A corrupt one is moved aside (bytes intact), not deleted.
    fs::write(&shard, b"{\"version\":1,\"events\":[").unwrap();
    assert_eq!(total(&snap(&mut restarted(&env), &env)), 20);
    assert!(!shard.exists());
    let kept: Vec<_> = fs::read_dir(&env.index_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".json.broken-"))
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        fs::read(kept[0].path()).unwrap(),
        b"{\"version\":1,\"events\":["
    );
}

#[test]
fn a_retired_store_survives_a_bump_of_the_live_shard_version() {
    let env = env();
    let paths = seed(&env);
    let mut index = restarted(&env);
    let before = snap(&mut index, &env);
    fs::remove_file(&paths[0]).unwrap();
    fs::remove_file(&paths[2]).unwrap();
    assert_eq!(figures(&snap(&mut index, &env)), figures(&before));
    // Playing INDEX_VERSION + 1: every live shard is from "another version".
    for path in [&paths[1], &paths[3], &paths[4]] {
        let shard = shard_path(&env.index_dir, path);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&shard).unwrap()).unwrap();
        value["version"] = (INDEX_VERSION + 1).into();
        fs::write(&shard, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let mut restarted = restarted(&env);
    let got = snap(&mut restarted, &env);
    assert_eq!(figures(&got), figures(&before));
    assert!(restarted.last_read_bytes() > 0, "live files were re-parsed");
}

fn sample_summary(version: u32) -> FileSummary {
    FileSummary {
        version,
        path: "/x/a.jsonl".into(),
        provider: "claude".into(),
        len: 5,
        scanned: 5,
        offset: 5,
        models: vec!["m".into()],
        projects: vec!["p".into()],
        events: vec![super::index::StoredEvent {
            k: 1,
            at: 100,
            t: 9,
            ..Default::default()
        }],
        ..FileSummary::default()
    }
}

#[test]
fn shard_decoding_migrates_older_versions_and_refuses_to_guess() {
    let old = serde_json::to_vec(&sample_summary(1)).unwrap();
    // The app at version 2 with a migration from 1.
    let upgrade = |from: u32, mut value: serde_json::Value| {
        assert_eq!(from, 1);
        value["version"] = 2.into();
        serde_json::from_value::<FileSummary>(value).ok()
    };
    match decode_shard(&old, 2, &upgrade) {
        Decoded::Summary(s) => assert_eq!((s.version, s.events.len(), s.events[0].t), (2, 1, 9)),
        other => panic!("{other:?}"),
    }
    // No migration: not usable, but reported, never mistaken for current.
    assert!(matches!(
        decode_shard(&old, 2, &|_, _| None),
        Decoded::Bad(_)
    ));
    // A migration that forgets to set the version is refused.
    assert!(matches!(
        decode_shard(&old, 2, &|_, v| serde_json::from_value(v).ok()),
        Decoded::Bad(_)
    ));
    // Current, newer, garbage, inconsistent.
    let current = serde_json::to_vec(&sample_summary(2)).unwrap();
    assert!(matches!(
        decode_shard(&current, 2, &|_, _| None),
        Decoded::Summary(_)
    ));
    let newer = serde_json::to_vec(&sample_summary(3)).unwrap();
    assert!(matches!(
        decode_shard(&newer, 2, &|_, _| None),
        Decoded::Newer(3)
    ));
    assert!(matches!(
        decode_shard(b"nope", 2, &|_, _| None),
        Decoded::Bad(_)
    ));
    let mut broken = sample_summary(2);
    broken.events[0].m = 5;
    let broken = serde_json::to_vec(&broken).unwrap();
    assert!(matches!(
        decode_shard(&broken, 2, &|_, _| None),
        Decoded::Bad(_)
    ));
}

#[test]
fn retired_store_decoding_migrates_older_formats_and_refuses_to_guess() {
    let env = env();
    with_one_retired(&env);
    let bytes = fs::read(retired_path(&env)).unwrap();
    let keys = |r: &Retired| r.key_count();
    // Current format round-trips.
    let back = decode_retired(&bytes, RETIRED_FORMAT, &|_, _| None).unwrap();
    assert_eq!(keys(&back), 1);
    assert_eq!(back.encode().unwrap(), bytes);
    // The app moves to format 2 and converts format 1.
    let migrated = decode_retired(&bytes, RETIRED_FORMAT + 1, &|from, value| {
        assert_eq!(from, RETIRED_FORMAT);
        decode_retired(
            &serde_json::to_vec(&value).unwrap(),
            RETIRED_FORMAT,
            &|_, _| None,
        )
        .ok()
    })
    .unwrap();
    assert_eq!(keys(&migrated), 1);
    // Without a migration the store is refused (the index then leaves it untouched and goes read-only).
    assert!(matches!(
        decode_retired(&bytes, RETIRED_FORMAT + 1, &|_, _| None),
        Err(StoreError::Corrupt(_))
    ));
    // A store from a newer yMux is refused as such.
    assert_eq!(
        decode_retired(&bytes, RETIRED_FORMAT - 1, &|_, _| None).err(),
        Some(StoreError::Newer(RETIRED_FORMAT))
    );
    assert!(decode_retired(b"{}", RETIRED_FORMAT, &|_, _| None).is_err());
}

#[test]
fn resetting_is_explicit_and_keeps_the_old_store_aside() {
    let env = env();
    with_one_retired(&env);
    let mut index = restarted(&env);
    assert_eq!(total(&snap(&mut index, &env)), 30);
    index.reset_retired().unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 20);
    assert!(!retired_path(&env).exists());
    assert_eq!(aside(&env, "reset").len(), 1);
}

// ---- cost ------------------------------------------------------------------

fn retired_after(events: u64) -> (usize, usize) {
    let env = env();
    let p = env.claude.join("projects").join("p").join("a.jsonl");
    // All in one hour, one project, one model: one aggregate however many.
    let text: String = (0..events)
        .map(|i| line("s", "2026-06-01T00:00:00Z", &format!("m{i}"), 1))
        .collect();
    fs::write(&p, text).unwrap();
    let mut index = restarted(&env);
    snap(&mut index, &env);
    fs::remove_file(&p).unwrap();
    let after = snap(&mut index, &env);
    assert_eq!(total(&after), events);
    (index.retired().work_units(), index.retired().key_count())
}

#[test]
fn what_each_tick_reads_of_retired_history_does_not_grow_with_its_events() {
    let (small_work, small_keys) = retired_after(100);
    let (large_work, large_keys) = retired_after(5000);
    assert_eq!(small_work, large_work);
    assert_eq!((small_keys, large_keys), (100, 5000));
    // Per-event data is gone from memory: only the aggregates remain.
    let env = env();
    let p = env.claude.join("projects").join("p").join("a.jsonl");
    let text: String = (0..50)
        .map(|i| line("s", "2026-06-01T00:00:00Z", &format!("m{i}"), 1))
        .collect();
    fs::write(&p, text).unwrap();
    let mut index = restarted(&env);
    snap(&mut index, &env);
    fs::remove_file(&p).unwrap();
    snap(&mut index, &env);
    assert_eq!(index.live_totals(), (0, 0));
    assert!(index.retired().recent().is_empty());
    assert_eq!(index.retired().rollup().len(), 1);
}
