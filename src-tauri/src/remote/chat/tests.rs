//! Synthetic transcripts only: no real conversation text is used here.

use super::*;
use crate::agent_scan_disk::claude_project_dir_name;
use crate::remote::proto::Op;
use tempfile::TempDir;

const SID: &str = "0a1b2c3d-0000-4000-8000-000000000001";
const T0: &str = "2026-11-12T13:43:44.123Z";

// The reader's entry points take a picture switch these tests mostly leave off;
// these shadow them with the picture-less form (the picture tests below call
// `super::` ones, spelled out).
fn line_items(agent: AgentKind, line: &[u8]) -> Vec<Item> {
    super::line_items(agent, line, false)
}

fn answer(
    b: &Binding<'_>,
    root: &Path,
    pane_cwd: &str,
    conv: &str,
    pos: Option<ChatPos>,
    cached: &mut Option<PathBuf>,
) -> Option<Value> {
    super::answer(b, root, pane_cwd, conv, pos, false, true, cached, &mut 0)
}

fn read_opened(
    agent: AgentKind,
    session_id: &str,
    opened: Opened,
    conv: &str,
    pos: Option<ChatPos>,
) -> Result<Value, Fail> {
    super::read_opened(
        Rd {
            agent,
            images: false,
            big: true,
        },
        session_id,
        opened,
        conv,
        pos,
        &mut 0,
    )
}

fn user(text: &str) -> String {
    json!({"type":"user","timestamp":T0,"cwd":"/w","isSidechain":false,
           "message":{"role":"user","content":text}})
    .to_string()
}

fn assistant(blocks: Value) -> String {
    json!({"type":"assistant","timestamp":T0,"cwd":"/w","isSidechain":false,
           "message":{"role":"assistant","content":blocks}})
    .to_string()
}

fn junk(bytes: usize) -> String {
    json!({"type":"attachment","blob":"x".repeat(bytes)}).to_string()
}

fn write(dir: &TempDir, name: &str, lines: &[String]) -> PathBuf {
    let p = dir.path().join(name);
    let mut s = lines.join("\n");
    s.push('\n');
    std::fs::write(&p, s).unwrap();
    p
}

fn append(path: &Path, s: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(s.as_bytes()).unwrap();
}

fn rd(path: &Path, conv: &str, pos: Option<ChatPos>) -> Value {
    read(AgentKind::Claude, SID, path, conv, pos)
}

fn texts(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["text"].as_str().unwrap().to_string())
        .collect()
}

fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap()
}

fn msgs(range: std::ops::Range<usize>) -> Vec<String> {
    range.map(|i| user(&format!("m{i}"))).collect()
}

fn names(range: std::ops::Range<usize>) -> Vec<String> {
    range.map(|i| format!("m{i}")).collect()
}

/// Items of one line, as (role, text).
fn items(agent: AgentKind, line: &str) -> Vec<(&'static str, String)> {
    line_items(agent, line.as_bytes())
        .into_iter()
        .map(|i| (i.role, i.text))
        .collect()
}

// ------------------------------------------------------------ claude shapes

#[test]
fn claude_user_lines() {
    let c = AgentKind::Claude;
    assert_eq!(items(c, &user("hello")), vec![("user", "hello".into())]);
    // Text blocks are joined by a blank line; images and the like are not text.
    let l = json!({"type":"user","message":{"content":[
        {"type":"text","text":"one"},{"type":"image","source":{}},{"type":"text","text":"two"}]}});
    assert_eq!(
        items(c, &l.to_string()),
        vec![("user", "one\n\ntwo".into())]
    );
    // A tool result line is the harness, even with text riding along.
    let l = json!({"type":"user","message":{"content":[
        {"type":"tool_result","tool_use_id":"t","content":"output"},
        {"type":"text","text":"injected"}]}});
    assert!(items(c, &l.to_string()).is_empty());
    for (k, v) in [
        ("isMeta", true),
        ("isSidechain", true),
        ("isCompactSummary", true),
    ] {
        let mut l = serde_json::from_str::<Value>(&user("hidden")).unwrap();
        l[k] = json!(v);
        assert!(items(c, &l.to_string()).is_empty(), "{k}");
    }
    // `false` flags do not hide anything.
    let mut l = serde_json::from_str::<Value>(&user("shown")).unwrap();
    l["isMeta"] = json!(false);
    assert_eq!(items(c, &l.to_string()).len(), 1);
}

#[test]
fn claude_wrappers_are_dropped_per_block() {
    let c = AgentKind::Claude;
    for w in [
        "<command-name>/clear</command-name>",
        "<command-message>x</command-message>",
        "<local-command-stdout>ok</local-command-stdout>",
        "<system-reminder>be good</system-reminder>",
        "  <task-notification>done</task-notification>",
    ] {
        assert!(items(c, &user(w)).is_empty(), "{w}");
    }
    let l = json!({"type":"user","message":{"content":[
        {"type":"text","text":"<system-reminder>x</system-reminder>"},
        {"type":"text","text":"the real question"}]}});
    assert_eq!(
        items(c, &l.to_string()),
        vec![("user", "the real question".into())]
    );
    // Other angle-bracket text is a message.
    assert_eq!(items(c, &user("<task>do it</task>")).len(), 1);
    assert!(items(c, &user("   \n ")).is_empty());
}

#[test]
fn claude_shell_mode_shows_the_command_never_its_output() {
    let c = AgentKind::Claude;
    assert_eq!(
        items(c, &user("<bash-input>cargo test -p x</bash-input>")),
        vec![("tool", "Bash: cargo test -p x".into())]
    );
    for out in [
        "<bash-stdout>secret output</bash-stdout><bash-stderr></bash-stderr>",
        "<bash-stdout></bash-stdout><bash-stderr>boom</bash-stderr>",
        "<bash-stderr>boom</bash-stderr>",
    ] {
        assert!(items(c, &user(out)).is_empty(), "{out}");
    }
    // As blocks, next to a real message.
    let l = json!({"type":"user","message":{"content":[
        {"type":"text","text":"<bash-input>ls\n-la</bash-input>"},
        {"type":"text","text":"and a question"}]}});
    assert_eq!(
        items(c, &l.to_string()),
        vec![
            ("user", "and a question".into()),
            ("tool", "Bash: ls -la".into())
        ]
    );
    // A missing closing tag never turns the output into the command.
    for (rec, want) in [
        (
            "<bash-input>pwd<bash-stdout>SECRET</bash-stdout>",
            "Bash: pwd",
        ),
        ("<bash-input>pwd<bash-stderr>SECRET", "Bash: pwd"),
        ("<bash-input>pwd\nSECRET OUTPUT", "Bash: pwd"),
        ("<bash-input>  ls -la  \nSECRET", "Bash: ls -la"),
    ] {
        let got = items(c, &user(rec));
        assert_eq!(got, vec![("tool", want.into())], "{rec:?}");
        assert!(!got[0].1.contains("SECRET"));
    }
    // An unterminated tag is still only a command, and an empty one is nothing.
    assert_eq!(
        items(c, &user("<bash-input>pwd")),
        vec![("tool", "Bash: pwd".into())]
    );
    assert_eq!(
        items(c, &user("<bash-input></bash-input>")),
        vec![("tool", "Bash".into())]
    );
}

#[test]
fn codex_user_shell_commands_show_the_command_never_the_result() {
    let c = AgentKind::Codex;
    let rec = "<user_shell_command>\n<command>\ncargo build\n</command>\n<result>\nExit code: 0\nWall time: 1 seconds\nOutput:\nTOPSECRETOUTPUT\n</result>\n</user_shell_command>";
    let got = items(c, &rollout_msg("user", "input_text", rec));
    assert_eq!(got, vec![("tool", "shell: cargo build".into())]);
    assert!(!got[0].1.contains("TOPSECRET"));
    // No command inside: nothing but the bare tool name, still no output.
    let got = items(
        c,
        &rollout_msg(
            "user",
            "input_text",
            "<user_shell_command><result>X</result>",
        ),
    );
    assert_eq!(got, vec![("tool", "shell".into())]);
    // No closing `</command>`: the output is not the command either.
    for rec in [
        "<user_shell_command>\n<command>\nmake<result>\nSECRET",
        "<user_shell_command>\n<command>\nmake\nSECRET output",
        "<user_shell_command><command>make</user_shell_command>SECRET",
    ] {
        let got = items(c, &rollout_msg("user", "input_text", rec));
        assert_eq!(got, vec![("tool", "shell: make".into())], "{rec:?}");
    }
    // Only a user message can carry one.
    assert!(items(c, &rollout_msg("assistant", "output_text", rec)).is_empty());
}

#[test]
fn claude_assistant_text_tools_and_thinking() {
    let c = AgentKind::Claude;
    let l = assistant(json!([
        {"type":"thinking","thinking":"private","signature":"s"},
        {"type":"text","text":"first"},
        {"type":"text","text":"second"},
        {"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test","description":"run"}},
        {"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/w/a.rs"}},
        {"type":"text","text":"after"},
        {"type":"tool_result","content":"never"},
    ]));
    assert_eq!(
        items(c, &l),
        vec![
            ("assistant", "first\n\nsecond".into()),
            ("tool", "Bash: cargo test".into()),
            ("tool", "Read: /w/a.rs".into()),
            ("assistant", "after".into()),
        ]
    );
    assert!(!items(c, &l).iter().any(|(_, t)| t.contains("private")));
    // A thinking-only line is nothing.
    assert!(items(c, &assistant(json!([{"type":"thinking","thinking":"x"}]))).is_empty());
    // Sub-agent conversations are never included.
    let mut side =
        serde_json::from_str::<Value>(&assistant(json!([{"type":"text","text":"x"}]))).unwrap();
    side["isSidechain"] = json!(true);
    assert!(items(c, &side.to_string()).is_empty());
}

#[test]
fn tool_summaries_pick_the_main_input() {
    let s = |name: &str, input: Value| {
        items(
            AgentKind::Claude,
            &assistant(json!([{"type":"tool_use","name":name,"input":input}])),
        )
        .remove(0)
        .1
    };
    assert_eq!(
        s("Grep", json!({"pattern":"foo.*","path":"/w"})),
        "Grep: /w"
    );
    assert_eq!(s("Grep", json!({"pattern":"foo.*"})), "Grep: foo.*");
    assert_eq!(
        s("Task", json!({"description":"scan","prompt":"long"})),
        "Task: scan"
    );
    assert_eq!(
        s("WebFetch", json!({"url":"https://e.example/x"})),
        "WebFetch: https://e.example/x"
    );
    assert_eq!(s("WebSearch", json!({"query":"rust"})), "WebSearch: rust");
    assert_eq!(s("Mystery", json!({"other":1})), "Mystery");
    assert_eq!(s("Bash", json!({"command":"a\nb"})), "Bash: a b");
    let long = s("Bash", json!({"command":"x ".repeat(500)}));
    assert_eq!(long.chars().count(), 200);
    assert!(long.ends_with('…'));
    // Secrets in a command are redacted.
    let t = s(
        "Bash",
        json!({"command":"curl -H 'Authorization: Bearer ghp_abcdefghijklmnopqrstuvwxyz0123456789'"}),
    );
    assert!(!t.contains("abcdefghijklmnop"), "{t}");
}

#[test]
fn timestamps_become_milliseconds_or_zero() {
    let l = user("x");
    let it = line_items(AgentKind::Claude, l.as_bytes());
    // 2026-11-12T13:43:44.123Z
    assert_eq!(it[0].ts_ms, 1_794_491_024_123);
    assert_eq!(rfc3339_ms("2026-11-12T13:43:44Z"), Some(1_794_491_024_000));
    assert_eq!(
        rfc3339_ms("2026-11-12T13:43:44.5Z"),
        Some(1_794_491_024_500)
    );
    assert_eq!(
        rfc3339_ms("2026-11-12T13:43:44.123456+00:00"),
        Some(1_794_491_024_123)
    );
    assert_eq!(rfc3339_ms("yesterday"), None);
    let no_ts = json!({"type":"user","message":{"content":"x"}}).to_string();
    assert_eq!(line_items(AgentKind::Claude, no_ts.as_bytes())[0].ts_ms, 0);
}

// -------------------------------------------------------------- codex shapes

fn rollout_msg(role: &str, kind: &str, text: &str) -> String {
    json!({"type":"response_item","timestamp":T0,
           "payload":{"type":"message","role":role,"content":[{"type":kind,"text":text}]}})
    .to_string()
}

#[test]
fn codex_lines() {
    let c = AgentKind::Codex;
    assert_eq!(
        items(c, &rollout_msg("user", "input_text", "do it")),
        vec![("user", "do it".into())]
    );
    assert_eq!(
        items(c, &rollout_msg("assistant", "output_text", "done")),
        vec![("assistant", "done".into())]
    );
    // Injected context and developer text are not the user's.
    for w in [
        "<environment_context>\n<cwd>/w</cwd>",
        "<user_instructions>x",
        "# AGENTS.md instructions for /w",
    ] {
        assert!(
            items(c, &rollout_msg("user", "input_text", w)).is_empty(),
            "{w}"
        );
    }
    assert!(items(c, &rollout_msg("developer", "input_text", "rules")).is_empty());
    // A `<task>` prompt is a prompt.
    assert_eq!(
        items(c, &rollout_msg("user", "input_text", "<task>\nx")).len(),
        1
    );
    let l = json!({"type":"response_item","payload":{"type":"message","role":"user","content":[
        {"type":"input_text","text":"<environment_context>x"},
        {"type":"input_text","text":"real"}]}});
    assert_eq!(items(c, &l.to_string()), vec![("user", "real".into())]);

    let call = |p: Value| json!({"type":"response_item","payload":p}).to_string();
    assert_eq!(
        items(
            c,
            &call(json!({"type":"function_call","name":"shell",
            "arguments":"{\"command\":[\"bash\",\"-lc\",\"cargo test\"]}","call_id":"c"}))
        ),
        vec![("tool", "shell: cargo test".into())]
    );
    assert_eq!(
        items(
            c,
            &call(json!({"type":"function_call","name":"read_file",
            "arguments":"{\"path\":\"/w/a\"}"}))
        ),
        vec![("tool", "read_file: /w/a".into())]
    );
    assert_eq!(
        items(
            c,
            &call(
                json!({"type":"local_shell_call","action":{"type":"exec","command":["ls","-la"]}})
            )
        ),
        vec![("tool", "shell: ls -la".into())]
    );
    assert_eq!(
        items(
            c,
            &call(json!({"type":"custom_tool_call","name":"apply_patch",
            "input":"*** Begin Patch\n*** Add File: a\n"}))
        ),
        vec![("tool", "apply_patch: *** Begin Patch".into())]
    );
    // Outputs, reasoning and event messages are never shown.
    for p in [
        json!({"type":"function_call_output","output":"secret output"}),
        json!({"type":"custom_tool_call_output","output":"x"}),
        json!({"type":"reasoning","summary":[{"text":"private"}]}),
    ] {
        assert!(items(c, &call(p)).is_empty());
    }
    let ev = json!({"type":"event_msg","payload":{"type":"agent_message","message":"dup"}});
    assert!(items(c, &ev.to_string()).is_empty());
    // A Claude line is nothing to the Codex reader and the other way round.
    assert!(items(c, &user("x")).is_empty());
    assert!(items(AgentKind::Claude, &rollout_msg("user", "input_text", "x")).is_empty());
}

// ----------------------------------------------------------------- cleaning

#[test]
fn texts_keep_newlines_drop_controls_redact_and_clip() {
    let key = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let text = format!("a\r\nb\u{1b}[31m\tc\u{200b}\nkey {key}\n```\ncode\n```\n| x | y |\n한국어");
    let got = items(AgentKind::Claude, &user(&text));
    let t = &got[0].1;
    assert!(t.starts_with("a\nb[31m\tc\nkey "), "{t:?}");
    assert!(!t.contains(key) && !t.contains('\u{1b}') && !t.contains('\r'));
    assert!(t.ends_with("```\n| x | y |\n한국어"));
    let long = "word ".repeat(4000);
    let got = items(AgentKind::Claude, &user(&long));
    assert_eq!(got[0].1.chars().count(), 8000);
    assert!(got[0].1.ends_with('…'));
}

// ------------------------------------------------------------------ paging

#[test]
fn newest_page_then_paging_back_reaches_the_start_without_gaps() {
    let d = TempDir::new().unwrap();
    let p = write(&d, "a.jsonl", &msgs(0..100));
    let first = rd(&p, "", None);
    assert_eq!(texts(&first), names(60..100));
    assert_eq!(
        (first["older"].clone(), first["newer"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(n(&first, "end"), std::fs::metadata(&p).unwrap().len());
    assert!(clip::reply_data(Op::Chat, &first).is_some());
    let conv = first["conv"].as_str().unwrap().to_string();
    assert_eq!(conv.len(), 16);
    assert_eq!(first["agent"], "claude");
    let id0 = first["items"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(id0, format!("{}.0", n(&first, "start")));

    let mut all = texts(&first);
    let mut cur = first;
    let mut rounds = 0;
    while cur["older"] == true {
        rounds += 1;
        assert!(rounds < 10);
        let r = rd(&p, &conv, Some(ChatPos::Before(n(&cur, "start"))));
        assert_eq!(r["conv"], conv.as_str());
        assert_eq!(n(&r, "end"), n(&cur, "start"));
        assert_eq!(r["newer"], true);
        let mut t = texts(&r);
        t.append(&mut all);
        all = t;
        cur = r;
    }
    assert_eq!(all, names(0..100));
    assert_eq!(n(&cur, "start"), 0);
    // Ids are unique and stable: the same line has the same id in any reply.
    let again = rd(&p, &conv, Some(ChatPos::Before(n(&cur, "end"))));
    assert_eq!(again["items"][0]["id"], cur["items"][0]["id"]);
}

#[test]
fn following_with_after_as_the_file_grows() {
    let d = TempDir::new().unwrap();
    let p = write(&d, "a.jsonl", &msgs(0..5));
    let first = rd(&p, "", None);
    let conv = first["conv"].as_str().unwrap().to_string();
    assert_eq!(first["older"], false);
    let mut e = n(&first, "end");

    // Nothing new: no items, the position stays, nothing is newer.
    let r = rd(&p, &conv, Some(ChatPos::After(e)));
    assert!(texts(&r).is_empty());
    assert_eq!((n(&r, "end"), r["newer"].clone()), (e, json!(false)));

    append(&p, &(msgs(5..8).join("\n") + "\n"));
    let r = rd(&p, &conv, Some(ChatPos::After(e)));
    assert_eq!(texts(&r), names(5..8));
    assert!(n(&r, "end") > e);
    assert_eq!(r["newer"], false);
    e = n(&r, "end");
    assert_eq!(e, std::fs::metadata(&p).unwrap().len());

    // Many more than one reply holds: each reply moves on, `newer` says so.
    append(&p, &(msgs(8..108).join("\n") + "\n"));
    let mut got = Vec::new();
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds < 10);
        let r = rd(&p, &conv, Some(ChatPos::After(e)));
        assert!(n(&r, "end") > e, "a reply makes progress");
        got.extend(texts(&r));
        e = n(&r, "end");
        if r["newer"] == false {
            break;
        }
        assert_eq!(texts(&r).len(), 40);
    }
    assert_eq!(got, names(8..108));
}

#[test]
fn a_partial_last_line_is_not_read_until_it_is_complete() {
    let d = TempDir::new().unwrap();
    let p = write(&d, "a.jsonl", &msgs(0..2));
    let full_len = std::fs::metadata(&p).unwrap().len();
    let last = user("m2");
    append(&p, &last[..last.len() - 4]);
    let r = rd(&p, "", None);
    assert_eq!(texts(&r), names(0..2));
    assert_eq!(n(&r, "end"), full_len);
    let conv = r["conv"].as_str().unwrap().to_string();
    let after = rd(&p, &conv, Some(ChatPos::After(full_len)));
    assert!(texts(&after).is_empty());
    assert_eq!(
        (n(&after, "end"), after["newer"].clone()),
        (full_len, json!(false))
    );
    // The line is finished.
    append(&p, &format!("{}\n", &last[last.len() - 4..]));
    let after = rd(&p, &conv, Some(ChatPos::After(full_len)));
    assert_eq!(texts(&after), names(2..3));
    // A file that is only a partial line has no items.
    let q = d.path().join("b.jsonl");
    std::fs::write(&q, &last[..10]).unwrap();
    let r = rd(&q, "", None);
    assert!(texts(&r).is_empty());
    assert_eq!(
        (n(&r, "start"), n(&r, "end"), r["older"].clone()),
        (0, 0, json!(false))
    );
}

#[test]
fn a_stale_conv_or_a_bad_position_answers_the_newest_page() {
    let d = TempDir::new().unwrap();
    let p = write(&d, "a.jsonl", &msgs(0..60));
    let first = rd(&p, "", None);
    let conv = first["conv"].as_str().unwrap().to_string();
    let newest = texts(&first);
    let len = std::fs::metadata(&p).unwrap().len();
    let start = n(&first, "start");
    // Another conversation's name.
    for pos in [ChatPos::After(0), ChatPos::Before(start)] {
        let r = rd(&p, "othername", Some(pos));
        assert_eq!(texts(&r), newest);
        assert_eq!(r["conv"], conv.as_str());
    }
    // Beyond the file: a newest page, with the page's own positions.
    for pos in [
        ChatPos::After(len + 1),
        ChatPos::After(len + 1_000_000),
        ChatPos::After(CHAT_POS_MAX),
        ChatPos::Before(len + 1),
    ] {
        let r = rd(&p, &conv, Some(pos));
        assert_eq!(texts(&r), newest, "{pos:?}");
        assert_eq!((n(&r, "start"), n(&r, "end")), (start, len), "{pos:?}");
    }
    // The ends are fine: after the last line is "nothing new", before 0 is empty.
    let r = rd(&p, &conv, Some(ChatPos::After(len)));
    assert!(texts(&r).is_empty());
    let r = rd(&p, &conv, Some(ChatPos::Before(0)));
    assert!(texts(&r).is_empty());
    assert_eq!(
        (r["older"].clone(), r["newer"].clone()),
        (json!(false), json!(true))
    );
    // A conv alone is the newest page.
    assert_eq!(texts(&rd(&p, &conv, None)), newest);
}

#[test]
fn text_total_is_bounded_to_64_kib_in_whole_lines() {
    let d = TempDir::new().unwrap();
    let body = "ab ".repeat(2666); // 7998 chars
    let lines: Vec<String> = (0..20).map(|i| user(&format!("{i:02}{body}"))).collect();
    let p = write(&d, "a.jsonl", &lines);
    let mut r = rd(&p, "", None);
    let conv = r["conv"].as_str().unwrap().to_string();
    let mut total = 0;
    let mut seen = 0;
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds < 10);
        let bytes: usize = texts(&r).iter().map(String::len).sum();
        assert!(bytes <= 64 * 1024, "{bytes}");
        assert_eq!(texts(&r).len(), 8, "63984 bytes fit, a ninth would not");
        assert!(clip::reply_data(Op::Chat, &r).is_some());
        total += bytes;
        seen += texts(&r).len();
        if r["older"] == false || seen >= 20 {
            break;
        }
        r = rd(&p, &conv, Some(ChatPos::Before(n(&r, "start"))));
        if texts(&r).len() < 8 {
            seen += texts(&r).len();
            break;
        }
    }
    assert_eq!(seen, 20);
    assert!(total > 0);
    // Forward the same way.
    let mut e = 0;
    let mut got = 0;
    for _ in 0..4 {
        let r = rd(&p, &conv, Some(ChatPos::After(e)));
        assert!(n(&r, "end") > e);
        assert!(texts(&r).len() <= 8);
        got += texts(&r).len();
        e = n(&r, "end");
        if r["newer"] == false {
            break;
        }
    }
    assert_eq!(got, 20);
}

#[test]
fn start_and_end_move_over_lines_without_items() {
    let d = TempDir::new().unwrap();
    let mut lines = vec![user("first")];
    lines.extend((0..300).map(|_| json!({"type":"attachment","n":1}).to_string()));
    lines.push(user("last"));
    let p = write(&d, "a.jsonl", &lines);
    let r = rd(&p, "", None);
    assert_eq!(texts(&r), vec!["first", "last"]);
    assert_eq!((n(&r, "start"), r["older"].clone()), (0, json!(false)));
    // Forward over the same file from the start.
    let conv = r["conv"].as_str().unwrap().to_string();
    let r = rd(&p, &conv, Some(ChatPos::After(0)));
    assert_eq!(texts(&r), vec!["first", "last"]);
    assert_eq!(r["newer"], false);
}

#[test]
fn an_oversize_line_is_skipped_unparsed() {
    let d = TempDir::new().unwrap();
    let giant = user(&"g".repeat(25 * 1024 * 1024));
    assert!(giant.len() as u64 > IMAGE_LINE_MAX);
    let p = write(&d, "a.jsonl", &[user("before"), giant, user("after")]);
    let r = rd(&p, "", None);
    assert_eq!(texts(&r), vec!["before", "after"]);
    assert_eq!(r["older"], false);
    let conv = r["conv"].as_str().unwrap().to_string();
    let r = rd(&p, &conv, Some(ChatPos::After(0)));
    assert_eq!(texts(&r), vec!["before", "after"]);
}

#[test]
fn the_read_budget_is_honoured_and_start_stays_truthful() {
    let d = TempDir::new().unwrap();
    // 1.5 MiB lines, each parsed (under the line cap) and each without items.
    let mut lines = vec![user("old")];
    lines.extend((0..6).map(|_| junk(1_500_000)));
    let p = write(&d, "a.jsonl", &lines);
    let r = rd(&p, "", None);
    assert!(texts(&r).is_empty());
    assert_eq!(r["older"], true);
    let conv = r["conv"].as_str().unwrap().to_string();
    let mut start = n(&r, "start");
    assert!(start > 0);
    // `start` is a line start, and paging back keeps moving until the file
    // start (the budget is per request, not per file).
    let mut found = Vec::new();
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds < 8, "stuck at {start}");
        let r = rd(&p, &conv, Some(ChatPos::Before(start)));
        assert!(n(&r, "start") < start, "progress");
        found.extend(texts(&r));
        start = n(&r, "start");
        if r["older"] == false {
            break;
        }
    }
    assert_eq!(found, vec!["old"]);
    assert_eq!(start, 0);
}

/// Follow `after` from `e` until nothing is newer; every call must move on.
fn follow(p: &Path, conv: &str, mut e: u64) -> (Vec<String>, usize) {
    let mut got = Vec::new();
    for calls in 1..40 {
        let r = rd(p, conv, Some(ChatPos::After(e)));
        assert_eq!(r["conv"], conv);
        // Honoured: the reply starts exactly where it was asked to.
        assert_eq!(n(&r, "start"), e);
        assert!(n(&r, "end") > e || (r["newer"] == false && n(&r, "end") == e));
        got.extend(texts(&r));
        e = n(&r, "end");
        if r["newer"] == false {
            return (got, calls);
        }
        assert!(n(&r, "end") > n(&r, "start"), "newer without progress");
    }
    panic!("never reached the end");
}

/// Page `before` from the end until the start; every call must move back.
fn walk_back(p: &Path, first: &Value) -> (Vec<String>, usize) {
    let conv = first["conv"].as_str().unwrap().to_string();
    let mut all = texts(first);
    let mut start = n(first, "start");
    let mut older = first["older"] == true;
    let mut calls = 0;
    while older {
        calls += 1;
        assert!(calls < 40, "never reached the start");
        let r = rd(p, &conv, Some(ChatPos::Before(start)));
        assert_eq!(n(&r, "end"), start, "honoured: the reply ends where asked");
        assert!(n(&r, "start") < start, "no progress at {start}");
        let mut t = texts(&r);
        t.append(&mut all);
        all = t;
        start = n(&r, "start");
        older = r["older"] == true;
    }
    (all, calls)
}

#[test]
fn a_line_beyond_the_budget_is_crossed_in_steps_going_forward() {
    let d = TempDir::new().unwrap();
    let p = write(&d, "a.jsonl", &[user("one")]);
    let first = rd(&p, "", None);
    let conv = first["conv"].as_str().unwrap().to_string();
    let e = n(&first, "end");
    // A 30 MiB excluded line (over what is ever parsed, and longer than the
    // 24 MiB + one page budget a request spends on a line), then a 1 MiB real
    // message, then a short one.
    let big_msg = user(&"word ".repeat(200_000));
    assert!(big_msg.len() > 900_000);
    append(
        &p,
        &format!(
            "{}\n{}\n{}\n",
            junk(30 * 1024 * 1024),
            big_msg,
            user("three")
        ),
    );
    let (got, calls) = follow(&p, &conv, e);
    assert!(calls > 1, "the 30 MiB line takes more than one call");
    assert_eq!(
        got.len(),
        2,
        "{:?}",
        got.iter().map(String::len).collect::<Vec<_>>()
    );
    assert!(got[0].starts_with("word word") && got[0].ends_with('…'));
    assert_eq!(got[1], "three");
}

#[test]
fn a_30_mib_line_is_crossed_going_forward_and_back() {
    let d = TempDir::new().unwrap();
    let p = write(
        &d,
        "a.jsonl",
        &[
            user("first"),
            junk(30 * 1024 * 1024),
            user("middle"),
            junk(30 * 1024 * 1024),
            user("last"),
        ],
    );
    let r0 = rd(&p, "", None);
    let conv = r0["conv"].as_str().unwrap().to_string();
    // Forward from the very start.
    let (got, calls) = follow(&p, &conv, 0);
    assert_eq!(got, vec!["first", "middle", "last"]);
    assert!(calls >= 4, "{calls}");
    // Backward from the end: the first reply holds only `last`.
    assert_eq!(texts(&r0), vec!["last"]);
    assert_eq!(r0["older"], true);
    let (all, calls) = walk_back(&p, &r0);
    assert_eq!(all, vec!["first", "middle", "last"]);
    assert!(calls >= 4, "{calls}");
}

/// Every record of `lines` (each a message) is reachable forward and back, every
/// call moves, and nothing answers "no conversation".
fn reach_all(p: &Path, want: &[&str]) {
    let r0 = rd(p, "", None);
    assert_ne!(r0["conv"], "", "newest page");
    let conv = r0["conv"].as_str().unwrap().to_string();
    let (got, _) = follow(p, &conv, 0);
    let first: Vec<&str> = got.iter().map(|g| &g[..g.len().min(4)]).collect();
    assert_eq!(first, want, "forward");
    let (all, _) = walk_back(p, &r0);
    let first: Vec<&str> = all.iter().map(|g| &g[..g.len().min(4)]).collect();
    assert_eq!(first, want, "backward");
}

fn write_with_tail(d: &TempDir, lines: &[String], tail: usize) -> PathBuf {
    let p = write(d, "a.jsonl", lines);
    // An unfinished last line: valid-looking start, no newline, `tail` bytes.
    let mut t = String::from("{\"type\":\"user\",\"message\":{\"content\":\"");
    t.push_str(&"t".repeat(tail.saturating_sub(t.len())));
    append(&p, &t);
    p
}

#[test]
fn an_unfinished_last_line_never_costs_the_page_anything() {
    let d = TempDir::new().unwrap();
    let big = user(&format!("BIG {}", "word ".repeat(120_000)));
    assert!(big.len() > 590_000 && (big.len() as u64) < MAX_LINE);
    let lines = vec![user("old1"), user("old2"), big];
    for tail in [
        3_800_000usize,
        4_150_000,
        4_300_000,
        20_000_000,
        MAX_READ as usize - 1,
        MAX_READ as usize,
        MAX_READ as usize + 1,
    ] {
        let p = write_with_tail(&d, &lines, tail);
        reach_all(&p, &["old1", "old2", "BIG "]);
        // The newest page itself holds all three (they fit one reply).
        let r0 = rd(&p, "", None);
        assert_eq!(texts(&r0).len(), 3, "{tail}");
        assert_eq!(r0["older"], false, "{tail}");
    }
}

#[test]
fn a_valid_record_is_never_split_by_what_follows_it() {
    let d = TempDir::new().unwrap();
    let rec = user(&format!("REC {}", "word ".repeat(300_000)));
    assert!((rec.len() as u64) < MAX_LINE);
    let p = write_with_tail(&d, &[user("old1"), rec], 3 * 1024 * 1024);
    reach_all(&p, &["old1", "REC "]);
    // Following from the start passes the record in the first call.
    let conv = rd(&p, "", None)["conv"].as_str().unwrap().to_string();
    let r = rd(&p, &conv, Some(ChatPos::After(0)));
    assert_eq!(texts(&r).len(), 2);
    assert_eq!(r["newer"], false);
}

#[test]
fn a_tail_beyond_the_scan_cap_is_an_error_not_no_conversation() {
    let d = TempDir::new().unwrap();
    let p = write_with_tail(&d, &[user("old1")], SCAN_MAX as usize + 5_000_000);
    let file = File::open(&p).unwrap();
    let md = file.metadata().unwrap();
    let opened = Opened {
        id: file_identity(&file),
        len: md.len(),
        path: p.clone(),
        file,
    };
    assert!(read_opened(AgentKind::Claude, SID, opened, "", None).is_err());
}

#[test]
fn a_position_inside_a_line_resumes_it_and_is_never_parsed() {
    let d = TempDir::new().unwrap();
    let a = user("alpha");
    let p = write(&d, "a.jsonl", &[a.clone(), user("beta"), user("gamma")]);
    let r0 = rd(&p, "", None);
    let conv = r0["conv"].as_str().unwrap().to_string();
    let len = std::fs::metadata(&p).unwrap().len();
    // Inside the first line: its rest is skipped, the next lines are read.
    let r = rd(&p, &conv, Some(ChatPos::After(3)));
    assert_eq!(texts(&r), vec!["beta", "gamma"]);
    assert_eq!((n(&r, "start"), n(&r, "end")), (3, len));
    // Backward from inside the second line: the head of that line is skipped.
    let second = a.len() as u64 + 1;
    let r = rd(&p, &conv, Some(ChatPos::Before(second + 5)));
    assert_eq!(texts(&r), vec!["alpha"]);
    assert_eq!((n(&r, "start"), n(&r, "end")), (0, second + 5));
}

#[test]
fn honoured_cursors_keep_their_guarantee_on_every_path() {
    let d = TempDir::new().unwrap();
    // Bound-limited by items (40), by text (64 KiB), by the read budget.
    let body = "ab ".repeat(2666);
    let mut lines: Vec<String> = msgs(0..100);
    lines.extend((0..12).map(|i| user(&format!("{i:02}{body}"))));
    lines.extend((0..4).map(|_| junk(1_500_000)));
    lines.extend(msgs(0..5));
    let p = write(&d, "a.jsonl", &lines);
    let r0 = rd(&p, "", None);
    let conv = r0["conv"].as_str().unwrap().to_string();
    let len = std::fs::metadata(&p).unwrap().len();
    // Newest pages are newest pages: end is the end of the complete lines.
    assert_eq!(n(&r0, "end"), len);
    let mut offsets: Vec<u64> = vec![0];
    let mut off = 0u64;
    for l in &lines {
        off += l.len() as u64 + 1;
        offsets.push(off);
    }
    for &o in offsets.iter().step_by(3) {
        let a = rd(&p, &conv, Some(ChatPos::After(o)));
        assert_eq!(n(&a, "start"), o, "after {o}");
        assert!(n(&a, "end") >= o);
        let b = rd(&p, &conv, Some(ChatPos::Before(o)));
        assert_eq!(n(&b, "end"), o, "before {o}");
        assert!(n(&b, "start") <= o);
        if o > 0 {
            assert!(n(&b, "start") < o || b["older"] == false, "before {o}");
        }
        if o < len {
            assert!(n(&a, "end") > o, "after {o} must move");
        }
    }
    // A position that is refused is a newest page with its own positions.
    for pos in [ChatPos::After(len + 7), ChatPos::Before(len + 7)] {
        let r = rd(&p, &conv, Some(pos));
        assert_eq!(n(&r, "end"), len);
    }
    let r = rd(&p, "otherconv", Some(ChatPos::After(0)));
    assert_eq!(n(&r, "end"), len);
}

#[test]
fn repeated_calls_never_skip_unexamined_data() {
    // After an excluded line comes a real record: no sequence of honoured
    // `after` calls may step over it, wherever the budget happens to cut.
    let d = TempDir::new().unwrap();
    for junk_len in [2_500_000usize, 3_700_000, 4_100_000, 6_000_000] {
        let p = write(
            &d,
            "a.jsonl",
            &[
                user("one"),
                junk(junk_len),
                user("two"),
                junk(junk_len),
                user("three"),
            ],
        );
        let conv = rd(&p, "", None)["conv"].as_str().unwrap().to_string();
        let (got, _) = follow(&p, &conv, 0);
        assert_eq!(got, vec!["one", "two", "three"], "{junk_len}");
        let (all, _) = walk_back(&p, &rd(&p, "", None));
        assert_eq!(all, vec!["one", "two", "three"], "{junk_len}");
    }
}

#[test]
fn crlf_files_and_blank_lines() {
    let d = TempDir::new().unwrap();
    let p = d.path().join("a.jsonl");
    let s = format!("{}\r\n\r\n{}\r\n", user("one"), user("two"));
    std::fs::write(&p, s).unwrap();
    let r = rd(&p, "", None);
    assert_eq!(texts(&r), vec!["one", "two"]);
    let conv = r["conv"].as_str().unwrap().to_string();
    assert_eq!(
        texts(&rd(&p, &conv, Some(ChatPos::After(0)))),
        vec!["one", "two"]
    );
}

#[test]
fn codex_files_page_like_claude_files() {
    let d = TempDir::new().unwrap();
    let mut lines =
        vec![json!({"type":"session_meta","payload":{"id":SID,"cwd":"/w"}}).to_string()];
    for i in 0..50 {
        lines.push(rollout_msg("user", "input_text", &format!("q{i}")));
        lines.push(rollout_msg("assistant", "output_text", &format!("a{i}")));
    }
    let p = write(&d, "rollout.jsonl", &lines);
    let r = read(AgentKind::Codex, SID, &p, "", None);
    assert_eq!(r["agent"], "codex");
    assert_eq!(texts(&r).len(), 40);
    assert_eq!(texts(&r)[39], "a49");
    let conv = r["conv"].as_str().unwrap().to_string();
    let b = read(
        AgentKind::Codex,
        SID,
        &p,
        &conv,
        Some(ChatPos::Before(n(&r, "start"))),
    );
    assert_eq!(texts(&b).len(), 40);
    assert_eq!(texts(&b)[0], "q10");
    // The same file is another conversation for another agent kind.
    let other = read(AgentKind::Claude, SID, &p, &conv, Some(ChatPos::After(0)));
    assert_ne!(other["conv"], conv.as_str());
}

// ----------------------------------------------------------------- robustness

#[test]
fn an_unreadable_file_is_nothing_to_show() {
    let d = TempDir::new().unwrap();
    let missing = d.path().join("nope.jsonl");
    assert_eq!(rd(&missing, "", None), json!({"conv": ""}));
    // A directory is not a transcript.
    assert_eq!(rd(d.path(), "", None), json!({"conv": ""}));
    let empty = d.path().join("empty.jsonl");
    std::fs::write(&empty, b"").unwrap();
    let r = rd(&empty, "", None);
    assert!(texts(&r).is_empty());
    assert_eq!((n(&r, "start"), n(&r, "end")), (0, 0));
}

#[test]
fn garbage_never_panics() {
    let d = TempDir::new().unwrap();
    // A deterministic noise source.
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut blob: Vec<u8> = Vec::new();
    for round in 0..300 {
        match round % 6 {
            0 => blob.extend((0..(next() % 200)).map(|_| next() as u8)),
            1 => blob.extend_from_slice(&user("ok line").as_bytes()[..(next() % 20) as usize]),
            2 => blob.extend_from_slice(
                b"{\"type\":\"user\",\"message\":{\"content\":[1,null,{\"type\":5}]}}",
            ),
            3 => blob.extend_from_slice(&[0xff, 0xfe, 0xc0, b'\n']),
            4 => blob.extend_from_slice(
                b"{\"type\":\"assistant\",\"message\":{\"content\":\"\xff\xfe\"}}\r\n",
            ),
            _ => blob.extend_from_slice(user("fine").as_bytes()),
        }
        blob.push(b'\n');
    }
    blob.extend_from_slice(b"{\"type\":\"user\",\"message\":{\"content\":\"cut off");
    let p = d.path().join("g.jsonl");
    std::fs::write(&p, &blob).unwrap();
    for agent in [AgentKind::Claude, AgentKind::Codex] {
        let first = read(agent, SID, &p, "", None);
        let conv = first["conv"].as_str().unwrap().to_string();
        assert!(clip::reply_data(Op::Chat, &first).is_some());
        // Every offset, as either cursor, in the file and just past it.
        for off in (0..blob.len() as u64 + 3).step_by(131) {
            for pos in [ChatPos::After(off), ChatPos::Before(off)] {
                let r = read(agent, SID, &p, &conv, Some(pos));
                assert!(clip::reply_data(Op::Chat, &r).is_some(), "{pos:?}");
            }
        }
    }
    // Nothing but newlines, and nothing but one huge unterminated line.
    std::fs::write(&p, vec![b'\n'; 5000]).unwrap();
    assert!(texts(&rd(&p, "", None)).is_empty());
    std::fs::write(&p, vec![b'x'; 300_000]).unwrap();
    let r = rd(&p, "", None);
    assert!(texts(&r).is_empty());
    assert_eq!((n(&r, "start"), n(&r, "end")), (0, 0));
}

#[test]
fn worst_case_escaping_still_fits_one_frame() {
    // 64 KiB of characters that double in JSON, sealed for a device.
    let d = TempDir::new().unwrap();
    let q = "\"".repeat(8000);
    let p = write(
        &d,
        "a.jsonl",
        &(0..12).map(|_| user(&q)).collect::<Vec<_>>(),
    );
    let r = rd(&p, "", None);
    let reply = crate::remote::proto::reply_ok(1, clip::reply_data(Op::Chat, &r).unwrap());
    let key = crate::remote::keys::LinkKey::from_bytes([3u8; 32]);
    let sealed = key.seal("aad", reply.to_string().as_bytes()).unwrap();
    let frame = crate::remote::proto::msg_frame_str("device", &sealed);
    assert!(
        frame.len() <= crate::remote::proto::MAX_FRAME,
        "{}",
        frame.len()
    );
}

#[test]
fn conv_ids_depend_on_agent_session_and_path() {
    let x = Path::new("/a/x.jsonl");
    let a = conv_id(AgentKind::Claude, SID, x, None);
    assert_eq!(a.len(), 16);
    assert!(crate::remote::proto::is_menu_id(&a));
    assert_eq!(a, conv_id(AgentKind::Claude, SID, x, None));
    assert_ne!(a, conv_id(AgentKind::Codex, SID, x, None));
    assert_ne!(a, conv_id(AgentKind::Claude, "0a1b", x, None));
    assert_ne!(
        a,
        conv_id(AgentKind::Claude, SID, Path::new("/a/y.jsonl"), None)
    );
    // Without a file identity: SHA-256("claude\0<sid>\0/a/x.jsonl")[..12].
    let mut h = Sha256::new();
    h.update(format!("claude\0{SID}\0/a/x.jsonl").as_bytes());
    assert_eq!(a, crate::remote::keys::b64u(&h.finalize()[..12]));
    // The file's identity is part of the name.
    let i1 = conv_id(AgentKind::Claude, SID, x, Some((1, 2)));
    assert_ne!(a, i1);
    assert_eq!(i1, conv_id(AgentKind::Claude, SID, x, Some((1, 2))));
    assert_ne!(i1, conv_id(AgentKind::Claude, SID, x, Some((1, 3))));
    assert_ne!(i1, conv_id(AgentKind::Claude, SID, x, Some((2, 2))));
}

#[test]
fn replacing_the_file_changes_the_conv() {
    let d = TempDir::new().unwrap();
    let p = write(&d, "a.jsonl", &msgs(0..3));
    let first = rd(&p, "", None);
    let conv = first["conv"].as_str().unwrap().to_string();
    // Same content, rewritten in place: still the same file.
    assert_eq!(rd(&p, "", None)["conv"], conv.as_str());
    // Another file at the same path (the old one is kept so its inode is not
    // handed out again): another conversation, so the old position is ignored.
    let keep = d.path().join("old.jsonl");
    std::fs::rename(&p, &keep).unwrap();
    std::fs::write(&p, msgs(0..3).join("\n") + "\n").unwrap();
    let r = rd(&p, &conv, Some(ChatPos::After(0)));
    assert_ne!(r["conv"], conv.as_str());
    assert_eq!(r["items"].as_array().unwrap().len(), 3);
    assert_eq!(n(&r, "end"), std::fs::metadata(&p).unwrap().len());
    assert!(file_identity(&File::open(&p).unwrap()).is_some());
}

#[test]
fn only_the_sessions_own_file_name_is_served() {
    let c = AgentKind::Claude;
    let x = AgentKind::Codex;
    assert!(is_session_file(
        c,
        Path::new(&format!("/r/p/{SID}.jsonl")),
        SID
    ));
    assert!(!is_session_file(
        c,
        Path::new("/r/p/other-session.jsonl"),
        SID
    ));
    assert!(!is_session_file(
        c,
        Path::new(&format!("/r/p/x{SID}.jsonl")),
        SID
    ));
    assert!(!is_session_file(
        c,
        Path::new(&format!("/r/p/{SID}.json")),
        SID
    ));
    assert!(!is_session_file(
        c,
        Path::new(&format!("/r/p/{SID}.jsonl.bak")),
        SID
    ));
    let rollout = format!("/r/2026/11/12/rollout-2026-11-12T22-43-44-{SID}.jsonl");
    assert!(is_session_file(x, Path::new(&rollout), SID));
    assert!(!is_session_file(
        x,
        Path::new("/r/2026/11/12/rollout-2026-11-12T22-43-44-other.jsonl"),
        SID
    ));
    assert!(!is_session_file(
        x,
        Path::new(&format!("/r/{SID}.jsonl")),
        SID
    ));
    assert!(!is_session_file(x, Path::new("/"), SID));
}

#[test]
fn a_hard_linked_or_non_regular_leaf_is_refused() {
    let d = TempDir::new().unwrap();
    let cwd = d.path().join("proj").to_string_lossy().into_owned();
    let p = claude_file(d.path(), &cwd, &cwd);
    let b = bind(AgentKind::Claude, &cwd, IdSource::Hook);
    assert!(open(&b, d.path(), &p, &cwd).is_ok());
    // A second name for the same file: not served (one hard link only).
    let other = d.path().join("elsewhere.jsonl");
    std::fs::hard_link(&p, &other).unwrap();
    assert_eq!(open(&b, d.path(), &p, &cwd).unwrap_err(), OpenErr::Refused);
    std::fs::remove_file(&other).unwrap();
    assert!(open(&b, d.path(), &p, &cwd).is_ok());
    // A directory named like the transcript is never opened.
    let dir = d.path().join(claude_project_dir_name("/w2"));
    std::fs::create_dir_all(dir.join(format!("{SID}.jsonl"))).unwrap();
    let b2 = bind(AgentKind::Claude, "/w2", IdSource::Hook);
    assert_eq!(
        open(&b2, d.path(), &dir.join(format!("{SID}.jsonl")), "/w2").unwrap_err(),
        OpenErr::Refused
    );
}

#[test]
fn a_read_error_while_looking_is_not_found_only_when_nothing_is_there() {
    let d = TempDir::new().unwrap();
    let b = bind(AgentKind::Claude, "/w", IdSource::Hook);
    // A missing root: nothing there.
    assert_eq!(find(&b, &d.path().join("absent")), Ok(None));
    assert_eq!(
        answer(&b, &d.path().join("absent"), "/w", "", None, &mut None),
        Some(json!({"conv": ""}))
    );
    // A root that cannot be listed (here a plain file): a failure, which the op
    // answers `internal`, not "no conversation".
    let file_root = d.path().join("not-a-dir");
    std::fs::write(&file_root, "x").unwrap();
    assert_eq!(find(&b, &file_root), Err(OpenErr::Io));
    assert_eq!(answer(&b, &file_root, "/w", "", None, &mut None), None);
    let cx = bind(AgentKind::Codex, "", IdSource::Hook);
    assert_eq!(find(&cx, &file_root), Err(OpenErr::Io));
}

#[test]
fn a_file_of_another_session_is_refused_even_inside_the_root() {
    // What a link inside the root pointing at another session's file looks
    // like once canonicalised: the right directory, the wrong name.
    let d = TempDir::new().unwrap();
    let cwd = d.path().join("proj").to_string_lossy().into_owned();
    let p = claude_file(d.path(), &cwd, &cwd);
    let other = p.with_file_name("0a1b2c3d-0000-4000-8000-0000000000ff.jsonl");
    std::fs::rename(&p, &other).unwrap();
    let b = bind(AgentKind::Claude, &cwd, IdSource::Hook);
    assert_eq!(
        open(&b, d.path(), &other, &cwd).unwrap_err(),
        OpenErr::Refused
    );
    assert!(locate(&b, d.path(), &cwd).is_none());
    // A path outside the root is refused whatever it is called.
    let out = TempDir::new().unwrap();
    let stray = out.path().join(format!("{SID}.jsonl"));
    std::fs::write(&stray, "{}\n").unwrap();
    assert_eq!(
        open(&b, d.path(), &stray, &cwd).unwrap_err(),
        OpenErr::Refused
    );
}

// ------------------------------------------------------------------- locate

fn claude_file(root: &Path, cwd: &str, recorded_cwd: &str) -> PathBuf {
    let dir = root.join(claude_project_dir_name(cwd));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{SID}.jsonl"));
    let head = json!({"type":"last-prompt"}).to_string();
    let l = json!({"type":"user","cwd":recorded_cwd,"entrypoint":"cli",
                   "message":{"content":"hi"}})
    .to_string();
    std::fs::write(&p, format!("{head}\n{l}\n")).unwrap();
    p
}

fn bind(agent: AgentKind, cwd: &str, source: IdSource) -> Binding<'_> {
    Binding {
        agent,
        session_id: SID,
        record_cwd: cwd,
        source,
    }
}

#[test]
fn a_hook_or_process_binding_is_trusted_for_its_id() {
    let d = TempDir::new().unwrap();
    let cwd = d.path().join("proj").to_string_lossy().into_owned();
    let want = claude_file(d.path(), &cwd, "/somewhere/else");
    for src in [IdSource::Hook, IdSource::Process] {
        let got = locate(
            &bind(AgentKind::Claude, &cwd, src),
            d.path(),
            "/not/the/same",
        );
        assert_eq!(got.unwrap(), want.canonicalize().unwrap());
    }
}

#[test]
fn a_disk_binding_needs_the_transcript_cwd_to_be_the_pane_cwd() {
    let d = TempDir::new().unwrap();
    let cwd = d.path().join("proj").to_string_lossy().into_owned();
    claude_file(d.path(), &cwd, &cwd);
    let b = bind(AgentKind::Claude, &cwd, IdSource::Disk);
    assert!(locate(&b, d.path(), &cwd).is_some());
    // Spelled differently, same directory (rule 15).
    assert!(locate(&b, d.path(), &cwd.replace('\\', "/")).is_some());
    assert!(locate(&b, d.path(), "/another/dir").is_none());
    // Unknown pane cwd: the record is never compared with itself.
    assert!(locate(&b, d.path(), "").is_none());
    // A transcript that records a different cwd than the pane's.
    let other = TempDir::new().unwrap();
    let cwd2 = other.path().join("p2").to_string_lossy().into_owned();
    claude_file(other.path(), &cwd2, "/elsewhere");
    let b2 = bind(AgentKind::Claude, &cwd2, IdSource::Disk);
    assert!(locate(&b2, other.path(), &cwd2).is_none());
    // A transcript that records no cwd at all.
    let nocwd = TempDir::new().unwrap();
    let cwd3 = nocwd.path().join("p3").to_string_lossy().into_owned();
    let dir = nocwd.path().join(claude_project_dir_name(&cwd3));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{SID}.jsonl")),
        "{\"type\":\"last-prompt\"}\n",
    )
    .unwrap();
    let b3 = bind(AgentKind::Claude, &cwd3, IdSource::Disk);
    assert!(locate(&b3, nocwd.path(), &cwd3).is_none());
}

#[test]
fn answer_is_no_conversation_for_a_disk_binding_in_the_wrong_directory() {
    let d = TempDir::new().unwrap();
    let cwd = d.path().join("proj").to_string_lossy().into_owned();
    let p = claude_file(d.path(), &cwd, &cwd);
    append(&p, &(user("hello") + "\n"));
    let b = bind(AgentKind::Claude, &cwd, IdSource::Disk);
    let ans = |b: &Binding<'_>, cwd: &str| answer(b, d.path(), cwd, "", None, &mut None).unwrap();
    let ok = ans(&b, &cwd);
    assert_eq!(texts(&ok), vec!["hi", "hello"]);
    assert_eq!(ok["agent"], "claude");
    assert_eq!(ans(&b, "/a/different/dir"), json!({"conv": ""}));
    assert_eq!(ans(&b, ""), json!({"conv": ""}));
    // The same transcript served to a pane the agent itself vouched for.
    let hook = bind(AgentKind::Claude, &cwd, IdSource::Hook);
    assert_eq!(texts(&ans(&hook, "/a/different/dir")), vec!["hi", "hello"]);
    // The conv is the same however it is reached.
    assert_eq!(ok["conv"], ans(&hook, "/x")["conv"]);
}

#[test]
fn answer_reuses_a_remembered_path_and_checks_it_every_time() {
    let d = TempDir::new().unwrap();
    let cwd = d.path().join("proj").to_string_lossy().into_owned();
    let p = claude_file(d.path(), &cwd, &cwd);
    let b = bind(AgentKind::Claude, &cwd, IdSource::Disk);
    let mut cache = None;
    let first = answer(&b, d.path(), &cwd, "", None, &mut cache).unwrap();
    // What the walk found, not its canonical form: that is redone every time.
    assert_eq!(cache.as_deref(), Some(p.as_path()));
    // A remembered path that does not exist (cached from somewhere else) is
    // dropped and the walk done again: the answer is the same.
    let mut stale = Some(d.path().join("gone.jsonl"));
    let again = answer(&b, d.path(), &cwd, "", None, &mut stale).unwrap();
    assert_eq!(again["conv"], first["conv"]);
    assert_eq!(stale.as_deref(), cache.as_deref());
    // The remembered path does not skip the checks: the pane moved away.
    let r = answer(&b, d.path(), "/elsewhere", "", None, &mut cache).unwrap();
    assert_eq!(r, json!({"conv": ""}));
    // And a file that is gone is "no conversation", not an error.
    std::fs::remove_file(&p).unwrap();
    let mut cache = Some(p.canonicalize().unwrap_or(p.clone()));
    assert_eq!(
        answer(&b, d.path(), &cwd, "", None, &mut cache),
        Some(json!({"conv": ""}))
    );
    assert!(cache.is_none());
}

#[test]
fn an_unreadable_conversation_is_an_error_not_no_conversation() {
    // A directory named like the transcript is "not a regular file": refused.
    // An I/O failure of an existing file is `None` (the op answers `internal`).
    let d = TempDir::new().unwrap();
    let cwd = d.path().join("proj").to_string_lossy().into_owned();
    let p = claude_file(d.path(), &cwd, &cwd);
    let b = bind(AgentKind::Claude, &cwd, IdSource::Hook);
    // Not valid as a root at all: canonicalize says not found -> no conversation.
    let nowhere = d.path().join("absent");
    assert_eq!(
        answer(&b, &nowhere, &cwd, "", None, &mut None),
        Some(json!({"conv": ""}))
    );
    // A file that is shorter than it was when read (truncated under us) is an
    // I/O error of the read, not an empty conversation.
    let opened = open(&b, d.path(), &p, &cwd).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(3)
        .unwrap();
    assert!(read_opened(AgentKind::Claude, SID, opened, "", None).is_err());
}

#[test]
fn codex_rollouts_are_found_by_id_and_checked_by_session_meta() {
    let d = TempDir::new().unwrap();
    let day = d.path().join("2026/11/12");
    std::fs::create_dir_all(&day).unwrap();
    let f = day.join(format!("rollout-2026-11-12T22-43-44-{SID}.jsonl"));
    let meta = json!({"type":"session_meta","payload":{"id":SID,"cwd":"/w/proj","source":"cli"}});
    std::fs::write(&f, format!("{meta}\n")).unwrap();
    let b = bind(AgentKind::Codex, "", IdSource::Disk);
    assert!(locate(&b, d.path(), "/w/proj").is_some());
    assert!(locate(&b, d.path(), "/w/other").is_none());
    assert!(locate(&b, d.path(), "").is_none());
    let hook = bind(AgentKind::Codex, "", IdSource::Hook);
    assert_eq!(
        locate(&hook, d.path(), "/w/other").unwrap(),
        f.canonicalize().unwrap()
    );
    // An unfinished first line proves nothing.
    std::fs::write(&f, meta.to_string()).unwrap();
    assert!(locate(&b, d.path(), "/w/proj").is_none());
}

#[test]
fn a_missing_or_directory_transcript_is_not_located() {
    let d = TempDir::new().unwrap();
    let hook = bind(AgentKind::Claude, "/w", IdSource::Hook);
    assert!(locate(&hook, d.path(), "/w").is_none());
    assert!(locate(&hook, &d.path().join("absent"), "/w").is_none());
    // A directory named like the transcript.
    let dir = d.path().join(claude_project_dir_name("/w"));
    std::fs::create_dir_all(dir.join(format!("{SID}.jsonl"))).unwrap();
    assert!(locate(&hook, d.path(), "/w").is_none());
    // An id that is not a session id never reaches the disk.
    let bad = Binding {
        agent: AgentKind::Claude,
        session_id: "../../x",
        record_cwd: "/w",
        source: IdSource::Hook,
    };
    assert!(locate(&bad, d.path(), "/w").is_none());
}

// ------------------------------------------------- real transcripts (manual)

mod real {
    //! A gate like `token_usage/real_tests.rs`: it runs the reader on this
    //! machine's own transcripts. It prints aggregate numbers only (counts,
    //! milliseconds, bytes, a file's index and size bucket, an offset), never a
    //! path or any text, and fails when anything is inconsistent.

    use super::*;
    use std::collections::HashSet;
    use std::time::Instant;

    const CALL_CAP: usize = 400;
    const WRAPPERS: [&str; 7] = [
        "<bash-stdout>",
        "<local-command-stdout>",
        "<system-reminder>",
        "<task-notification>",
        "<command-name>",
        "<environment_context>",
        "<user_shell_command>",
    ];

    #[derive(Default)]
    struct Stats {
        files: usize,
        calls: usize,
        user: usize,
        assistant: usize,
        tool: usize,
        max_ms: u128,
        max_bytes: usize,
        empty_files: usize,
        grew: usize,
        capped: usize,
        mid_line_positions: usize,
        failures: Vec<String>,
        /// Descriptors seen in the file being walked: `(type, id)`.
        seen: Vec<(String, String)>,
        /// Unique descriptors over all files, by type: png, jpeg, gif, webp.
        by_type: [usize; 4],
        descriptors_size: u64,
        max_line: u64,
        big_lines: usize,
        fetched: usize,
        fetched_parts: usize,
        fetched_bytes: u64,
        max_fetch_ms: u128,
    }

    /// Up to this many real pictures are fetched whole, in parts, over the run.
    const FETCH_CAP: usize = 20;

    fn bucket(len: u64) -> &'static str {
        match len {
            0..=999_999 => "<1MB",
            1_000_000..=9_999_999 => "1-10MB",
            10_000_000..=99_999_999 => "10-100MB",
            _ => ">=100MB",
        }
    }

    /// One reply, checked; returns its ids and `(start, end, older, newer)`.
    fn check(
        st: &mut Stats,
        r: &Value,
        ms: u128,
    ) -> Result<(Vec<String>, u64, u64, bool, bool), String> {
        st.calls += 1;
        st.max_ms = st.max_ms.max(ms);
        st.max_bytes = st.max_bytes.max(r.to_string().len());
        if r["conv"] == "" {
            return Err("answered no conversation".into());
        }
        if clip::reply_data(Op::Chat, r).is_none() {
            return Err("reply fails clip::reply_data".into());
        }
        let items = r["items"].as_array().ok_or("no items")?;
        let bytes: usize = items
            .iter()
            .map(|i| i["text"].as_str().map_or(0, str::len))
            .sum();
        if items.len() > 40 || bytes > 64 * 1024 {
            return Err(format!(
                "over the limits: {} items, {bytes} bytes",
                items.len()
            ));
        }
        let mut ids = Vec::new();
        for i in items {
            for im in i["images"].as_array().into_iter().flatten() {
                st.seen.push((
                    im["type"]
                        .as_str()
                        .ok_or("picture without a type")?
                        .to_string(),
                    im["id"]
                        .as_str()
                        .ok_or("picture without an id")?
                        .to_string(),
                ));
            }
            match i["role"].as_str() {
                Some("user") => st.user += 1,
                Some("assistant") => st.assistant += 1,
                Some("tool") => st.tool += 1,
                _ => return Err("bad role".into()),
            }
            let text = i["text"].as_str().unwrap_or("");
            if WRAPPERS.iter().any(|w| text.trim_start().starts_with(w)) {
                return Err("a wrapper tag starts an item".into());
            }
            ids.push(i["id"].as_str().ok_or("no id")?.to_string());
        }
        let g = |k: &str| r[k].as_u64().unwrap_or(u64::MAX);
        Ok((
            ids,
            g("start"),
            g("end"),
            r["older"] == true,
            r["newer"] == true,
        ))
    }

    fn one(st: &mut Stats, agent: AgentKind, sid: &str, path: &Path) -> Result<(), String> {
        let len0 = std::fs::metadata(path).map_err(|_| "stat")?.len();
        let timed = |conv: &str, pos: Option<ChatPos>| {
            let t = Instant::now();
            let r = read_with(agent, sid, path, conv, pos, true);
            (r, t.elapsed().as_millis())
        };
        st.seen.clear();
        let (first, ms) = timed("", None);
        let at = |what: &str, off: u64| format!("{what} (offset {off})");
        let (ids, start0, end0, mut older, _) = check(st, &first, ms).map_err(|e| at(&e, 0))?;
        let conv = first["conv"].as_str().unwrap_or("").to_string();
        let mut back: HashSet<String> = HashSet::new();
        for id in ids {
            if !back.insert(id) {
                return Err(at("duplicate id", start0));
            }
        }
        let mut capped = false;
        let mut start = start0;
        let mut calls = 0;
        while older {
            calls += 1;
            if calls > CALL_CAP {
                // A very large conversation: not walked to its end here.
                capped = true;
                break;
            }
            let (r, ms) = timed(&conv, Some(ChatPos::Before(start)));
            let (ids, s, e, o, _) = check(st, &r, ms).map_err(|m| at(&m, start))?;
            if r["conv"] != conv.as_str() {
                return Err(at("conv changed", start));
            }
            if e != start {
                return Err(at("before not honoured", start));
            }
            if s >= start {
                return Err(at("no progress going back", start));
            }
            for id in ids {
                if !back.insert(id) {
                    return Err(at("duplicate id", start));
                }
            }
            start = s;
            older = o;
        }
        if !capped && start != 0 {
            return Err(at("older=false but start is not 0", start));
        }
        // Forward from the very start.
        let mut fwd: HashSet<String> = HashSet::new();
        let mut e = 0u64;
        let mut calls = 0;
        loop {
            calls += 1;
            if calls > CALL_CAP {
                capped = true;
                break;
            }
            let (r, ms) = timed(&conv, Some(ChatPos::After(e)));
            let (ids, s, end, _, newer) = check(st, &r, ms).map_err(|m| at(&m, e))?;
            if s != e {
                return Err(at("after not honoured", e));
            }
            if end < e || (end == e && newer) {
                return Err(at("no progress going forward", e));
            }
            for id in ids {
                if !fwd.insert(id) {
                    return Err(at("duplicate id", e));
                }
            }
            if !newer {
                break;
            }
            e = end;
        }
        let len1 = std::fs::metadata(path).map_err(|_| "stat")?.len();
        if capped {
            st.capped += 1;
        } else if len0 != len1 {
            // A live session appended meanwhile: the two walks saw different ends.
            st.grew += 1;
        } else if back != fwd {
            return Err(at(
                &format!("id sets differ: back {}, forward {}", back.len(), fwd.len()),
                end0,
            ));
        }
        if back.is_empty() {
            st.empty_files += 1;
        }
        // The longest line this file has, and how many are "big".
        {
            use std::io::BufRead;
            let mut rdr =
                std::io::BufReader::with_capacity(1 << 20, File::open(path).map_err(|_| "open")?);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                let n = rdr.read_until(b'\n', &mut buf).map_err(|_| "read")? as u64;
                if n == 0 {
                    break;
                }
                if n > MAX_LINE {
                    st.big_lines += 1;
                }
                if n <= IMAGE_LINE_MAX {
                    st.max_line = st.max_line.max(n);
                }
            }
        }
        // Pictures: count each listed one once, then fetch a few whole.
        let mut uniq: Vec<(String, String)> = Vec::new();
        for (ty, id) in std::mem::take(&mut st.seen) {
            if !uniq.iter().any(|(_, i)| *i == id) {
                uniq.push((ty, id));
            }
        }
        let mut cache = pics::PicCache::default();
        for (ty, id) in &uniq {
            let slot = ["image/png", "image/jpeg", "image/gif", "image/webp"]
                .iter()
                .position(|t| t == &ty.as_str())
                .ok_or("a listed picture of an unknown type")?;
            st.by_type[slot] += 1;
            if st.fetched >= FETCH_CAP {
                continue;
            }
            let t = Instant::now();
            let file = File::open(path).map_err(|_| "open")?;
            let opened = Opened {
                id: file_identity(&file),
                len: file.metadata().map_err(|_| "stat")?.len(),
                path: path.to_path_buf(),
                file,
            };
            let first = image_opened(
                agent, sid, opened, "pane", &conv, id, 0, true, &mut cache, &mut 0,
            )
            .ok_or("an image read failed")?;
            let (parts, size) = (
                first["parts"].as_u64().unwrap_or(0),
                first["size"].as_u64().unwrap_or(0),
            );
            if first.get("data").is_none() || parts == 0 || size == 0 {
                return Err(format!("a listed picture is gone ({ty})"));
            }
            let mut total = 0u64;
            for k in 0..parts {
                let file = File::open(path).map_err(|_| "open")?;
                let opened = Opened {
                    id: file_identity(&file),
                    len: file.metadata().map_err(|_| "stat")?.len(),
                    path: path.to_path_buf(),
                    file,
                };
                let r = image_opened(
                    agent, sid, opened, "pane", &conv, id, k as u32, true, &mut cache, &mut 0,
                )
                .ok_or("an image read failed")?;
                let ok = clip::reply_data(Op::Image, &r).ok_or("part fails clip::reply_data")?;
                if ok["type"] != ty.as_str()
                    || ok["size"].as_u64() != Some(size)
                    || ok["parts"].as_u64() != Some(parts)
                {
                    return Err("parts disagree".into());
                }
                total += crate::remote::keys::b64u_decode(ok["data"].as_str().unwrap_or(""))
                    .ok_or("bad part data")?
                    .len() as u64;
                st.fetched_parts += 1;
            }
            if total != size {
                return Err(format!("parts add up to {total}, not {size}"));
            }
            st.fetched += 1;
            st.fetched_bytes += size;
            st.descriptors_size += size;
            st.max_fetch_ms = st.max_fetch_ms.max(t.elapsed().as_millis());
        }
        Ok(())
    }

    fn files() -> Vec<(AgentKind, String, PathBuf)> {
        let home = dirs::home_dir().expect("a home directory");
        let mut claude: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        if let Ok(projects) = std::fs::read_dir(home.join(".claude").join("projects")) {
            for dir in projects.flatten() {
                for f in std::fs::read_dir(dir.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let p = f.path();
                    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Ok(md) = f.metadata() {
                        if md.is_file() {
                            claude.push((
                                md.modified().unwrap_or(std::time::UNIX_EPOCH),
                                md.len(),
                                p,
                            ));
                        }
                    }
                }
            }
        }
        let mut picked: Vec<PathBuf> = Vec::new();
        claude.sort_by_key(|c| std::cmp::Reverse(c.0));
        picked.extend(claude.iter().take(30).map(|c| c.2.clone()));
        claude.sort_by_key(|c| std::cmp::Reverse(c.1));
        for c in claude.iter().take(5) {
            if !picked.contains(&c.2) {
                picked.push(c.2.clone());
            }
        }
        let mut out: Vec<(AgentKind, String, PathBuf)> = picked
            .into_iter()
            .map(|p| {
                let sid = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                (AgentKind::Claude, sid, p)
            })
            .collect();
        // Codex: every rollout under the date tree, newest 10.
        let mut stack = vec![home.join(".codex").join("sessions")];
        let mut rollouts: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        while let Some(dir) = stack.pop() {
            for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = f.path();
                let Ok(ty) = f.file_type() else { continue };
                if ty.is_dir() {
                    stack.push(p);
                } else if p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                {
                    let m = f
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    rollouts.push((m, p));
                }
            }
        }
        rollouts.sort_by_key(|r| std::cmp::Reverse(r.0));
        out.extend(
            rollouts
                .into_iter()
                .take(10)
                .map(|(_, p)| (AgentKind::Codex, "real".to_string(), p)),
        );
        out
    }

    #[test]
    #[ignore = "walks this machine's real transcripts; prints aggregates only"]
    fn real_transcripts_page_consistently() {
        let list = files();
        let mut st = Stats::default();
        let (mut claude, mut codex) = (0, 0);
        for (idx, (agent, sid, path)) in list.iter().enumerate() {
            st.files += 1;
            match agent {
                AgentKind::Claude => claude += 1,
                AgentKind::Codex => codex += 1,
            }
            let size = std::fs::metadata(path).map_or(0, |m| m.len());
            if let Err(e) = one(&mut st, *agent, sid, path) {
                st.failures.push(format!(
                    "file #{idx} ({}, {}): {e}",
                    agent.as_str(),
                    bucket(size)
                ));
            }
        }
        println!(
            "real chat gate: files {} (claude {claude}, codex {codex}), calls {}, \
             items user {} assistant {} tool {}, max {} ms/call, max reply {} bytes, \
             files with no items {}, files that grew meanwhile {}, files stopped at the call cap {}, failures {}",
            st.files,
            st.calls,
            st.user,
            st.assistant,
            st.tool,
            st.max_ms,
            st.max_bytes,
            st.empty_files,
            st.grew,
            st.capped,
            st.failures.len()
        );
        println!(
            "real picture gate: descriptors png {} jpeg {} gif {} webp {}, \
             largest parsed line {} bytes, lines over {} bytes {}, \
             pictures fetched whole {} ({} parts, {} bytes, slowest {} ms)",
            st.by_type[0],
            st.by_type[1],
            st.by_type[2],
            st.by_type[3],
            st.max_line,
            MAX_LINE,
            st.big_lines,
            st.fetched,
            st.fetched_parts,
            st.fetched_bytes,
            st.max_fetch_ms
        );
        let _ = (st.mid_line_positions, st.descriptors_size);
        for f in &st.failures {
            println!("FAILURE {f}");
        }
        assert!(st.failures.is_empty(), "{} failures", st.failures.len());
    }
}
