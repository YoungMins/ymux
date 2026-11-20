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

/// Longest `user` / `assistant` text of a `chat` item.
pub const CHAT_TEXT_MAX: usize = 8000;
/// Longest `tool` text of a `chat` item.
pub const CHAT_TOOL_MAX: usize = 200;
/// Most `chat` items in one reply.
pub const CHAT_ITEMS_MAX: usize = 40;
/// The texts of one `chat` reply total at most this many UTF-8 bytes.
pub const CHAT_BYTES_MAX: usize = 64 * 1024;

/// Message text as written: CR and CRLF become a newline, newline and tab
/// stay, every other control, format, line and paragraph separator character
/// goes (PROTOCOL.md 5.2, last paragraph).
fn clean_chat(s: &str) -> String {
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
            c if c.is_control() || is_format_char(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// A `chat` item's `user` / `assistant` text: cleaned (newlines kept),
/// redacted, clipped to [`CHAT_TEXT_MAX`] chars (the ellipsis counts).
pub fn chat_text(s: &str) -> String {
    // The `…` that marks a cut can itself be redacted (a value right after
    // `password:`), so the text is passed through until it no longer changes:
    // what leaves is what `reply_data` would make of it again, to the byte.
    let mut t = chat_text_pass(s);
    for _ in 0..3 {
        let next = chat_text_pass(&t);
        if next == t {
            break;
        }
        t = next;
    }
    t
}

fn chat_text_pass(s: &str) -> String {
    // Order matters: clean first (so removed characters cannot move a cut
    // across a secret), then bound the work at TEXT_PRECUT chars, then redact.
    // A cut can separate a secret from its closer (a quote, the `@` of URL
    // userinfo), so after cutting the redactor is also shown those closers and
    // the last, partial line is not emitted.
    let cleaned = clean_chat(s);
    if cleaned.chars().nth(TEXT_PRECUT).is_none() {
        return clip(&redact(&cleaned), CHAT_TEXT_MAX);
    }
    let head: String = cleaned.chars().take(TEXT_PRECUT).collect();
    let mut t = redact_cut(&head);
    drop_partial_line(&mut t);
    let mut t = clip(&t, CHAT_TEXT_MAX - 1);
    if !t.ends_with('…') {
        t.push('…');
    }
    t
}

/// Message text is looked at up to this many (cleaned) chars before redaction.
pub const TEXT_PRECUT: usize = 32_000;
/// After a cut, the partial last line goes; if there is no newline within this
/// many chars of the end, this many chars go instead.
pub const TEXT_TAIL: usize = 4000;
/// A tool input is looked at up to this many (cleaned) chars before redaction,
/// and when it was cut loses this many of its last chars.
pub const TOOL_PRECUT: usize = 4000;
pub const TOOL_TAIL: usize = 1000;

/// Redact text that was cut off. Patterns that need a closer (`"..."`,
/// `'...'`, userinfo up to `@`) cannot match a value whose closer was cut
/// away, so they are shown one at the end; whatever of it is left is removed.
fn redact_cut(t: &str) -> String {
    let mut r = redact(&format!("{t}\"'@"));
    let keep = r.trim_end_matches(['"', '\'', '@']).len();
    r.truncate(keep);
    r
}

fn drop_partial_line(t: &mut String) {
    match t.rfind('\n') {
        Some(i) if t[i + 1..].chars().count() <= TEXT_TAIL => t.truncate(i),
        _ => drop_chars(t, TEXT_TAIL),
    }
}

fn drop_chars(t: &mut String, n: usize) {
    let keep = t.chars().count().saturating_sub(n);
    *t = t.chars().take(keep).collect();
}

/// A `chat` item's `tool` text: one line, at most [`CHAT_TOOL_MAX`] chars.
pub fn chat_tool(s: &str) -> String {
    text(s, CHAT_TOOL_MAX)
}

/// `<name>: <summary>` for a tool call, one line of at most [`CHAT_TOOL_MAX`]
/// chars. Name and field are cleaned first, then (the field up to
/// [`TOOL_PRECUT`] chars) redacted, and only then cut to what fits, so a secret
/// is never cut in half before redaction sees it. A field that had to be cut
/// at [`TOOL_PRECUT`] also loses its last [`TOOL_TAIL`] chars.
pub fn chat_tool_line(name: &str, summary: &str) -> String {
    // The name is cleaned, then capped, then redacted (a capped name is cut
    // text like any other).
    let name = clean(name);
    let name = if name.chars().nth(TOOL_NAME_MAX).is_some() {
        let head: String = name.chars().take(TOOL_NAME_MAX).collect();
        redact_cut(&head)
    } else {
        redact(&name)
    };
    let cleaned = clean(summary);
    let cut = cleaned.chars().nth(TOOL_PRECUT).is_some();
    let s = if cut {
        let head: String = cleaned.chars().take(TOOL_PRECUT).collect();
        let mut s = redact_cut(&head);
        drop_chars(&mut s, TOOL_TAIL);
        s
    } else {
        redact(&cleaned)
    };
    let long = s.chars().count() > CHAT_TOOL_MAX;
    let mut s: String = s.chars().take(CHAT_TOOL_MAX).collect();
    if cut || long {
        s.push('…');
    }
    let line = if s.is_empty() {
        name
    } else {
        format!("{name}: {s}")
    };
    // Until stable, like `chat_text`: `reply_data` runs `chat_tool` on it again.
    let mut t = text(&line, CHAT_TOOL_MAX);
    for _ in 0..3 {
        let next = chat_tool(&t);
        if next == t {
            break;
        }
        t = next;
    }
    t
}

/// A tool name is at most this many chars.
pub const TOOL_NAME_MAX: usize = 100;

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

/// A `chat` reply as it may leave: the reader's own output, checked field by
/// field (roles, id and conv spellings, counts, the 64 KiB total) and every
/// text cleaned again. `{"conv":""}` stays as it is.
fn chat(data: &Value) -> Option<Value> {
    let o = data.as_object()?;
    let conv = o.get("conv")?.as_str()?;
    if conv.is_empty() {
        return Some(json!({"conv": ""}));
    }
    if !is_menu_id(conv) {
        return None;
    }
    let agent = o
        .get("agent")?
        .as_str()
        .filter(|a| matches!(*a, "claude" | "codex"))?;
    let raw = o.get("items")?.as_array()?;
    if raw.len() > CHAT_ITEMS_MAX {
        return None;
    }
    let mut total = 0usize;
    let mut items = Vec::with_capacity(raw.len());
    for it in raw {
        let id = it.get("id")?.as_str()?;
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return None;
        }
        let role = it.get("role")?.as_str()?;
        let text = it.get("text")?.as_str()?;
        let text = match role {
            "user" | "assistant" => chat_text(text),
            "tool" => chat_tool(text),
            _ => return None,
        };
        total += text.len();
        if total > CHAT_BYTES_MAX {
            return None;
        }
        items.push(json!({"id": id, "role": role, "text": text,
                          "ts_ms": as_count(it.get("ts_ms"))}));
    }
    let pos = |k: &str| {
        o.get(k)?
            .as_u64()
            .filter(|n| *n <= super::proto::CHAT_POS_MAX)
    };
    Some(json!({
        "conv": conv, "agent": agent, "items": items,
        "start": pos("start")?, "end": pos("end")?,
        "older": o.get("older")?.as_bool()?, "newer": o.get("newer")?.as_bool()?,
    }))
}

/// The reply `data` for `op`, rebuilt from the app's answer, or `None` when
/// the answer does not fit the protocol.
pub fn reply_data(op: Op, data: &Value) -> Option<Value> {
    match op {
        Op::List => Some(json!({ "panes": panes(data.get("panes")?)? })),
        Op::Peek => peek(data),
        Op::Chat => chat(data),
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
    fn chat_text_keeps_newlines_and_tabs_and_drops_the_rest() {
        assert_eq!(chat_text("a\r\nb\rc\nd\te"), "a\nb\nc\nd\te");
        // Controls, escapes, bidi/zero-width, NEL, LS and PS go.
        assert_eq!(
            chat_text("a\u{1b}[31mb\u{0}\u{7}\u{85}c\u{200b}d\u{202e}e\u{2028}f\u{2029}g"),
            "a[31mbcdefg"
        );
        assert_eq!(chat_text("한국어 문장\n- 목록"), "한국어 문장\n- 목록");
        // Redaction runs after cleaning, so a split key is still found.
        let key = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let cut = format!("use gh\u{200b}p_{}", &key[3..]);
        assert!(!chat_text(&cut).contains("abcdefghijklmnop"));
        assert!(!chat_text(&format!("token {key}\nnext")).contains("abcdefghijklmnop"));
        assert!(chat_text(&format!("x {key}\nnext")).ends_with("\nnext"));
    }

    #[test]
    fn chat_text_is_clipped_to_8000_chars_with_an_ellipsis() {
        let long = "word ".repeat(5000);
        let t = chat_text(&long);
        assert_eq!(t.chars().count(), CHAT_TEXT_MAX);
        assert!(t.ends_with('…'));
        let exact = "a ".repeat(4000);
        assert_eq!(chat_text(&exact), exact);
        // A tool line is one line of at most 200 chars.
        let t = chat_tool(&format!("Bash: {}\nsecond", "x ".repeat(300)));
        assert_eq!(t.chars().count(), CHAT_TOOL_MAX);
        assert!(!t.contains('\n'));
        assert_eq!(chat_tool("Bash: a\nb"), "Bash: a b");
    }

    #[test]
    fn chat_text_cleaning_is_idempotent() {
        let key = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        for s in [
            format!("password: hunter2 {key}\n```rust\nfn main() {{}}\n```\n| a | b |"),
            "plain".repeat(3000),
            "word ".repeat(5000),
        ] {
            let once = chat_text(&s);
            assert_eq!(chat_text(&once), once);
        }
    }

    fn chat_item(id: &str, role: &str, text: &str) -> Value {
        json!({"id": id, "role": role, "text": text, "ts_ms": 5})
    }

    #[test]
    fn chat_reply_is_rebuilt_field_by_field() {
        let good = json!({
            "conv": "abc_-1", "agent": "claude", "extra": 1,
            "items": [chat_item("0.0", "user", "a\nb"), chat_item("12.1", "tool", "Bash: ls")],
            "start": 0, "end": 40, "older": false, "newer": true,
        });
        let d = reply_data(Op::Chat, &good).unwrap();
        assert!(d.get("extra").is_none());
        assert_eq!(d["items"][0]["text"], "a\nb");
        assert_eq!(d["items"][1]["role"], "tool");
        assert_eq!(
            (d["start"].clone(), d["end"].clone()),
            (json!(0), json!(40))
        );
        assert_eq!(d["newer"], true);
        // Nothing to show.
        assert_eq!(
            reply_data(Op::Chat, &json!({"conv": "", "items": [1]})),
            Some(json!({"conv": ""}))
        );
        let with = |f: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            f(&mut v);
            reply_data(Op::Chat, &v)
        };
        assert!(with(&|v| v["conv"] = json!("a b")).is_none());
        assert!(with(&|v| v["agent"] = json!("gemini")).is_none());
        assert!(with(&|v| v["items"][0]["role"] = json!("system")).is_none());
        assert!(with(&|v| v["items"][0]["id"] = json!("a.b")).is_none());
        assert!(with(&|v| v["items"][0]["id"] = json!("1".repeat(41))).is_none());
        assert!(with(&|v| v["start"] = json!(1u64 << 53)).is_none());
        assert!(with(&|v| v["older"] = json!("no")).is_none());
        assert!(with(&|v| v["items"] = json!([1])).is_none());
        // More than 40 items, or more than 64 KiB of text, is not a reply.
        let many: Vec<Value> = (0..41)
            .map(|i| chat_item(&i.to_string(), "user", "x"))
            .collect();
        assert!(with(&|v| v["items"] = json!(many)).is_none());
        let big: Vec<Value> = (0..9)
            .map(|i| chat_item(&i.to_string(), "user", &"x ".repeat(4000)))
            .collect();
        assert!(with(&|v| v["items"] = json!(big)).is_none());
        // A text the reader should have cleaned is cleaned again.
        let d = with(&|v| v["items"][0]["text"] = json!("x\u{1b}y")).unwrap();
        assert_eq!(d["items"][0]["text"], "xy");
    }

    #[test]
    fn a_secret_straddling_a_pre_cut_is_never_emitted_as_a_fragment() {
        // AWS-secret shaped: 40 chars of base64 with mixed case, digits, `/`.
        let token = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        assert_eq!(token.len(), 40);
        // Whole inside the input: redacted by the pattern itself.
        let t = chat_text(&format!("key {token} end"));
        assert!(!t.contains(&token[..12]), "{t:?}");
        // Cleaning comes first: control characters in front of a secret cannot
        // move the cut across it (they are gone before anything is counted).
        let pw = "Pw!#".repeat(500);
        let url = |pad: &str| format!("{pad}https://bob:{pw}@example.com tail");
        let t = chat_text(&url(&"\u{1b}".repeat(31_000)));
        assert!(!t.contains("Pw!#Pw!#"), "{t:?}");
        let t = chat_tool_line("Bash", &url(&"\u{7}".repeat(3_000)));
        assert!(!t.contains("Pw!#Pw!#"), "{t:?}");
        // A real straddle: 20 chars of the token before the cut, no newline.
        let pad = "ab ".repeat(TEXT_PRECUT / 3 - 6);
        assert_eq!(pad.chars().count(), TEXT_PRECUT - 20);
        let t = chat_text(&format!("{pad}{token} and more"));
        assert!(!t.contains(&token[..12]), "{t:?}");
        assert!(t.ends_with('…'), "{t:?}");
        // Userinfo whose `@` is past the cut, and a quoted value whose closing
        // quote is: patterns that need a closer are shown one.
        let t = chat_text(&format!(
            "https://bob:{}@example.com",
            "Pw!#".repeat(10_000)
        ));
        assert!(!t.contains("Pw!#Pw!#"), "{t:?}");
        let secret = "Zq7 Xk9 ".repeat(5_000);
        let t = chat_text(&format!("password = \"{secret}\" tail"));
        assert!(!t.contains("Zq7 Xk9 Zq7"), "{t:?}");
        // The redactor itself catches them (the tail drop is a second line).
        let r = redact_cut(&format!("password = \"{secret}"));
        assert!(
            !r.contains("Zq7 Xk9 Zq7") && r.contains("[REDACTED]"),
            "{r:?}"
        );
        let r = redact_cut(&format!("see https://bob:{}", "Pw!#".repeat(500)));
        assert!(!r.contains("Pw!#Pw!#") && r.contains("[REDACTED]"), "{r:?}");
        // Tool field cut inside a 3000-char quoted value (it shows its start).
        let secret = "Zq7 Xk9 ".repeat(500);
        let t = chat_tool_line("Bash", &format!("export API_PASSWORD=\"{secret}\" tail"));
        assert!(!t.contains("Zq7 Xk9 Zq7"), "{t:?}");
        assert!(t.starts_with("Bash"), "{t:?}");
        // Cut away from its closing quote (secret longer than the cut).
        let secret = "Zq7 Xk9 ".repeat(1_000);
        let t = chat_tool_line("Bash", &format!("export API_PASSWORD=\"{secret}\" tail"));
        assert!(!t.contains("Zq7 Xk9 Zq7"), "{t:?}");
        // The tool's name is cleaned and redacted too.
        let t = chat_tool_line(&format!("A\u{1b}\nB {token}"), "x");
        assert!(
            !t.contains(&token[..12]) && !t.contains('\n') && t.starts_with("A B "),
            "{t:?}"
        );
        let t = chat_tool_line(&format!("{} {token}", "n".repeat(90)), "x");
        assert!(!t.contains(&token[..12]), "{t:?}");
        // Cut at a newline: the partial last line is dropped, whole lines stay.
        let lines = "a line of text\n".repeat(3_000);
        let t = chat_text(&lines);
        assert!(
            t.ends_with('…') && t.starts_with("a line of text\n"),
            "{t:?}"
        );
        // Tool input over the cut is cut and says so.
        let t = chat_tool_line("Bash", &"y ".repeat(5_000));
        assert!(t.ends_with('…'), "{t:?}");
        // A field that fits is shown whole (and redacted when it is a secret).
        assert_eq!(chat_tool_line("Read", "/w/a.rs"), "Read: /w/a.rs");
        assert_eq!(chat_tool_line("Mystery", ""), "Mystery");
        let t = chat_tool_line("Bash", &format!("export K={token}"));
        assert!(!t.contains(&token[..12]), "{t:?}");
        // A long summary is cut to one line of 200 with an ellipsis.
        let t = chat_tool_line("Bash", &"x ".repeat(1000));
        assert_eq!(t.chars().count(), CHAT_TOOL_MAX);
        assert!(t.ends_with('…'));
    }

    /// A small deterministic generator.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    const TRIGGERS: [&str; 9] = [
        "password:",
        "password: ",
        "--token ",
        "API_KEY=",
        "Cookie: ",
        "secret=",
        "Authorization: Bearer ",
        "pass=",
        "https://bob:",
    ];

    /// Text built from words, newlines and trigger words, with a trigger put so
    /// that it ends right where the 8000-char clip (or a cut) will put its `…`.
    fn generated(g: &mut Lcg) -> String {
        let words = [
            "ab ",
            "foo ",
            "x\n",
            "한국어 ",
            "tab\t",
            "```\n",
            "- item\n",
            "ok. ",
        ];
        let target = [7_998usize, 7_999, 32_000, 31_990, 100, 20_000][g.below(6) as usize];
        let mut s = String::new();
        while s.chars().count() < target.saturating_sub(12) {
            s.push_str(words[g.below(words.len() as u64) as usize]);
        }
        if g.below(4) != 0 {
            let t = TRIGGERS[g.below(TRIGGERS.len() as u64) as usize];
            let keep = target.saturating_sub(t.chars().count());
            s = s.chars().take(keep).collect::<String>();
            s.push_str(t);
        }
        let extra = [0usize, 5, 50, 5_000, 12_000][g.below(5) as usize];
        for _ in 0..extra / 3 {
            s.push_str(["ab ", "x\n", "q "][g.below(3) as usize]);
        }
        s
    }

    #[test]
    fn chat_text_and_tool_lines_are_idempotent_and_bounded() {
        let mut g = Lcg(7);
        for i in 0..400 {
            let s = generated(&mut g);
            let t = chat_text(&s);
            assert_eq!(chat_text(&t), t, "case {i}");
            assert!(t.chars().count() <= CHAT_TEXT_MAX, "case {i}");
            // What `reply_data` does to an item changes nothing.
            let item = json!({"id": "0.0", "role": "assistant", "text": t, "ts_ms": 0});
            let reply = json!({"conv": "c", "agent": "claude", "items": [item],
                               "start": 0, "end": 1, "older": false, "newer": false});
            assert_eq!(reply_data(Op::Chat, &reply).unwrap()["items"][0]["text"], t);
            let name = ["Bash", "Read", "mcp__x__y"][g.below(3) as usize];
            let l = chat_tool_line(name, &s);
            assert_eq!(chat_tool(&l), l, "case {i}");
            assert!(l.chars().count() <= CHAT_TOOL_MAX, "case {i}");
        }
    }

    #[test]
    fn a_cut_right_after_a_trigger_word_stays_stable() {
        for t in TRIGGERS {
            // The ellipsis of the clip lands right after the trigger word.
            let keep = CHAT_TEXT_MAX - 1 - t.chars().count();
            let s = format!(
                "{}{t}{}",
                &"ab ".repeat(keep / 3 + 1)[..keep],
                "z ".repeat(9_000)
            );
            let out = chat_text(&s);
            assert_eq!(chat_text(&out), out, "{t:?}");
            // And on the pre-cut path: a long text whose last line ends there.
            let s = format!("{}{t}\n{}", "ab ".repeat(11_000), "q ".repeat(10_000));
            let out = chat_text(&s);
            assert_eq!(chat_text(&out), out, "{t:?}");
            let tool = chat_tool_line("Bash", &format!("{}{t}", "ab ".repeat(100)));
            assert_eq!(chat_tool(&tool), tool, "{t:?}");
        }
    }

    #[test]
    fn a_long_tool_name_is_cleaned_capped_and_redacted() {
        let key = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let name = format!("{}\u{1b}\n{key}", "n".repeat(TOOL_NAME_MAX - 10));
        let t = chat_tool_line(&name, "x");
        assert!(!t.contains("abcdefghij"), "{t:?}");
        assert!(t.chars().count() <= CHAT_TOOL_MAX);
        let t = chat_tool_line(&"m".repeat(5_000), "x");
        assert!(
            t.chars().count() <= CHAT_TOOL_MAX && t.ends_with(": x"),
            "{t:?}"
        );
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
