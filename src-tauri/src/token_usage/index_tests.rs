//! Behaviour of the persistent per-file index: every figure must equal a
//! from-scratch parse of the same files, however the index got there.
use super::collect::collect_with;
use super::disk::{self, Budget};
use super::index::{shard_path, FileSummary, Index, INDEX_VERSION};
use super::{scan_roots, ScanRoot, TokenSnapshot};
use crate::accounts::AgentDirs;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(super) const NOW: u64 = 1_790_000_000;

pub(super) struct Env {
    pub _dir: tempfile::TempDir,
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub index_dir: PathBuf,
}

pub(super) fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    let codex = dir.path().join("codex");
    let index_dir = dir.path().join("index");
    fs::create_dir_all(claude.join("projects/p")).unwrap();
    fs::create_dir_all(codex.join("sessions")).unwrap();
    Env {
        _dir: dir,
        claude,
        codex,
        index_dir,
    }
}

pub(super) fn roots(env: &Env) -> Vec<ScanRoot> {
    scan_roots(&[(
        String::new(),
        AgentDirs {
            claude: env.claude.display().to_string(),
            codex: env.codex.display().to_string(),
            claude_env: None,
        },
    )])
}

/// An index that sweeps and persists on every call, so tests are exact.
pub(super) fn fresh(dir: Option<&Path>) -> Index {
    let mut index = Index::new(dir.map(Path::to_path_buf));
    index.sweep_every = Duration::ZERO;
    index.persist_every = Duration::ZERO;
    index.retired_persist_every = Duration::ZERO;
    index
}

pub(super) fn snap(index: &mut Index, env: &Env) -> TokenSnapshot {
    collect_with(index, &roots(env), NOW, Budget::unlimited())
}

/// Everything the frontend reads, as one comparable value.
pub(super) fn view(s: &TokenSnapshot) -> String {
    serde_json::to_string(&(&s.providers, &s.rows, &s.hourly, &s.warnings)).unwrap()
}

fn scratch(env: &Env) -> TokenSnapshot {
    snap(&mut fresh(None), env)
}

pub(super) fn line(session: &str, time: &str, id: &str, input: u64) -> String {
    format!("{{\"type\":\"assistant\",\"sessionId\":\"{session}\",\"cwd\":\"/proj\",\"timestamp\":\"{time}\",\"message\":{{\"id\":\"{id}\",\"model\":\"m\",\"usage\":{{\"input_tokens\":{input}}}}}}}\n")
}

pub(super) fn append(path: &Path, text: &str) {
    let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

pub(super) fn total(s: &TokenSnapshot) -> u64 {
    s.providers.iter().map(|p| p.all_time.total).sum()
}

#[test]
fn appended_file_equals_from_scratch_and_is_read_incrementally() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(&path, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    let mut index = fresh(None);
    assert_eq!(total(&snap(&mut index, &env)), 10);
    let first_len = fs::metadata(&path).unwrap().len();
    append(&path, &line("s", "2026-09-21T01:00:00Z", "b", 5));
    let got = snap(&mut index, &env);
    assert_eq!(view(&got), view(&scratch(&env)));
    assert_eq!(total(&got), 15);
    // The second pass started where the first stopped.
    assert_eq!(
        index.last_read_bytes(),
        fs::metadata(&path).unwrap().len() - first_len
    );
}

#[test]
fn appended_update_of_a_streamed_message_keeps_the_larger_record() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(&path, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    let mut index = fresh(None);
    snap(&mut index, &env);
    append(&path, &line("s", "2026-09-21T00:00:00Z", "a", 30));
    let got = snap(&mut index, &env);
    assert_eq!(total(&got), 30);
    assert_eq!(view(&got), view(&scratch(&env)));
}

#[test]
fn rewritten_and_shrunk_files_are_parsed_from_scratch() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("a.jsonl");
    let long = [
        line("s", "2026-09-21T00:00:00Z", "a", 10),
        line("s", "2026-09-21T01:00:00Z", "b", 20),
        line("s", "2026-09-21T02:00:00Z", "c", 30),
    ]
    .concat();
    fs::write(&path, long).unwrap();
    let mut index = fresh(None);
    assert_eq!(total(&snap(&mut index, &env)), 60);
    // Shrunk.
    fs::write(&path, line("s", "2026-09-21T03:00:00Z", "d", 7)).unwrap();
    let got = snap(&mut index, &env);
    assert_eq!(total(&got), 7);
    assert_eq!(view(&got), view(&scratch(&env)));
    // Rewritten LARGER with different leading bytes: not an append.
    fs::write(
        &path,
        [
            line("x", "2026-09-21T04:00:00Z", "e", 1),
            line("x", "2026-09-21T05:00:00Z", "f", 2),
            line("x", "2026-09-21T06:00:00Z", "g", 3),
        ]
        .concat(),
    )
    .unwrap();
    let got = snap(&mut index, &env);
    assert_eq!(total(&got), 6);
    assert_eq!(view(&got), view(&scratch(&env)));
}

#[test]
fn same_length_rewrite_with_new_mtime_is_reparsed() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(&path, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    let mut index = fresh(None);
    snap(&mut index, &env);
    fs::write(&path, line("s", "2026-09-21T00:00:00Z", "a", 99)).unwrap();
    let bumped = fs::metadata(&path).unwrap().modified().unwrap() + Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(bumped)
        .unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 99);
}

#[test]
fn deleted_files_keep_their_history_and_their_shard_is_replaced_by_the_retired_store() {
    let env = env();
    let a = env.claude.join("projects").join("p").join("a.jsonl");
    let b = env.claude.join("projects").join("p").join("b.jsonl");
    fs::write(&a, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    fs::write(&b, line("t", "2026-09-21T00:00:00Z", "b", 20)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 30);
    assert!(shard_path(&env.index_dir, &a).is_file());
    fs::remove_file(&a).unwrap();
    let got = snap(&mut index, &env);
    assert_eq!(total(&got), 30, "history survives the deletion");
    assert_eq!(got.providers[0].files, 1);
    assert!(env.index_dir.join("retired.json").is_file());
    // The shard goes a refresh later, once the store is read back and found
    // to hold it (two-phase retirement).
    assert!(shard_path(&env.index_dir, &a).is_file());
    super::harden_tests::settle_all(&mut index, &env);
    assert!(!shard_path(&env.index_dir, &a).exists());
    assert!(shard_path(&env.index_dir, &b).is_file());
}

#[test]
fn a_file_larger_than_the_old_64mb_limit_is_streamed_and_counted() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("huge.jsonl");
    // ~1 MB of padding per line keeps the line count (and test time) small.
    let pad = "x".repeat(1024 * 1024);
    let mut f = fs::File::create(&path).unwrap();
    let mut expected = 0;
    for i in 0..70u64 {
        let text = format!("{{\"type\":\"user\",\"cwd\":\"/proj\",\"pad\":\"{pad}\"}}\n");
        f.write_all(text.as_bytes()).unwrap();
        f.write_all(line("s", "2026-09-21T00:00:00Z", &format!("m{i}"), i + 1).as_bytes())
            .unwrap();
        expected += i + 1;
    }
    drop(f);
    assert!(fs::metadata(&path).unwrap().len() > 64 * 1024 * 1024);
    let got = snap(&mut fresh(None), &env);
    assert_eq!(total(&got), expected);
    assert_eq!(got.providers[0].skipped, 0);
    assert!(!got.indexing);
}

#[test]
fn more_than_ten_thousand_events_in_one_file_are_all_counted() {
    let env = env();
    let text: String = (0..12_000)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("m{i}"), 1))
        .collect();
    fs::write(
        env.claude.join("projects").join("p").join("many.jsonl"),
        text,
    )
    .unwrap();
    let got = snap(&mut fresh(None), &env);
    assert_eq!(total(&got), 12_000);
    assert_eq!(got.providers[0].skipped, 0);
}

#[test]
fn growth_during_the_read_is_ignored_then_picked_up_next_time() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(&path, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    let stale = disk::Stat::of(&fs::metadata(&path).unwrap());
    // The file grows after the metadata call, before the read.
    append(&path, &line("s", "2026-09-21T01:00:00Z", "b", 5));
    let mut index = fresh(None);
    let mut budget = Budget::unlimited();
    assert!(index.update(&path, "claude", stale, &mut budget));
    let summary: &FileSummary = index.summary(&path).unwrap();
    assert_eq!(summary.scanned, stale.len);
    assert_eq!(summary.events.len(), 1);
    // The next ordinary refresh sees the growth and reads only the new part.
    let got = snap(&mut index, &env);
    assert_eq!(total(&got), 15);
    assert_eq!(view(&got), view(&scratch(&env)));
}

#[test]
fn a_partial_trailing_line_is_left_for_next_time() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("a.jsonl");
    let second = line("s", "2026-09-21T01:00:00Z", "b", 5);
    let (head, tail) = second.split_at(second.len() / 2);
    fs::write(
        &path,
        format!("{}{head}", line("s", "2026-09-21T00:00:00Z", "a", 10)),
    )
    .unwrap();
    let mut index = fresh(None);
    let got = snap(&mut index, &env);
    assert_eq!((total(&got), got.providers[0].skipped), (10, 0));
    assert!(!got.indexing, "a half-written line is not pending work");
    append(&path, tail);
    let got = snap(&mut index, &env);
    assert_eq!((total(&got), got.providers[0].skipped), (15, 0));
    assert_eq!(view(&got), view(&scratch(&env)));
}

#[test]
fn an_unterminated_but_complete_last_line_still_counts() {
    let env = env();
    let text = line("s", "2026-09-21T00:00:00Z", "a", 10);
    fs::write(
        env.claude.join("projects").join("p").join("a.jsonl"),
        text.trim_end_matches('\n'),
    )
    .unwrap();
    assert_eq!(total(&snap(&mut fresh(None), &env)), 10);
}

#[test]
fn unchanged_files_are_not_read_again_and_the_index_survives_a_restart() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(&path, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    snap(&mut index, &env);
    snap(&mut index, &env);
    assert_eq!(index.last_read_bytes(), 0);
    // A new process loads the shard instead of parsing: poison it to prove it.
    let shard = shard_path(&env.index_dir, &path);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&shard).unwrap()).unwrap();
    value["events"][0]["i"] = 777.into();
    value["events"][0]["t"] = 777.into();
    fs::write(&shard, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut restarted = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut restarted, &env)), 777);
    assert_eq!(restarted.last_read_bytes(), 0);
}

#[test]
fn a_corrupt_index_file_is_discarded_and_rebuilt() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(&path, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    snap(&mut index, &env);
    let shard = shard_path(&env.index_dir, &path);
    for garbage in [
        b"not json at all".to_vec(),
        Vec::new(),
        fs::read(&shard).unwrap()[..20].to_vec(),
        // Valid JSON, wrong shape.
        b"{\"version\":1}".to_vec(),
    ] {
        fs::write(&shard, garbage).unwrap();
        let mut restarted = fresh(Some(&env.index_dir));
        let got = snap(&mut restarted, &env);
        assert_eq!(total(&got), 10);
        assert_eq!(view(&got), view(&scratch(&env)));
    }
    // Self-consistent JSON whose event points outside its string tables.
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&shard).unwrap()).unwrap();
    value["events"][0]["m"] = 99.into();
    fs::write(&shard, serde_json::to_vec(&value).unwrap()).unwrap();
    let got = snap(&mut fresh(Some(&env.index_dir)), &env);
    assert_eq!(total(&got), 10);
}

#[test]
fn an_index_from_another_version_is_discarded_and_rebuilt() {
    let env = env();
    let path = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(&path, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    snap(&mut index, &env);
    let shard = shard_path(&env.index_dir, &path);
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&shard).unwrap()).unwrap();
    value["events"][0]["i"] = 777.into();
    value["events"][0]["t"] = 777.into();
    value["version"] = (INDEX_VERSION + 1).into();
    fs::write(&shard, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut restarted = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut restarted, &env)), 10);
    // And the rebuilt shard is current again.
    let rebuilt: serde_json::Value = serde_json::from_slice(&fs::read(&shard).unwrap()).unwrap();
    assert_eq!(rebuilt["version"], INDEX_VERSION);
}

#[test]
fn budgeted_indexing_converges_newest_first_and_reports_progress() {
    let env = env();
    let mut sizes = 0;
    for f in 0..6u64 {
        let text: String = (0..20)
            .map(|i| {
                line(
                    &format!("s{f}"),
                    "2026-09-21T00:00:00Z",
                    &format!("f{f}m{i}"),
                    1,
                )
            })
            .collect();
        let path = env.claude.join(format!("projects/p/f{f}.jsonl"));
        fs::write(&path, &text).unwrap();
        // Newer files have higher numbers.
        let when = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + f * 100);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        sizes += text.len() as u64;
    }
    let mut index = fresh(Some(&env.index_dir));
    let mut ticks = 0;
    let mut last = 0;
    let mut first_total = None;
    loop {
        ticks += 1;
        let got = collect_with(&mut index, &roots(&env), NOW, Budget::bytes(1500));
        assert_eq!(got.total_bytes, sizes);
        assert!(got.indexed_bytes >= last, "progress never goes backwards");
        last = got.indexed_bytes;
        first_total.get_or_insert(total(&got));
        if !got.indexing {
            assert_eq!(got.indexed_bytes, got.total_bytes);
            assert_eq!(view(&got), view(&scratch(&env)));
            break;
        }
        assert!(ticks < 50, "indexing must converge");
    }
    assert!(ticks > 2, "the budget must actually spread the work");
    // The first tick already held the newest files' data.
    assert!(first_total.unwrap() > 0 && first_total.unwrap() < 120);
}

#[test]
fn a_single_huge_file_is_indexed_across_ticks_from_its_saved_offset() {
    let env = env();
    let text: String = (0..200)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("m{i}"), 2))
        .collect();
    fs::write(
        env.claude.join("projects").join("p").join("big.jsonl"),
        &text,
    )
    .unwrap();
    let mut index = fresh(None);
    let mut partials = 0;
    loop {
        let got = collect_with(&mut index, &roots(&env), NOW, Budget::bytes(4000));
        if !got.indexing {
            assert_eq!(total(&got), 400);
            break;
        }
        partials += 1;
        assert!(total(&got) <= 400);
        assert!(partials < 100);
    }
    assert!(partials >= 2);
}

#[test]
fn a_claude_message_copied_into_another_session_file_counts_once() {
    let env = env();
    // A resumed/forked session copies earlier messages under a new sessionId.
    fs::write(
        env.claude.join("projects").join("p").join("old.jsonl"),
        line("old", "2026-09-21T00:00:00Z", "msg_1", 10),
    )
    .unwrap();
    fs::write(
        env.claude.join("projects").join("p").join("fork.jsonl"),
        [
            line("fork", "2026-09-21T00:00:00Z", "msg_1", 10),
            line("fork", "2026-09-21T01:00:00Z", "msg_2", 4),
        ]
        .concat(),
    )
    .unwrap();
    let got = snap(&mut fresh(None), &env);
    assert_eq!(total(&got), 14);
    assert_eq!(got.hourly.iter().map(|b| b.total).sum::<u64>(), 14);
    assert_eq!(got.rows.iter().map(|r| r.all_time.total).sum::<u64>(), 14);
}

#[test]
fn codex_session_split_across_files_counts_deltas_once() {
    let env = env();
    let header = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"sess\",\"cwd\":\"/c\"}}\n";
    let count = |time: &str, total: u64| {
        format!("{{\"timestamp\":\"{time}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{total},\"total_tokens\":{total}}}}}}}}}\n")
    };
    let path = env.codex.join("sessions/a.jsonl");
    fs::write(
        &path,
        format!("{header}{}", count("2026-09-21T00:00:00Z", 10)),
    )
    .unwrap();
    let mut index = fresh(None);
    assert_eq!(total(&snap(&mut index, &env)), 10);
    // Appended checkpoints continue from the saved session context.
    append(&path, &count("2026-09-21T01:00:00Z", 25));
    // An archived copy of the same session is not counted again.
    fs::write(
        env.codex.join("sessions/b.jsonl"),
        format!("{header}{}", count("2026-09-21T01:00:00Z", 25)),
    )
    .unwrap();
    let got = snap(&mut index, &env);
    assert_eq!(total(&got), 25);
    assert_eq!(view(&got), view(&scratch(&env)));
}

#[test]
fn quotas_survive_in_the_index() {
    let env = env();
    let rec = |time: &str, pct: u32| {
        format!("{{\"timestamp\":\"{time}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"rate_limits\":{{\"primary\":{{\"used_percent\":{pct},\"window_minutes\":300}}}}}}}}\n")
    };
    let path = env.codex.join("sessions/q.jsonl");
    fs::write(&path, rec("2026-09-21T03:00:00Z", 30)).unwrap();
    let now = crate::agent_scan_disk::parse_rfc3339_secs("2026-09-21T05:00:00Z").unwrap();
    let mut index = fresh(Some(&env.index_dir));
    collect_with(&mut index, &roots(&env), now, Budget::unlimited());
    append(&path, &rec("2026-09-21T04:00:00Z", 40));
    let mut restarted = fresh(Some(&env.index_dir));
    let got = collect_with(&mut restarted, &roots(&env), now, Budget::unlimited());
    assert_eq!(got.providers[1].quotas[0].used_percent, 40.0);
}

#[test]
fn between_full_sweeps_only_hot_files_are_restatted() {
    let env = env();
    let a = env.claude.join("projects").join("p").join("a.jsonl");
    fs::write(&a, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    let mut index = fresh(None);
    snap(&mut index, &env);
    index.sweep_every = Duration::from_secs(3600);
    // A new file is not noticed until the next sweep ...
    fs::write(
        env.claude.join("projects").join("p").join("new.jsonl"),
        line("t", "2026-09-21T00:00:00Z", "n", 5),
    )
    .unwrap();
    // ... but an append to a recently written file is.
    append(&a, &line("s", "2026-09-21T01:00:00Z", "a2", 1));
    assert_eq!(total(&snap(&mut index, &env)), 11);
    index.sweep_every = Duration::ZERO;
    assert_eq!(total(&snap(&mut index, &env)), 16);
    // A hot file that vanishes between sweeps is retired too: still counted.
    index.sweep_every = Duration::from_secs(3600);
    fs::remove_file(&a).unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 16);
    assert_eq!(index.retired().key_count(), 2);
}

#[test]
#[ignore = "reads this machine's real Claude/Codex history (read-only); prints timings"]
fn measure_real_history_index() {
    use std::time::Instant;
    let home = dirs::home_dir().unwrap();
    let dirs = vec![(
        String::new(),
        AgentDirs {
            claude: home.join(".claude").display().to_string(),
            codex: home.join(".codex").display().to_string(),
            claude_env: None,
        },
    )];
    let roots = scan_roots(&dirs);
    let tmp = tempfile::tempdir().unwrap();
    let mut index = Index::new(Some(tmp.path().to_path_buf()));
    let started = Instant::now();
    let mut ticks = 0;
    loop {
        ticks += 1;
        let t = Instant::now();
        let s = collect_with(
            &mut index,
            &roots,
            super::collect::now_for_tests(),
            Budget::new(256 * 1024 * 1024, Some(Duration::from_millis(1500))),
        );
        println!(
            "tick {ticks}: {:?} indexed {}/{} MB all_time {}",
            t.elapsed(),
            s.indexed_bytes >> 20,
            s.total_bytes >> 20,
            s.providers.iter().map(|p| p.all_time.total).sum::<u64>()
        );
        if !s.indexing {
            break;
        }
    }
    println!("first full index: {:?} in {ticks} ticks", started.elapsed());
    let shards: u64 = fs::read_dir(tmp.path())
        .unwrap()
        .flatten()
        .map(|e| e.metadata().map_or(0, |m| m.len()))
        .sum();
    println!("shard bytes on disk: {} MB", shards >> 20);
    for label in ["steady (sweep each tick)", "steady (hot set only)"] {
        index.sweep_every = if label.contains("sweep") {
            Duration::ZERO
        } else {
            Duration::from_secs(3600)
        };
        let t = Instant::now();
        for _ in 0..5 {
            collect_with(
                &mut index,
                &roots,
                super::collect::now_for_tests(),
                Budget::new(256 * 1024 * 1024, Some(Duration::from_millis(1500))),
            );
        }
        println!("{label}: {:?} per tick", t.elapsed() / 5);
    }
    let t = Instant::now();
    let mut restarted = Index::new(Some(tmp.path().to_path_buf()));
    let s = collect_with(
        &mut restarted,
        &roots,
        super::collect::now_for_tests(),
        Budget::new(256 * 1024 * 1024, Some(Duration::from_millis(1500))),
    );
    println!(
        "restart from shards: {:?}, indexing={}, read {} bytes",
        t.elapsed(),
        s.indexing,
        restarted.last_read_bytes()
    );
}
