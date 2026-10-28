use super::{collect_from, collect_from_roots, scan_roots};
use crate::accounts::AgentDirs;
use std::fs;
use std::path::{Path, PathBuf};

const NOW: u64 = 1_790_000_000;

fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    let codex = dir.path().join("codex");
    fs::create_dir_all(claude.join("projects/p/subagents")).unwrap();
    fs::create_dir_all(codex.join("sessions")).unwrap();
    fs::create_dir_all(codex.join("archived_sessions")).unwrap();
    (dir, claude, codex)
}

#[test]
fn claude_stream_updates_and_copies_count_once() {
    let (_dir, claude, codex) = setup();
    let lines = concat!(
        "{\"type\":\"assistant\",\"sessionId\":\"s\",\"cwd\":\"/project\",\"timestamp\":\"2026-09-21T00:00:00Z\",\"message\":{\"id\":\"m\",\"model\":\"claude\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1,\"cache_read_input_tokens\":20}}}\n",
        "{\"type\":\"assistant\",\"sessionId\":\"s\",\"cwd\":\"/project\",\"timestamp\":\"2026-09-21T00:00:00Z\",\"message\":{\"id\":\"m\",\"model\":\"claude\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"cache_read_input_tokens\":20}}}\n"
    );
    fs::write(claude.join("projects/p/a.jsonl"), lines).unwrap();
    fs::write(claude.join("projects/p/subagents/copy.jsonl"), lines).unwrap();
    let snapshot = collect_from(&claude, &codex, NOW);
    assert_eq!(snapshot.providers[0].all_time.total, 35);
    assert_eq!(snapshot.rows[0].project, "/project");
}

#[test]
fn codex_repeated_cumulative_usage_and_archived_copy_count_once() {
    let (_dir, claude, codex) = setup();
    let header = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session\",\"cwd\":\"/codex\"}}\n{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt\"}}\n";
    let count = |input, output| {
        format!("{{\"timestamp\":\"2026-09-21T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{input},\"output_tokens\":{output},\"cached_input_tokens\":5,\"total_tokens\":{}}},\"last_token_usage\":{{\"input_tokens\":999}}}}}}}}\n", input + output)
    };
    let lines = format!("{header}{}{}{}", count(20, 5), count(20, 5), count(30, 10));
    fs::write(codex.join("sessions/a.jsonl"), &lines).unwrap();
    fs::write(
        codex.join("archived_sessions/b.jsonl"),
        format!("{header}{}", count(30, 10)),
    )
    .unwrap();
    let snapshot = collect_from(&claude, &codex, NOW);
    let total = snapshot.providers[1].all_time;
    assert_eq!(
        (total.total, total.input, total.output, total.cache_read),
        (40, 25, 10, 5)
    );
}

#[test]
fn refresh_sees_appended_records_and_reports_corrupt_lines() {
    let (_dir, claude, codex) = setup();
    let path = claude.join("projects/p/a.jsonl");
    fs::write(&path, "broken\n").unwrap();
    assert_eq!(collect_from(&claude, &codex, NOW).providers[0].skipped, 1);
    fs::write(path, "{\"type\":\"assistant\",\"timestamp\":\"2026-09-21T00:00:00Z\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":9}}}\n").unwrap();
    assert_eq!(
        collect_from(&claude, &codex, NOW).providers[0]
            .all_time
            .total,
        9
    );
}

#[test]
fn rolling_windows_include_boundary_and_exclude_future() {
    let (_dir, claude, codex) = setup();
    let now = crate::agent_scan_disk::parse_rfc3339_secs("2026-09-21T05:00:00Z").unwrap();
    let records = [
        ("2026-09-21T00:00:00Z", "five", 10),
        ("2026-09-14T05:00:00Z", "week", 20),
        ("2026-09-14T04:59:59Z", "old", 30),
        ("2026-09-21T05:00:01Z", "future", 99),
    ].into_iter().map(|(time,id,input)| format!("{{\"type\":\"assistant\",\"timestamp\":\"{time}\",\"message\":{{\"id\":\"{id}\",\"usage\":{{\"input_tokens\":{input}}}}}}}\n")).collect::<String>();
    fs::write(claude.join("projects/p/time.jsonl"), records).unwrap();
    let snapshot = collect_from(&claude, &codex, now);
    let p = &snapshot.providers[0];
    assert_eq!(
        (p.five_hour.total, p.seven_day.total, p.all_time.total),
        (10, 30, 60)
    );
}

#[test]
fn quota_selection_ignores_future_and_out_of_order_records() {
    let (_dir, claude, codex) = setup();
    let records = [("2026-09-21T04:00:00Z", 40), ("2026-09-21T03:00:00Z", 30), ("2026-09-22T03:00:00Z", 99)]
        .into_iter().map(|(time,pct)| format!("{{\"timestamp\":\"{time}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"rate_limits\":{{\"primary\":{{\"used_percent\":{pct},\"window_minutes\":300}}}}}}}}\n")).collect::<String>();
    fs::write(codex.join("sessions/quota.jsonl"), records).unwrap();
    let now = crate::agent_scan_disk::parse_rfc3339_secs("2026-09-21T05:00:00Z").unwrap();
    let snapshot = collect_from(&claude, &codex, now);
    assert_eq!(snapshot.providers[1].quotas[0].used_percent, 40.0);
}

#[test]
fn oversized_transcripts_report_partial_without_scanning() {
    let (_dir, claude, codex) = setup();
    let file = fs::File::create(claude.join("projects/p/large.jsonl")).unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    let snapshot = collect_from(&claude, &codex, NOW);
    assert!(snapshot.providers[0].warnings.contains(&"partial"));
    assert!(snapshot.warnings.is_empty());
    assert_eq!(snapshot.providers[0].files, 0);
}

#[test]
fn file_reader_rejects_growth_beyond_charged_metadata() {
    let (_dir, claude, _codex) = setup();
    let path = claude.join("projects/p/growing.jsonl");
    fs::write(&path, b"{}\n{}\n").unwrap();
    assert!(super::disk::parse_file(&path, "claude", "session", 3).is_err());
    assert!(super::disk::parse_file(&path, "claude", "session", 6).is_ok());
}

#[test]
fn oversized_retained_metadata_is_not_copied_into_events() {
    let mut parsed = super::parse::Parsed::new("session");
    let metadata = serde_json::json!({"cwd": "x".repeat(4097)});
    parsed.line(&serde_json::to_vec(&metadata).unwrap(), "claude");
    parsed.line(br#"{"type":"assistant","timestamp":"2026-09-21T00:00:00Z","message":{"id":"m","usage":{"input_tokens":1}}}"#, "claude");
    assert_eq!(parsed.skipped, 1);
    assert_eq!(parsed.events[0].project, "unknown");
}

#[test]
fn out_of_order_codex_checkpoints_preserve_window_deltas() {
    let (_dir, claude, codex) = setup();
    let records = [("2026-09-21T04:00:00Z", 30), ("2026-09-20T23:59:59Z", 25)]
        .into_iter()
        .map(|(time, count)| {
            let record = serde_json::json!({ "timestamp": time, "type": "event_msg",
                "payload": { "type": "token_count", "info": { "total_token_usage": {
                    "input_tokens": count, "total_tokens": count } } } });
            format!("{record}\n")
        })
        .collect::<String>();
    fs::write(codex.join("sessions/checkpoints.jsonl"), records).unwrap();
    let now = crate::agent_scan_disk::parse_rfc3339_secs("2026-09-21T05:00:00Z").unwrap();
    let snapshot = collect_from(&claude, &codex, now);
    assert_eq!(snapshot.providers[1].five_hour.total, 5);
    assert_eq!(snapshot.providers[1].all_time.total, 30);
}

#[test]
fn projects_merge_windows_spellings_and_preserve_posix_case() {
    let (_dir, claude, codex) = setup();
    let records = ["D:/Git/App", "d:\\git\\app", "/srv/App", "/srv/app"]
        .into_iter()
        .enumerate()
        .map(|(index, project)| {
            let record = serde_json::json!({"type":"assistant", "timestamp":"2026-09-21T00:00:00Z",
                "cwd":project, "message":{"id":index.to_string(), "usage":{"input_tokens":10}}});
            format!("{record}\n")
        })
        .collect::<String>();
    fs::write(claude.join("projects/p/paths.jsonl"), records).unwrap();
    let snapshot = collect_from(&claude, &codex, NOW);
    assert_eq!(snapshot.rows.len(), 3);
    assert_eq!(
        snapshot
            .rows
            .iter()
            .find(|r| r.project.starts_with("D:"))
            .unwrap()
            .all_time
            .total,
        20
    );
}

#[test]
fn hourly_buckets_honor_dedupe_codex_delta_window_and_order() {
    let (_dir, claude, codex) = setup();
    let now = crate::agent_scan_disk::parse_rfc3339_secs("2026-09-21T05:30:00Z").unwrap();
    let claude_line = |time: &str, id: &str, input: u64, output: u64| {
        format!("{{\"type\":\"assistant\",\"timestamp\":\"{time}\",\"message\":{{\"id\":\"{id}\",\"usage\":{{\"input_tokens\":{input},\"output_tokens\":{output}}}}}}}\n")
    };
    // `a` is streamed twice (the larger record wins); `old` is outside 371 days.
    let records = [
        claude_line("2026-09-21T05:10:00Z", "a", 10, 1),
        claude_line("2026-09-21T05:10:00Z", "a", 10, 5),
        claude_line("2026-09-21T03:59:59Z", "b", 7, 0),
        claude_line("2025-09-15T05:30:00Z", "edge", 3, 0),
        claude_line("2025-09-15T05:29:59Z", "old", 100, 0),
    ]
    .concat();
    fs::write(claude.join("projects/p/h.jsonl"), records).unwrap();
    let header = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s\",\"cwd\":\"/c\"}}\n";
    let count = |time: &str, total: u64| {
        format!("{{\"timestamp\":\"{time}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{total},\"total_tokens\":{total}}}}}}}}}\n")
    };
    let lines = format!(
        "{header}{}{}{}",
        count("2026-09-21T04:00:00Z", 20),
        count("2026-09-21T04:30:00Z", 20),
        count("2026-09-21T05:00:00Z", 50)
    );
    fs::write(codex.join("sessions/h.jsonl"), lines).unwrap();
    let snapshot = collect_from(&claude, &codex, now);
    let hour = |s: &str| crate::agent_scan_disk::parse_rfc3339_secs(s).unwrap() / 3600;
    let got: Vec<_> = snapshot
        .hourly
        .iter()
        .map(|b| (b.hour, b.provider, b.total))
        .collect();
    assert_eq!(
        got,
        vec![
            (hour("2025-09-15T05:30:00Z"), "claude", 3),
            (hour("2026-09-21T03:00:00Z"), "claude", 7),
            (hour("2026-09-21T04:00:00Z"), "codex", 20),
            (hour("2026-09-21T05:00:00Z"), "claude", 15),
            (hour("2026-09-21T05:00:00Z"), "codex", 30),
        ]
    );
}

fn claude_line(time: &str, id: &str, input: u64) -> String {
    format!("{{\"type\":\"assistant\",\"sessionId\":\"s\",\"timestamp\":\"{time}\",\"message\":{{\"id\":\"{id}\",\"usage\":{{\"input_tokens\":{input}}}}}}}\n")
}

fn dirs_of(claude: &Path, codex: &Path) -> AgentDirs {
    AgentDirs {
        claude: claude.display().to_string(),
        codex: codex.display().to_string(),
        claude_env: None,
    }
}

/// Default account plus a "work" profile with its own directories.
fn two_accounts() -> (tempfile::TempDir, Vec<(String, AgentDirs)>, PathBuf) {
    let (dir, claude, codex) = setup();
    let wclaude = dir.path().join("wclaude");
    let wcodex = dir.path().join("wcodex");
    fs::create_dir_all(wclaude.join("projects/p")).unwrap();
    fs::create_dir_all(wcodex.join("sessions")).unwrap();
    let dirs = vec![
        (String::new(), dirs_of(&claude, &codex)),
        ("work".to_string(), dirs_of(&wclaude, &wcodex)),
    ];
    (dir, dirs, wclaude)
}

#[test]
fn two_accounts_get_own_cards_rows_hours() {
    let (_dir, dirs, wclaude) = two_accounts();
    let default_claude = PathBuf::from(&dirs[0].1.claude);
    fs::write(
        default_claude.join("projects/p/a.jsonl"),
        claude_line("2026-09-21T00:00:00Z", "a", 10),
    )
    .unwrap();
    fs::write(
        wclaude.join("projects/p/b.jsonl"),
        claude_line("2026-09-21T00:30:00Z", "b", 7),
    )
    .unwrap();
    let snapshot = collect_from_roots(&scan_roots(&dirs), NOW);
    let cards: Vec<_> = snapshot
        .providers
        .iter()
        .map(|p| (p.account.as_str(), p.id, p.all_time.total))
        .collect();
    assert_eq!(
        cards,
        vec![
            ("", "claude", 10),
            ("work", "claude", 7),
            ("", "codex", 0),
            ("work", "codex", 0)
        ]
    );
    let rows: Vec<_> = snapshot
        .rows
        .iter()
        .map(|r| (r.account.as_str(), r.all_time.total))
        .collect();
    assert_eq!(rows, vec![("", 10), ("work", 7)]);
    // Same UTC hour, two accounts: two buckets that still sum to the total.
    let hours: Vec<_> = snapshot
        .hourly
        .iter()
        .map(|b| (b.account.as_str(), b.total))
        .collect();
    assert_eq!(hours, vec![("", 10), ("work", 7)]);
}

#[test]
fn shared_dir_scanned_once_under_first_owner() {
    let (dir, claude, codex) = setup();
    let wcodex = dir.path().join("wcodex");
    fs::create_dir_all(wcodex.join("sessions")).unwrap();
    // "work" shares the default's Claude dir but has its own Codex home.
    let dirs = vec![
        (String::new(), dirs_of(&claude, &codex)),
        ("work".to_string(), dirs_of(&claude, &wcodex)),
    ];
    fs::write(
        claude.join("projects/p/a.jsonl"),
        claude_line("2026-09-21T00:00:00Z", "a", 10),
    )
    .unwrap();
    let roots = scan_roots(&dirs);
    let claude_roots: Vec<_> = roots.iter().filter(|r| r.provider == "claude").collect();
    assert_eq!(claude_roots.len(), 1);
    assert_eq!(claude_roots[0].owner, "");
    assert_eq!(claude_roots[0].also, vec!["work".to_string()]);
    let snapshot = collect_from_roots(&roots, NOW);
    let cards: Vec<_> = snapshot
        .providers
        .iter()
        .filter(|p| p.id == "claude")
        .collect();
    assert_eq!(cards.len(), 1);
    assert_eq!((cards[0].files, cards[0].all_time.total), (1, 10));
    assert_eq!(cards[0].also, vec!["work".to_string()]);
    assert_eq!(
        snapshot
            .providers
            .iter()
            .filter(|p| p.id == "codex")
            .count(),
        2
    );
}

#[test]
fn transcript_copied_into_profile_counts_once_for_default() {
    let (_dir, dirs, wclaude) = two_accounts();
    let default_claude = PathBuf::from(&dirs[0].1.claude);
    let lines = claude_line("2026-09-21T00:00:00Z", "a", 10);
    fs::write(default_claude.join("projects/p/a.jsonl"), &lines).unwrap();
    fs::write(wclaude.join("projects/p/a.jsonl"), &lines).unwrap();
    let snapshot = collect_from_roots(&scan_roots(&dirs), NOW);
    let totals: Vec<_> = snapshot
        .providers
        .iter()
        .filter(|p| p.id == "claude")
        .map(|p| (p.account.as_str(), p.all_time.total))
        .collect();
    assert_eq!(totals, vec![("", 10), ("work", 0)]);
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].account, "");
}

#[test]
fn card_warnings_do_not_leak_across_accounts() {
    let (_dir, dirs, wclaude) = two_accounts();
    let file = fs::File::create(wclaude.join("projects/p/large.jsonl")).unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    let snapshot = collect_from_roots(&scan_roots(&dirs), NOW);
    let warnings: Vec<_> = snapshot
        .providers
        .iter()
        .map(|p| (p.account.as_str(), p.id, p.warnings.clone()))
        .collect();
    assert_eq!(
        warnings,
        vec![
            ("", "claude", vec![]),
            ("work", "claude", vec!["partial"]),
            ("", "codex", vec![]),
            ("work", "codex", vec![]),
        ]
    );
    assert!(snapshot.warnings.is_empty());
}

#[test]
#[ignore = "reads this machine's real Claude/Codex history"]
fn print_real_hourly_bucket_count() {
    let snapshot = super::collect(&[]);
    let days: std::collections::BTreeSet<_> = snapshot.hourly.iter().map(|b| b.hour / 24).collect();
    let json = serde_json::to_string(&snapshot.hourly).unwrap();
    println!(
        "hourly buckets: {}, active UTC days: {}, json bytes: {}",
        snapshot.hourly.len(),
        days.len(),
        json.len()
    );
}

#[test]
fn a_file_over_the_event_budget_is_skipped_the_same_way_every_refresh() {
    let (_dir, claude, codex) = setup();
    let line = |i: usize| {
        format!("{{\"type\":\"assistant\",\"sessionId\":\"s\",\"timestamp\":\"2026-09-21T00:00:00Z\",\"message\":{{\"id\":\"m{i}\",\"usage\":{{\"input_tokens\":1}}}}}}\n")
    };
    // A file holds at most 10k events; the provider's budget is 25k, so the
    // third full file does not fit after the first two and the small one.
    for f in 0..3 {
        let big: String = (f * 10_000..(f + 1) * 10_000).map(line).collect();
        fs::write(claude.join(format!("projects/p/big{f}.jsonl")), big).unwrap();
    }
    fs::write(claude.join("projects/p/small.jsonl"), line(99_999)).unwrap();
    for _ in 0..2 {
        let p = &collect_from(&claude, &codex, NOW).providers[0];
        assert_eq!((p.files, p.skipped, p.all_time.total), (3, 1, 20_001));
    }
}
