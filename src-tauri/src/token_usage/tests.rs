use super::collect_from;
use std::fs;

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
    assert!(snapshot.warnings.iter().any(|w| w == "claude_partial"));
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
