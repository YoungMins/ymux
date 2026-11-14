//! Redaction and clipping of everything that leaves for a phone (PROTOCOL.md
//! section 5.2, last paragraph), plus shape checks on what the app hands back.
//!
//! The app's page is our own, but the reply that reaches the socket is built
//! here from validated pieces only: unknown fields are dropped, strings are
//! cleaned, redacted and clipped, enums are matched against fixed lists.

use serde_json::{json, Map, Value};

use super::proto::{is_menu_id, is_uuid, Op, Pane, LINES_MAX};
use crate::activity::redact::{clip, redact};

/// Longest screen line or question line sent out.
pub const LINE_MAX: usize = 2000;
/// Longest title, workspace label or option label.
pub const LABEL_MAX: usize = 120;
/// More panes than this are not sent (keeps the frame well under 256 KiB).
pub const PANES_MAX: usize = 200;
const MENU_MAX: usize = 50;
const QUESTION_MAX: usize = 40;

const STATUSES: [&str; 5] = ["working", "waiting", "done", "idle", "none"];

/// Unicode format (Cf), line (Zl) and paragraph (Zp) separator characters:
/// invisible, and able to reorder or hide text on the phone (bidi overrides,
/// zero-width joiners, tag characters) or to split a redaction pattern.
/// Explicit ranges, so no Unicode table is needed.
pub fn is_format_char(c: char) -> bool {
    matches!(u32::from(c),
        0x00AD
        | 0x0600..=0x0605
        | 0x061C
        | 0x06DD
        | 0x070F
        | 0x0890..=0x0891
        | 0x08E2
        | 0x180E
        | 0x200B..=0x200F
        | 0x2028..=0x202E
        | 0x2060..=0x2064
        | 0x2066..=0x206F
        | 0xFEFF
        | 0xFFF9..=0xFFFB
        | 0x110BD
        | 0x110CD
        | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A
        | 0xE0001
        | 0xE0020..=0xE007F)
}

/// Control characters become a space (tab, CR, LF) or vanish (the rest), and
/// so do format and separator characters, so a line cannot carry a terminal
/// escape or hidden text to the phone.
fn clean(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\t' | '\r' | '\n' => Some(' '),
            c if c.is_control() || is_format_char(c) => None,
            c => Some(c),
        })
        .collect()
}

/// A prompt as it may be typed: every control character except tab and
/// newline is removed (so no escape sequence), CR and CRLF become a newline.
pub fn clean_prompt(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\t' | '\n' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Is the first non-blank character of a prompt `!`? An agent CLI may run
/// such a line as a shell command, outside its permission rules. Blank
/// includes whitespace and invisible format characters.
pub fn starts_with_bang(text: &str) -> bool {
    text.chars()
        .find(|c| !c.is_whitespace() && !is_format_char(*c) && !c.is_control())
        == Some('!')
}

/// Clean, redact, then clip to `max` chars (the ellipsis counts).
pub fn text(s: &str, max: usize) -> String {
    // Bound the work first: a hostile 10 MB "line" costs nothing this way.
    let head: String = s.chars().take(max.saturating_mul(4)).collect();
    clip(&redact(&clean(&head)), max)
}

pub fn line(s: &str) -> String {
    text(s, LINE_MAX)
}

pub fn label(s: &str) -> String {
    text(s, LABEL_MAX)
}

/// `[a-z0-9_-]`, at most `max` chars, else empty.
fn token(s: &str, max: usize) -> String {
    if !s.is_empty()
        && s.chars().count() <= max
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        s.to_string()
    } else {
        String::new()
    }
}

fn status_of(v: Option<&Value>) -> String {
    match v.and_then(Value::as_str) {
        Some(s) if STATUSES.contains(&s) => s.to_string(),
        _ => "none".to_string(),
    }
}

/// A JSON number as a non-negative integer, tolerating a float spelling
/// (`1700000000000.0`) that a JS number can produce.
fn as_count(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.as_u64().unwrap_or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && *f >= 0.0 && *f < 9.0e15)
                .map_or(0, |f| f as u64)
        }),
        _ => 0,
    }
}

/// The pane list as it may leave the machine. `None` when `v` is not a list
/// (an object with a `panes` array is accepted too). Panes without a valid id
/// are dropped.
pub fn panes(v: &Value) -> Option<Vec<Pane>> {
    let arr = match v {
        Value::Array(a) => a,
        Value::Object(o) => o.get("panes")?.as_array()?,
        _ => return None,
    };
    let mut out = Vec::new();
    for p in arr.iter().take(PANES_MAX) {
        let Some(id) = p.get("pane").and_then(Value::as_str).filter(|s| is_uuid(s)) else {
            continue;
        };
        let get = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("");
        out.push(Pane {
            pane: id.to_string(),
            ws: label(get("ws")),
            title: label(get("title")),
            kind: token(get("kind"), 24),
            status: status_of(p.get("status")),
            since_ms: as_count(p.get("since_ms")),
            queued: u32::try_from(as_count(p.get("queued")).min(9999)).unwrap_or(0),
        });
    }
    Some(out)
}

fn strings(v: Option<&Value>, max_items: usize, keep_tail: bool) -> Vec<String> {
    let Some(arr) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    let items: Vec<&str> = arr.iter().filter_map(Value::as_str).collect();
    let start = if keep_tail {
        items.len().saturating_sub(max_items)
    } else {
        0
    };
    items
        .into_iter()
        .skip(start)
        .take(max_items)
        .map(line)
        .collect()
}

fn peek(data: &Value) -> Option<Value> {
    let o = data.as_object()?;
    let menu: Vec<Value> = o
        .get("menu")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let n = u32::try_from(m.get("n").and_then(Value::as_u64)?).ok()?;
                    Some(json!({
                        "n": n,
                        "label": label(m.get("label").and_then(Value::as_str).unwrap_or("")),
                        "selected": m.get("selected").and_then(Value::as_bool).unwrap_or(false),
                        "input": m.get("input").and_then(Value::as_bool).unwrap_or(false),
                    }))
                })
                .take(MENU_MAX)
                .collect()
        })
        .unwrap_or_default();
    let blocked = match o.get("blocked") {
        Some(Value::String(s)) => {
            let t = token(s, 32);
            if t.is_empty() {
                Value::Null
            } else {
                Value::String(t)
            }
        }
        _ => Value::Null,
    };
    let mut out = Map::new();
    out.insert(
        "lines".into(),
        json!(strings(o.get("lines"), LINES_MAX as usize, true)),
    );
    out.insert("menu".into(), Value::Array(menu));
    // One appearance of one menu; "" when there is none or the page sent
    // something that is not an id.
    let menu_id = o
        .get("menu_id")
        .and_then(Value::as_str)
        .filter(|m| is_menu_id(m))
        .unwrap_or("");
    out.insert("menu_id".into(), Value::String(menu_id.to_string()));
    out.insert(
        "question".into(),
        json!(strings(o.get("question"), QUESTION_MAX, false)),
    );
    out.insert("blocked".into(), blocked);
    out.insert("status".into(), Value::String(status_of(o.get("status"))));
    Some(Value::Object(out))
}

fn result(data: &Value, allowed: &[&str]) -> Option<Value> {
    let r = data.get("result")?.as_str()?;
    allowed.contains(&r).then(|| json!({ "result": r }))
}

/// The reply `data` for `op`, rebuilt from the app's answer, or `None` when
/// the answer does not fit the protocol.
pub fn reply_data(op: Op, data: &Value) -> Option<Value> {
    match op {
        Op::List => Some(json!({ "panes": panes(data.get("panes")?)? })),
        Op::Peek => peek(data),
        Op::Answer => result(data, &["sent", "stale", "gone", "failed"]),
        Op::Interrupt => result(data, &["sent", "gone", "failed"]),
        Op::Prompt => result(
            data,
            &["sent", "queued", "waiting", "blocked", "gone", "failed"],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    /// The phone echoes an option label back and the page's `labelMatches`
    /// (src/remote/remoteModel.ts) accepts it when it is equal to the screen's
    /// or a clipped copy: "first 119 chars + ellipsis" of the screen's own text.
    /// This pins what the desktop produces; a redaction inside the kept text
    /// makes the copy differ from the screen, so it must not be a prefix of it.
    #[test]
    fn clipped_label_is_a_prefix_of_the_screen_label_unless_redacted() {
        let plain = format!(
            "Yes, allow this command for the session {}",
            "and so on ".repeat(12)
        );
        assert_eq!(label("Yes"), "Yes");
        let sent = label(&plain);
        let head = sent.strip_suffix('…').expect("clipped with an ellipsis");
        assert_eq!(head.chars().count(), LABEL_MAX - 1);
        assert!(plain.starts_with(head));

        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let with_secret = format!("Yes, use {secret} {}", "and so on ".repeat(12));
        let sent = label(&with_secret);
        let head = sent.strip_suffix('…').expect("clipped with an ellipsis");
        assert!(
            !with_secret.starts_with(head),
            "a redacted label no longer matches the screen"
        );
    }

    #[test]
    fn lines_are_clipped_to_2000_and_titles_to_120() {
        let long = "word ".repeat(1000);
        let l = line(&long);
        assert_eq!(l.chars().count(), 2000);
        assert!(l.ends_with('…'));
        assert_eq!(label(&long).chars().count(), 120);
        assert_eq!(line("short"), "short");
        // Multi-byte text is cut on a character, never a byte.
        let ko = "한".repeat(2500);
        assert_eq!(line(&ko).chars().count(), 2000);
    }

    #[test]
    fn secrets_are_redacted_before_they_leave() {
        let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEF";
        let l = line(&format!("export ANTHROPIC_API_KEY={key} done"));
        assert!(!l.contains("abcdefghijklmnop"), "{l}");
        assert!(l.contains("done"));
        let t = label("ghp_abcdefghijklmnopqrstuvwxyz0123456789");
        assert!(!t.contains("abcdefghijklmnop"), "{t}");
        // A secret inside a menu label and a pane title too.
        let d = reply_data(
            Op::Peek,
            &json!({"menu":[{"n":1,"label":format!("use {key}"),"selected":true,"input":false}],
                    "lines":[], "question":[format!("Is {key} ok?")], "status":"waiting"}),
        )
        .unwrap();
        let s = d.to_string();
        assert!(!s.contains("abcdefghijklmnop"), "{s}");
    }

    #[test]
    fn control_characters_never_reach_the_phone() {
        assert_eq!(line("a\x1b[31mred\x07\x00"), "a[31mred");
        assert_eq!(line("a\tb\r\nc"), "a b  c");
    }

    #[test]
    fn format_and_separator_characters_are_dropped() {
        // Every character the brief names, one by one, inside a word.
        let named: Vec<u32> = [
            vec![0x00AD, 0x061C, 0x06DD, 0x070F, 0x180E, 0xFEFF, 0xE0001],
            (0x0600..=0x0605).collect(),
            (0x200B..=0x200F).collect(),
            (0x2028..=0x202E).collect(),
            (0x2060..=0x2064).collect(),
            (0x2066..=0x206F).collect(),
            (0xFFF9..=0xFFFB).collect(),
            (0xE0020..=0xE007F).collect(),
        ]
        .concat();
        for cp in named {
            let c = char::from_u32(cp).unwrap();
            assert_eq!(line(&format!("a{c}b")), "ab", "U+{cp:04X}");
            assert_eq!(label(&format!("a{c}b")), "ab", "U+{cp:04X}");
        }
        // Ordinary text, accents, emoji and Korean are untouched.
        assert_eq!(
            line("café 한글 \u{1F600} naïve"),
            "café 한글 \u{1F600} naïve"
        );
        // A right-to-left override cannot reorder a label.
        assert_eq!(label("Yes\u{202E}oN"), "YesoN");
        // Splitting a key with zero-width characters does not hide it.
        let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEF";
        let split = format!("{}\u{200B}{}", &key[..20], &key[20..]);
        assert!(!line(&split).contains("abcdefghijklmnop"));
    }

    #[test]
    fn prompt_text_loses_controls_but_keeps_tab_and_newline() {
        assert_eq!(clean_prompt("a\x1b[31mb\x07\x00c"), "a[31mbc");
        assert_eq!(clean_prompt("a\r\nb\rc\nd\te"), "a\nb\nc\nd\te");
        assert_eq!(clean_prompt("a\u{9b}31mb\x7f"), "a31mb");
        assert_eq!(clean_prompt("한글 \u{1F600}"), "한글 \u{1F600}");
        assert_eq!(clean_prompt("\r\r\n"), "\n\n");
    }

    #[test]
    fn a_bang_first_is_detected_through_blanks_and_invisibles() {
        assert!(starts_with_bang("!ls"));
        assert!(starts_with_bang("  \t\n !ls"));
        assert!(starts_with_bang("\u{200B}\u{feff}!ls"));
        assert!(starts_with_bang("\u{a0}!ls"));
        assert!(!starts_with_bang("hello !ls"));
        assert!(!starts_with_bang("say !"));
        assert!(!starts_with_bang(""));
        assert!(!starts_with_bang("   "));
    }

    #[test]
    fn huge_input_is_bounded_before_the_regexes_run() {
        let huge = "lorem ipsum ".repeat(400_000);
        assert_eq!(line(&huge).chars().count(), 2000);
    }

    #[test]
    fn pane_list_is_sanitised() {
        let v = json!([
            {"pane":P,"ws":"main","title":"Fix the bug","kind":"claude","status":"waiting",
             "since_ms":1700000000000u64,"queued":2, "extra":"dropped"},
            {"pane":"not-a-uuid","title":"x"},
            {"pane":P,"ws":"w","title":"t","kind":"Evil Kind!","status":"exploding",
             "since_ms":-5,"queued":"many"},
            {"pane":P,"since_ms":1700000000000.0},
            5
        ]);
        let out = panes(&v).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].status, "waiting");
        assert_eq!(out[0].since_ms, 1_700_000_000_000);
        assert_eq!(out[0].queued, 2);
        assert_eq!(out[1].kind, "");
        assert_eq!(out[1].status, "none");
        assert_eq!(out[1].since_ms, 0);
        assert_eq!(out[1].queued, 0);
        assert_eq!(out[2].since_ms, 1_700_000_000_000);
        let ser = serde_json::to_value(&out[0]).unwrap();
        assert!(ser.get("extra").is_none());
        assert_eq!(ser["pane"], P);
        assert!(panes(&json!("x")).is_none());
        assert!(panes(&json!({"panes": 3})).is_none());
        assert_eq!(panes(&json!({"panes": []})).unwrap().len(), 0);
    }

    #[test]
    fn pane_count_is_capped() {
        let many: Vec<Value> = (0..500).map(|_| json!({"pane": P})).collect();
        assert_eq!(panes(&Value::Array(many)).unwrap().len(), PANES_MAX);
    }

    #[test]
    fn peek_reply_is_rebuilt_in_protocol_shape() {
        let lines: Vec<String> = (0..300).map(|i| format!("line {i}")).collect();
        let d = reply_data(
            Op::Peek,
            &json!({"lines":lines,
                "menu":[{"n":1,"label":"Yes","selected":true,"input":false},
                        {"n":-1,"label":"bad n"},
                        {"n":2,"label":"No","junk":1}],
                "question":["Allow?"], "blocked":"permission", "status":"waiting",
                "menu_id":"Ab3_-z", "secret_extra":"nope"}),
        )
        .unwrap();
        assert_eq!(d["menu_id"], "Ab3_-z");
        assert_eq!(d["lines"].as_array().unwrap().len(), 200);
        // The newest lines are kept.
        assert_eq!(d["lines"][199], "line 299");
        assert_eq!(d["menu"].as_array().unwrap().len(), 2);
        assert_eq!(d["menu"][1]["selected"], false);
        assert_eq!(d["blocked"], "permission");
        assert_eq!(d["status"], "waiting");
        assert!(d.get("secret_extra").is_none());
        assert!(reply_data(Op::Peek, &json!([1])).is_none());
        let d = reply_data(Op::Peek, &json!({"blocked":"Has Spaces","status":"x"})).unwrap();
        assert_eq!(d["blocked"], Value::Null);
        assert_eq!(d["status"], "none");
        assert_eq!(d["lines"], json!([]));
        // No menu id, or one that is not an id, is the empty string.
        assert_eq!(d["menu_id"], "");
        for bad in [
            json!("has space"),
            json!("x".repeat(33)),
            json!(5),
            json!(null),
        ] {
            let d = reply_data(Op::Peek, &json!({"menu_id": bad})).unwrap();
            assert_eq!(d["menu_id"], "");
        }
    }

    #[test]
    fn the_page_can_never_answer_refused() {
        // `refused` is the desktop's own answer to a `!` prompt.
        assert_eq!(reply_data(Op::Prompt, &json!({"result":"refused"})), None);
        assert_eq!(reply_data(Op::Answer, &json!({"result":"refused"})), None);
    }

    #[test]
    fn results_are_matched_per_op() {
        let ok = |op, r: &str| reply_data(op, &json!({"result": r}));
        assert_eq!(ok(Op::Answer, "stale"), Some(json!({"result":"stale"})));
        assert_eq!(ok(Op::Answer, "queued"), None);
        assert_eq!(ok(Op::Interrupt, "sent"), Some(json!({"result":"sent"})));
        assert_eq!(ok(Op::Interrupt, "stale"), None);
        assert_eq!(ok(Op::Prompt, "waiting"), Some(json!({"result":"waiting"})));
        assert_eq!(ok(Op::Prompt, "stale"), None);
        assert_eq!(reply_data(Op::Prompt, &json!({})), None);
        assert_eq!(reply_data(Op::Prompt, &json!({"result": 3})), None);
        assert_eq!(reply_data(Op::List, &json!({})), None);
        assert!(reply_data(Op::List, &json!({"panes": []})).is_some());
    }
}
