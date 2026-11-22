//! Pictures in the reader (descriptors, big lines) and the `image` op's pure
//! half. Synthetic transcripts only: the "pictures" are a real file signature
//! followed by generated filler, never a real image.

use super::pics::{self, PicCache, IMAGE_MAX, PART_BYTES};
use super::*;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use tempfile::TempDir;

const SID: &str = "0a1b2c3d-0000-4000-8000-000000000001";
const T0: &str = "2026-11-12T13:43:44.123Z";
const PANE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const JPG: [u8; 4] = [0xFF, 0xD8, 0xFF, 0xE0];

/// A signature followed by `total - sig.len()` bytes of a cheap generator
/// (so two pictures of one size still differ by `seed`).
fn bytes_of(sig: &[u8], total: usize, seed: u8) -> Vec<u8> {
    let mut v = sig.to_vec();
    let mut x = u32::from(seed) + 1;
    while v.len() < total {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        v.push((x >> 24) as u8);
    }
    v.truncate(total.max(sig.len()));
    v
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn png(total: usize) -> Vec<u8> {
    bytes_of(&PNG, total, 1)
}

fn block(data: &str) -> Value {
    json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":data}})
}

fn user_text(text: &str) -> String {
    json!({"type":"user","timestamp":T0,"isSidechain":false,
           "message":{"role":"user","content":text}})
    .to_string()
}

/// A user message with optional text and the given pictures (base64).
fn user_with(text: Option<&str>, pics: &[String]) -> String {
    let mut blocks: Vec<Value> = Vec::new();
    if let Some(t) = text {
        blocks.push(json!({"type":"text","text":t}));
    }
    blocks.extend(pics.iter().map(|p| block(p)));
    json!({"type":"user","timestamp":T0,"isSidechain":false,
           "message":{"role":"user","content":blocks}})
    .to_string()
}

/// A tool result line: its own text output plus the given pictures.
fn result_with(output: &str, pics: &[String]) -> String {
    let mut inner = vec![json!({"type":"text","text":output})];
    inner.extend(pics.iter().map(|p| block(p)));
    json!({"type":"user","timestamp":T0,"isSidechain":false,
           "message":{"role":"user","content":[
               {"type":"tool_result","tool_use_id":"t1","content":inner}]}})
    .to_string()
}

fn write(dir: &TempDir, name: &str, lines: &[String]) -> PathBuf {
    let p = dir.path().join(name);
    let mut s = lines.join("\n");
    s.push('\n');
    std::fs::write(&p, s).unwrap();
    p
}

fn items(line: &str, images: bool) -> Vec<Item> {
    super::line_items(AgentKind::Claude, line.as_bytes(), images)
}

fn codex_items(line: &Value, images: bool) -> Vec<Item> {
    super::line_items(AgentKind::Codex, line.to_string().as_bytes(), images)
}

fn rd(path: &Path, conv: &str, pos: Option<ChatPos>, images: bool) -> Value {
    read_with(AgentKind::Claude, SID, path, conv, pos, images)
}

fn opened(path: &Path) -> Opened {
    let file = File::open(path).unwrap();
    Opened {
        id: file_identity(&file),
        len: file.metadata().unwrap().len(),
        path: path.to_path_buf(),
        file,
    }
}

fn src_of(path: &Path) -> Src {
    let file = File::open(path).unwrap();
    Src::new(file.try_clone().unwrap(), file.metadata().unwrap().len())
}

fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap()
}

fn texts(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["text"].as_str().unwrap().to_string())
        .collect()
}

/// `image` on `path`, as the app does it (the file is already open and vetted).
fn fetch(path: &Path, conv: &str, id: &str, part: u32, cache: &mut PicCache) -> Option<Value> {
    super::image_opened(
        AgentKind::Claude,
        SID,
        opened(path),
        PANE,
        conv,
        id,
        part,
        true,
        cache,
        &mut 0,
    )
}

fn gone() -> Option<Value> {
    Some(json!({"result": "gone"}))
}

fn all_images(r: &Value) -> Vec<Value> {
    r["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|i| i["images"].as_array().cloned().unwrap_or_default())
        .collect()
}

// -------------------------------------------------------------- descriptors

#[test]
fn a_user_message_lists_its_pictures_by_file_signature() {
    let gif = bytes_of(b"GIF89a", 30, 3);
    let webp = bytes_of(b"RIFF\x10\x00\x00\x00WEBPVP8 ", 40, 4);
    let line = user_with(
        Some("look at these"),
        &[
            b64(&png(100)),
            b64(&bytes_of(&JPG, 50, 2)),
            b64(&gif),
            b64(&webp),
        ],
    );
    let it = items(&line, true);
    assert_eq!(it.len(), 1);
    assert_eq!((it[0].role, it[0].text.as_str()), ("user", "look at these"));
    let got: Vec<(u32, &str, u64)> = it[0]
        .images
        .iter()
        .map(|i| (i.ord, i.kind, i.size))
        .collect();
    assert_eq!(
        got,
        [
            (0, "image/png", 100),
            (1, "image/jpeg", 50),
            (2, "image/gif", 30),
            (3, "image/webp", 40)
        ]
    );
    // The declared media type is nowhere near the truth here (all say png).
    // With sharing off nothing is listed and the item is what it always was.
    let off = items(&line, false);
    assert_eq!(off.len(), 1);
    assert_eq!(off[0].text, "look at these");
    assert!(off[0].images.is_empty());
}

#[test]
fn a_user_line_with_only_pictures_is_an_item_with_empty_text() {
    let line = user_with(None, &[b64(&png(64))]);
    let it = items(&line, true);
    assert_eq!(it.len(), 1);
    assert_eq!(
        (it[0].role, it[0].text.as_str(), it[0].images.len()),
        ("user", "", 1)
    );
    assert!(items(&line, false).is_empty(), "off: nothing, as before");
    // Whitespace-only text next to a picture is the same thing.
    let ws = user_with(Some("  \n"), &[b64(&png(64))]);
    assert_eq!(items(&ws, true)[0].text, "");
}

#[test]
fn a_tool_result_with_pictures_is_a_tool_item_and_its_text_stays_out() {
    let line = result_with(
        "SECRET-TOOL-OUTPUT",
        &[b64(&png(80)), b64(&bytes_of(&JPG, 60, 5))],
    );
    let it = items(&line, true);
    assert_eq!(it.len(), 1);
    assert_eq!((it[0].role, it[0].text.as_str()), ("tool", ""));
    assert_eq!(it[0].images.len(), 2);
    assert!(!format!("{it:?}").contains("SECRET"));
    // Off: the whole line is the harness talking back, as it always was.
    assert!(items(&line, false).is_empty());
    // A result without a picture stays nothing, with sharing on too.
    assert!(items(&result_with("only text", &[]), true).is_empty());
    // Two results in one message: one item each, ordinals run on.
    let two = json!({"type":"user","timestamp":T0,"message":{"content":[
        {"type":"tool_result","tool_use_id":"a","content":[block(&b64(&png(40)))]},
        {"type":"tool_result","tool_use_id":"b","content":"plain string result"},
        {"type":"tool_result","tool_use_id":"c","content":[block(&b64(&png(41)))]}]}})
    .to_string();
    let it = items(&two, true);
    assert_eq!(it.len(), 2);
    assert_eq!((it[0].images[0].ord, it[1].images[0].ord), (0, 1));
    assert_eq!((it[0].images[0].size, it[1].images[0].size), (40, 41));
}

#[test]
fn sidechain_meta_and_summary_lines_show_no_pictures() {
    for extra in [
        json!({"isSidechain": true}),
        json!({"isMeta": true}),
        json!({"isCompactSummary": true}),
    ] {
        let mut v: Value = serde_json::from_str(&user_with(Some("x"), &[b64(&png(64))])).unwrap();
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(items(&v.to_string(), true).is_empty(), "{extra}");
        let mut t: Value = serde_json::from_str(&result_with("o", &[b64(&png(64))])).unwrap();
        t.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(items(&t.to_string(), true).is_empty(), "{extra}");
    }
    // An assistant message never lists a picture, even with one in it.
    let a = json!({"type":"assistant","message":{"content":[
        {"type":"text","text":"here"}, block(&b64(&png(64)))]}})
    .to_string();
    let it = items(&a, true);
    assert_eq!((it.len(), it[0].images.len()), (1, 0));
}

#[test]
fn only_what_passes_the_checks_is_listed_and_at_most_eight_an_item() {
    let mut pics: Vec<String> = vec![
        b64(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
        "!!!!not base64!!!!".into(),
        String::new(),
        b64(&bytes_of(b"RIFF\x10\x00\x00\x00WAVEfmt ", 40, 1)),
        "A".into(),
        "é€".into(),
        b64(&png(IMAGE_MAX as usize + 1)),
    ];
    pics.extend((0..12).map(|i| b64(&bytes_of(&PNG, 20 + i, i as u8))));
    let it = items(&user_with(Some("t"), &pics), true);
    let sizes: Vec<u64> = it[0].images.iter().map(|i| i.size).collect();
    assert_eq!(
        sizes,
        [20, 21, 22, 23, 24, 25, 26, 27],
        "eight, in order, no gaps"
    );
    let ords: Vec<u32> = it[0].images.iter().map(|i| i.ord).collect();
    assert_eq!(ords, (0..8).collect::<Vec<_>>());
    // A source that is not base64 data is not a picture.
    let url = json!({"type":"user","message":{"content":[
        {"type":"image","source":{"type":"url","url":"https://example.test/a.png"}},
        {"type":"image","source":{"type":"base64"}},
        {"type":"image"}]}})
    .to_string();
    assert!(items(&url, true).is_empty());
}

#[test]
fn three_hundred_pictures_in_one_line_stay_bounded() {
    let one = b64(&png(32));
    let flood = user_with(Some("t"), &vec![one.clone(); 300]);
    let it = items(&flood, true);
    assert_eq!((it.len(), it[0].images.len()), (1, 8));
    // 300 tool results with a picture each: 20 items (the line cap), 20 ordinals.
    let results: Vec<Value> = (0..300)
        .map(
            |i| json!({"type":"tool_result","tool_use_id":format!("t{i}"),"content":[block(&one)]}),
        )
        .collect();
    let line = json!({"type":"user","message":{"content":results}}).to_string();
    let it = items(&line, true);
    assert_eq!(it.len(), LINE_ITEMS_MAX);
    let last = it.last().unwrap().images[0].ord;
    assert_eq!(last as usize, LINE_ITEMS_MAX - 1);
    assert!(last < pics::LINE_MAX);
    // Their indices are in the pictures-only range.
    assert_eq!(it[0].idx as usize, LINE_ITEMS_MAX);
    assert_eq!(it.last().unwrap().idx as usize, 2 * LINE_ITEMS_MAX - 1);
}

#[test]
fn item_ids_are_the_same_with_sharing_on_and_off() {
    // A `!cmd` (a text-less Bash item) and a picture in one user message, text
    // items around picture-only ones, a tool result with a picture, and more
    // items than a line may hold.
    let d = TempDir::new().unwrap();
    let pic = b64(&png(64));
    let shell_and_pic = json!({"type":"user","timestamp":T0,"message":{"content":[
        block(&pic), {"type":"text","text":"<bash-input>ls</bash-input>"}]}})
    .to_string();
    let mut lines = vec![
        user_text("first"),
        shell_and_pic,
        result_with("out", std::slice::from_ref(&pic)),
        user_with(None, std::slice::from_ref(&pic)),
        user_with(Some("with text"), std::slice::from_ref(&pic)),
        user_text("last"),
    ];
    // A line with 12 tool calls and 12 picture results (the whole conversation then fits one page of 40 items).
    let calls: Vec<Value> = (0..12)
        .map(|i| json!({"type":"tool_use","name":"Bash","input":{"command":format!("c{i}")}}))
        .collect();
    lines.push(json!({"type":"assistant","message":{"content":calls}}).to_string());
    let results: Vec<Value> = (0..12)
        .map(
            |i| json!({"type":"tool_result","tool_use_id":format!("t{i}"),"content":[block(&pic)]}),
        )
        .collect();
    lines.push(json!({"type":"user","message":{"content":results}}).to_string());
    lines.push(user_text("end"));
    let p = write(&d, "a.jsonl", &lines);
    let key = |r: &Value| -> Vec<(String, String, String)> {
        r["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["id"].as_str().unwrap().to_string(),
                    i["role"].as_str().unwrap().to_string(),
                    i["text"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let on = key(&rd(&p, "", None, true));
    let off = key(&rd(&p, "", None, false));
    assert!(
        on.len() > off.len(),
        "the picture-only items are the difference"
    );
    for it in &off {
        assert!(
            on.contains(it),
            "{it:?} is there with sharing on, identically"
        );
    }
    // The extra ones sit in 20..40 and the others below 20.
    for (id, _, text) in &on {
        let idx: u32 = id.split('.').nth(1).unwrap().parse().unwrap();
        assert_eq!(
            idx >= 20,
            text.is_empty() && !off.iter().any(|o| &o.0 == id),
            "{id}"
        );
    }
    let mut ids: Vec<&String> = on.iter().map(|i| &i.0).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), on.len(), "unique");
    assert!(
        on.iter()
            .any(|(id, _, t)| id.ends_with(".0") && t == "Bash: ls"),
        "Bash keeps .0"
    );
}

#[test]
fn codex_lists_what_the_user_attached_and_what_a_tool_returned() {
    let p = b64(&png(70));
    let j = b64(&bytes_of(&JPG, 55, 9));
    let user_msg = json!({"type":"response_item","timestamp":T0,"payload":{
        "type":"message","role":"user","content":[
            {"type":"input_text","text":"see this"},
            {"type":"input_image","image_url":format!("data:image/png;base64,{p}")},
            {"type":"input_image","image_url":{"url":format!("data:image/jpeg;base64,{j}")}},
            {"type":"input_image","image_url":"https://example.test/a.png"},
            {"type":"input_image","image_url":format!("data:image/png;base64{p}")}]}});
    let it = codex_items(&user_msg, true);
    assert_eq!(it.len(), 1);
    assert_eq!(it[0].text, "see this");
    let kinds: Vec<&str> = it[0].images.iter().map(|i| i.kind).collect();
    assert_eq!(kinds, ["image/png", "image/jpeg"]);
    let off = codex_items(&user_msg, false);
    assert_eq!((off.len(), off[0].images.len()), (1, 0));
    // Only pictures: an item with an empty text.
    let only = json!({"type":"response_item","payload":{"type":"message","role":"user","content":[
        {"type":"input_image","image_url":format!("data:image/png;base64,{p}")}]}});
    let it = codex_items(&only, true);
    assert_eq!((it.len(), it[0].text.as_str()), (1, ""));
    assert!(codex_items(&only, false).is_empty());
    // What the model wrote is not what the user attached.
    let assistant = json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[
        {"type":"output_text","text":"done"},
        {"type":"output_image","image_url":format!("data:image/png;base64,{p}")}]}});
    assert_eq!(codex_items(&assistant, true)[0].images.len(), 0);
    // A tool's output that is an array with a picture.
    for ty in ["function_call_output", "custom_tool_call_output"] {
        let out = json!({"type":"response_item","payload":{"type":ty,"call_id":"c",
            "output":[{"type":"input_text","text":"SECRET"},
                      {"type":"input_image","image_url":format!("data:image/png;base64,{p}")}]}});
        let it = codex_items(&out, true);
        assert_eq!(
            (it.len(), it[0].role, it[0].text.as_str()),
            (1, "tool", ""),
            "{ty}"
        );
        assert!(!format!("{it:?}").contains("SECRET"));
        assert!(codex_items(&out, false).is_empty());
    }
    // A string output has no picture.
    let s =
        json!({"type":"response_item","payload":{"type":"function_call_output","output":"text"}});
    assert!(codex_items(&s, true).is_empty());
}

#[test]
fn the_reply_lists_descriptors_with_ids_that_cannot_meet_item_ids() {
    let d = TempDir::new().unwrap();
    let a = user_text("first");
    let b = user_with(Some("pic"), &[b64(&png(100)), b64(&bytes_of(&JPG, 60, 3))]);
    let c = result_with("out", &[b64(&bytes_of(b"GIF87a", 33, 1))]);
    let p = write(&d, "a.jsonl", &[a.clone(), b, c]);
    let off_b = a.len() as u64 + 1;
    let r = rd(&p, "", None, true);
    let it = r["items"].as_array().unwrap();
    assert_eq!(it.len(), 3);
    assert!(
        it[0].get("images").is_none(),
        "no field when nothing is listed"
    );
    assert_eq!(it[1]["id"], format!("{off_b}.0"));
    assert_eq!(
        it[1]["images"],
        json!([{"id": format!("{off_b}.100"), "type":"image/png","size":100},
               {"id": format!("{off_b}.101"), "type":"image/jpeg","size":60}])
    );
    assert_eq!(it[2]["role"], "tool");
    assert_eq!(it[2]["images"][0]["type"], "image/gif");
    // Every id is unique and the checker (clip) takes the whole reply.
    let mut ids: Vec<String> = it
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    ids.extend(
        all_images(&r)
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string()),
    );
    let n_ids = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n_ids);
    assert!(crate::remote::clip::reply_data(crate::remote::proto::Op::Chat, &r).is_some());
    // Off: the same file, no `images` anywhere and the empty-text item is gone.
    let off = rd(&p, "", None, false);
    assert!(all_images(&off).is_empty());
    assert_eq!(texts(&off), ["first", "pic"]);
}

// --------------------------------------------------------------- big lines

/// `user("")`-shaped line of exactly `len` bytes.
fn user_of_len(len: usize) -> String {
    let base = user_text("").len();
    // Words, not one long run: the redactor treats an unbroken run as a key.
    let pad = len - base;
    let words = "word ".repeat(pad / 5 + 1);
    user_text(&words[..pad])
}

#[test]
fn a_big_line_up_to_24_mib_is_parsed_and_one_byte_more_is_not() {
    let d = TempDir::new().unwrap();
    let exact = user_of_len(IMAGE_LINE_MAX as usize);
    let over = user_of_len(IMAGE_LINE_MAX as usize + 1);
    assert_eq!(exact.len() as u64, IMAGE_LINE_MAX);
    let p = write(
        &d,
        "a.jsonl",
        &[user_text("before"), exact, over, user_text("after")],
    );
    // Sharing off: the text of the big line is back (a bug fix).
    let r = rd(&p, "", None, false);
    let conv = r["conv"].as_str().unwrap().to_string();
    // Backward from the end, over the line that is one byte too long (which
    // costs the request most of its page budget, so it takes several calls).
    let mut t = texts(&r);
    let mut start = n(&r, "start");
    let mut older = r["older"] == true;
    for _ in 0..20 {
        if !older {
            break;
        }
        let r = rd(&p, &conv, Some(ChatPos::Before(start)), false);
        assert!(n(&r, "start") < start, "progress");
        let mut more = texts(&r);
        more.append(&mut t);
        t = more;
        start = n(&r, "start");
        older = r["older"] == true;
    }
    assert!(!older);
    assert_eq!(
        t.len(),
        3,
        "{:?}",
        t.iter().map(String::len).collect::<Vec<_>>()
    );
    assert_eq!((t[0].as_str(), t[2].as_str()), ("before", "after"));
    assert!(
        t[1].starts_with("word word") && t[1].ends_with('…'),
        "the 24 MiB line's text is back"
    );
    // Forward from the start gets the same three, whichever way it is split.
    let mut e = 0;
    let mut got = Vec::new();
    for _ in 0..40 {
        let r = rd(&p, &conv, Some(ChatPos::After(e)), false);
        assert_eq!(n(&r, "start"), e);
        got.extend(texts(&r));
        e = n(&r, "end");
        if r["newer"] == false {
            break;
        }
    }
    assert_eq!(got.len(), 3);
}

#[test]
fn a_big_picture_line_lists_its_picture_and_keeps_its_text() {
    let d = TempDir::new().unwrap();
    let big = png(5 * 1024 * 1024);
    let line = user_with(Some("see"), &[b64(&big)]);
    assert!(line.len() as u64 > MAX_LINE);
    let p = write(
        &d,
        "a.jsonl",
        &[user_text("before"), line, user_text("after")],
    );
    let on = rd(&p, "", None, true);
    assert_eq!(texts(&on), ["before", "see", "after"]);
    let imgs = all_images(&on);
    assert_eq!(imgs.len(), 1);
    assert_eq!(
        (imgs[0]["type"].as_str(), imgs[0]["size"].as_u64()),
        (Some("image/png"), Some(big.len() as u64))
    );
    let off = rd(&p, "", None, false);
    assert_eq!(texts(&off), ["before", "see", "after"]);
    assert!(all_images(&off).is_empty());
}

#[test]
fn big_lines_are_paid_from_their_own_pool_and_a_page_stops_at_a_line_start() {
    let d = TempDir::new().unwrap();
    // Four 20 MiB messages: each is 18 MiB past MAX_LINE, so two fit the
    // 48 MiB pool and the third waits for the next request.
    let big = |i: usize| user_text(&format!("n{i} {}", "word ".repeat(4 * 1024 * 1024)));
    let lines: Vec<String> = (0..4).map(big).collect();
    let starts: Vec<u64> = lines
        .iter()
        .scan(0u64, |at, l| {
            let s = *at;
            *at += l.len() as u64 + 1;
            Some(s)
        })
        .collect();
    let p = write(&d, "a.jsonl", &lines);
    let mut src = src_of(&p);
    let lc = src.len;
    let page = src
        .page_fwd(
            Rd {
                agent: AgentKind::Claude,
                images: false,
                big: true,
            },
            0,
            lc,
            false,
        )
        .unwrap();
    assert!(src.big_used <= BIG_BUDGET, "big pool {}", src.big_used);
    assert!(src.used <= MAX_READ, "page pool {}", src.used);
    assert_eq!(page.lines.len(), 2, "two big lines fit one request");
    assert_eq!(
        (page.end, page.newer),
        (starts[2], true),
        "stopped at a line start"
    );
    // Backward from the end: the same, mirrored.
    let mut src = src_of(&p);
    let page = src
        .page_back(
            Rd {
                agent: AgentKind::Claude,
                images: false,
                big: true,
            },
            lc,
            false,
        )
        .unwrap();
    assert!(src.big_used <= BIG_BUDGET && src.used <= MAX_READ);
    assert_eq!(page.lines.len(), 2);
    assert_eq!((page.start, page.older), (starts[2], true));
    // And the conversation is still reachable whole, both ways, with progress.
    let r0 = rd(&p, "", None, false);
    let conv = r0["conv"].as_str().unwrap().to_string();
    let mut all = texts(&r0);
    let mut start = n(&r0, "start");
    let mut calls = 0;
    while start > 0 {
        calls += 1;
        assert!(calls < 10);
        let r = rd(&p, &conv, Some(ChatPos::Before(start)), false);
        assert_eq!(n(&r, "end"), start);
        assert!(n(&r, "start") < start, "progress");
        let mut t = texts(&r);
        t.append(&mut all);
        all = t;
        start = n(&r, "start");
    }
    let heads: Vec<&str> = all.iter().map(|t| &t[..2]).collect();
    assert_eq!(heads, ["n0", "n1", "n2", "n3"]);
    let (mut e, mut fwd, mut calls) = (0, Vec::new(), 0);
    loop {
        calls += 1;
        assert!(calls < 10);
        let r = rd(&p, &conv, Some(ChatPos::After(e)), false);
        assert_eq!(n(&r, "start"), e);
        fwd.extend(texts(&r).iter().map(|t| t[..2].to_string()));
        assert!(n(&r, "end") > e || r["newer"] == false);
        e = n(&r, "end");
        if r["newer"] == false {
            break;
        }
    }
    assert_eq!(fwd, ["n0", "n1", "n2", "n3"]);
    assert!(calls >= 2);
}

#[test]
fn the_first_big_line_of_a_request_is_always_allowed() {
    let d = TempDir::new().unwrap();
    let line = user_of_len(23 * 1024 * 1024);
    let p = write(&d, "a.jsonl", &[line.clone(), user_text("next")]);
    let r0 = rd(&p, "", None, false);
    let conv = r0["conv"].as_str().unwrap().to_string();
    let r = rd(&p, &conv, Some(ChatPos::After(0)), false);
    assert_eq!(
        r["items"].as_array().unwrap().len(),
        2,
        "the big one and the next"
    );
    assert_eq!(r["newer"], false);
    // As the first thing of a page after a line before it.
    let r = rd(
        &p,
        &conv,
        Some(ChatPos::Before(line.len() as u64 + 1)),
        false,
    );
    assert_eq!(texts(&r).len(), 1);
    assert_eq!(n(&r, "start"), 0);
}

#[test]
fn a_normal_session_never_comes_near_the_work_budget() {
    use crate::remote::engine::{WorkBudget, WORK_MAX_BYTES};
    let d = TempDir::new().unwrap();
    // Several big picture lines (3 MiB of picture each), and a 400 KiB picture
    // (4 parts) at the end.
    let mut lines = vec![user_text("start")];
    lines.extend((0..4u8).map(|i| {
        user_with(
            Some(&format!("shot {i}")),
            &[b64(&bytes_of(&PNG, 3 * 1024 * 1024, i))],
        )
    }));
    lines.push(user_with(
        Some("last"),
        &[
            b64(&bytes_of(&PNG, 300 * 1024, 9)),
            b64(&bytes_of(&PNG, 3 * 1024 * 1024, 8)),
        ],
    ));
    let p = write(&d, "a.jsonl", &lines);
    let mut budget = WorkBudget::default();
    let mut now = 1_000u64;
    let mut total = 0u64;
    let mut call = |work: u64, budget: &mut WorkBudget, now: &mut u64| {
        assert!(
            budget.room(*now, "phone"),
            "never busy at {now} after {total}"
        );
        budget.charge(*now, "phone", work);
        total += work;
        *now += 100;
    };
    // The newest page, with pictures listed.
    let mut work = 0;
    let first = super::read_opened(
        Rd {
            agent: AgentKind::Claude,
            images: true,
            big: true,
        },
        SID,
        opened(&p),
        "",
        None,
        &mut work,
    )
    .unwrap();
    assert!(work > 0, "it did read big lines");
    call(work, &mut budget, &mut now);
    let (conv, mut end) = (
        first["conv"].as_str().unwrap().to_string(),
        n(&first, "end"),
    );
    // Thirty polls from `end`: finished lines are never read again.
    for _ in 0..30 {
        let mut work = 0;
        let r = super::read_opened(
            Rd {
                agent: AgentKind::Claude,
                images: true,
                big: true,
            },
            SID,
            opened(&p),
            &conv,
            Some(ChatPos::After(end)),
            &mut work,
        )
        .unwrap();
        assert_eq!(work, 0, "an `after: end` poll reads nothing big");
        end = n(&r, "end");
        call(work, &mut budget, &mut now);
    }
    // One 4-part picture: one read of its line, the rest from the cache.
    let imgs = all_images(&first);
    let id = imgs[imgs.len() - 2]["id"].as_str().unwrap().to_string();
    let mut cache = PicCache::default();
    let mut parts_seen = 0;
    for k in 0..4 {
        let mut work = 0;
        let r = super::image_opened(
            AgentKind::Claude,
            SID,
            opened(&p),
            PANE,
            &conv,
            &id,
            k,
            true,
            &mut cache,
            &mut work,
        )
        .unwrap();
        parts_seen = n(&r, "parts");
        if k == 0 {
            assert!(work > 0, "the first part reads the big line");
        } else {
            assert_eq!(work, 0, "part {k} comes from the cache");
        }
        call(work, &mut budget, &mut now);
    }
    assert_eq!(parts_seen, 4, "300 KiB is 4 parts of 96 KiB");
    assert!(total < WORK_MAX_BYTES / 4, "{total} bytes of big-line work");
}

// ------------------------------------------------- the work budget, as a phone

/// A phone with its own clock, talking to the reader the way the app does: the
/// device's work budget decides `big`, what a request read is charged to it,
/// and `Denied` / a busy image answer is a `busy`.
struct Phone {
    budget: crate::remote::engine::WorkBudget,
    now: u64,
    busy: usize,
    images: bool,
}

impl Phone {
    fn new(images: bool) -> Self {
        Phone {
            budget: Default::default(),
            now: 1_000,
            busy: 0,
            images,
        }
    }

    fn chat(&mut self, p: &Path, conv: &str, pos: Option<ChatPos>) -> Option<Value> {
        let big = self.budget.room(self.now, "phone");
        let mut work = 0;
        let rd = Rd {
            agent: AgentKind::Claude,
            images: self.images,
            big,
        };
        let r = super::read_opened(rd, SID, opened(p), conv, pos, &mut work);
        self.budget.charge(self.now, "phone", work);
        match r {
            Ok(v) => Some(v),
            Err(Fail::Denied) => {
                self.busy += 1;
                None
            }
            Err(e) => panic!("{e:?}"),
        }
    }

    fn image(
        &mut self,
        p: &Path,
        conv: &str,
        id: &str,
        part: u32,
        cache: &mut PicCache,
    ) -> Option<Value> {
        let big = self.budget.room(self.now, "phone");
        let mut work = 0;
        let r = super::image_opened(
            AgentKind::Claude,
            SID,
            opened(p),
            PANE,
            conv,
            id,
            part,
            big,
            cache,
            &mut work,
        )
        .unwrap();
        self.budget.charge(self.now, "phone", work);
        if super::is_busy(&r) {
            self.busy += 1;
            None
        } else {
            Some(r)
        }
    }

    fn wait(&mut self, ms: u64) {
        self.now += ms;
    }
}

#[test]
fn a_screenshot_heavy_walk_never_sees_busy() {
    // 40 lines of 2.5 MiB (a text and a picture) with a small line before each.
    let d = TempDir::new().unwrap();
    let mut lines = Vec::new();
    for i in 0..40u8 {
        lines.push(user_text(&format!("note {i}")));
        lines.push(user_with(
            Some(&format!("shot {i}")),
            &[b64(&bytes_of(&PNG, 2_500_000 * 3 / 4, i))],
        ));
    }
    let p = write(&d, "a.jsonl", &lines);
    for images in [false, true] {
        let mut ph = Phone::new(images);
        // The newest page.
        let first = ph.chat(&p, "", None).expect("newest page");
        let conv = first["conv"].as_str().unwrap().to_string();
        let end = n(&first, "end");
        ph.wait(250);
        // Thirty tail polls.
        for _ in 0..30 {
            let r = ph
                .chat(&p, &conv, Some(ChatPos::After(end)))
                .expect("tail poll");
            assert!(texts(&r).is_empty() && r["newer"] == false);
            ph.wait(250);
        }
        // Fifteen "load older" steps, four requests a second.
        let mut start = n(&first, "start");
        let mut got = texts(&first).len();
        for _ in 0..15 {
            if start == 0 {
                break;
            }
            let r = ph
                .chat(&p, &conv, Some(ChatPos::Before(start)))
                .expect("older");
            assert!(
                n(&r, "start") < start,
                "progress: {} -> {} ({} items, older {})",
                start,
                n(&r, "start"),
                texts(&r).len(),
                r["older"]
            );
            got += texts(&r).len();
            start = n(&r, "start");
            ph.wait(250);
        }
        assert!(got > 15, "{got}");
        // Reopen: the newest page again.
        let again = ph.chat(&p, "", None).expect("reopen");
        assert_eq!(texts(&again), texts(&first));
        ph.wait(250);
        // Eight pictures fetched whole (with pictures on).
        if images {
            // The newest pictures, found by paging back like the phone does.
            let mut imgs = all_images(&first);
            let mut cur = n(&first, "start");
            while imgs.len() < 8 && cur > 0 {
                let r = ph
                    .chat(&p, &conv, Some(ChatPos::Before(cur)))
                    .expect("older");
                cur = n(&r, "start");
                imgs.extend(all_images(&r));
                ph.wait(250);
            }
            assert!(imgs.len() >= 8, "{}", imgs.len());
            let mut cache = PicCache::default();
            for im in imgs.iter().take(8) {
                let id = im["id"].as_str().unwrap();
                let first_part = ph.image(&p, &conv, id, 0, &mut cache).expect("picture");
                for k in 1..n(&first_part, "parts") {
                    ph.image(&p, &conv, id, k as u32, &mut cache).expect("part");
                }
                ph.wait(250);
            }
        }
        assert_eq!(ph.busy, 0, "images {images}");
    }
}

#[test]
fn a_hostile_loop_over_big_lines_is_limited_and_recovers() {
    let d = TempDir::new().unwrap();
    let lines: Vec<String> = (0..6).map(|_| user_of_len(22 * 1024 * 1024)).collect();
    let p = write(&d, "a.jsonl", &lines);
    let mut ph = Phone::new(false);
    let mut ok = 0;
    let mut first_busy_at = None;
    // The newest page five times a second for four seconds.
    for i in 0..20 {
        if ph.chat(&p, "", None).is_some() {
            ok += 1;
        } else if first_busy_at.is_none() {
            first_busy_at = Some(i);
        }
        ph.wait(200);
    }
    assert!((5..20).contains(&ok), "{ok} served");
    assert!(first_busy_at.is_some() && ph.busy > 0, "it was limited");
    // Everything it read stayed within the budget plus the last request.
    assert!(
        ok as u64 * 2 * 22 * 1024 * 1024
            <= crate::remote::engine::WORK_MAX_BYTES + 2 * 22 * 1024 * 1024 + 1
    );
    // After the window it works again.
    ph.wait(crate::remote::engine::WORK_WINDOW_MS + 1);
    assert!(ph.chat(&p, "", None).is_some());
}

#[test]
fn a_budget_stop_is_truthful_and_nothing_is_skipped() {
    let d = TempDir::new().unwrap();
    let big = user_of_len(3 * 1024 * 1024);
    let lines = vec![
        user_text("s0"),
        user_text("s1"),
        big,
        user_text("s3"),
        user_text("s4"),
    ];
    let p = write(&d, "a.jsonl", &lines);
    let rd = |big_ok: bool, conv: &str, pos: Option<ChatPos>| {
        super::read_opened(
            Rd {
                agent: AgentKind::Claude,
                images: false,
                big: big_ok,
            },
            SID,
            opened(&p),
            conv,
            pos,
            &mut 0,
        )
    };
    let all = rd(true, "", None).unwrap();
    let conv = all["conv"].as_str().unwrap().to_string();
    let big_start = (lines[0].len() + lines[1].len() + 2) as u64;
    let after_big = big_start + lines[2].len() as u64 + 1;
    // Forward, no big lines: s0 and s1, `newer`, the cursor in front of the big line.
    let r = rd(false, &conv, Some(ChatPos::After(0))).unwrap();
    assert_eq!(texts(&r), ["s0", "s1"]);
    assert_eq!(
        (n(&r, "start"), n(&r, "end"), r["newer"].clone()),
        (0, big_start, json!(true))
    );
    // From there the very first line needed is big: no progress possible.
    assert!(matches!(
        rd(false, &conv, Some(ChatPos::After(big_start))),
        Err(Fail::Denied)
    ));
    // With big lines allowed the rest arrives and nothing was skipped.
    let r = rd(true, &conv, Some(ChatPos::After(big_start))).unwrap();
    let t = texts(&r);
    assert_eq!((t.len(), t[1].as_str(), t[2].as_str()), (3, "s3", "s4"));
    assert_eq!(r["newer"], false);
    // Backward, no big lines: s4 and s3, `older`, the cursor in front of them.
    let r = rd(false, "", None).unwrap();
    assert_eq!(texts(&r), ["s3", "s4"]);
    assert_eq!(
        (n(&r, "start"), r["older"].clone()),
        (after_big, json!(true))
    );
    assert!(matches!(
        rd(false, &conv, Some(ChatPos::Before(after_big))),
        Err(Fail::Denied)
    ));
    let r = rd(true, &conv, Some(ChatPos::Before(after_big))).unwrap();
    let t = texts(&r);
    assert_eq!((t.len(), t[0].as_str(), t[1].as_str()), (3, "s0", "s1"));
    assert!(t[2].starts_with("word word"), "the big line's text is back");
    assert_eq!(r["older"], false);
    // A tail poll with nothing new never needs a big line.
    let end = n(&all, "end");
    let r = rd(false, &conv, Some(ChatPos::After(end))).unwrap();
    assert!(texts(&r).is_empty() && r["newer"] == false);
    // A line of exactly MAX_LINE is not big.
    let exact = user_of_len(MAX_LINE as usize);
    let p2 = write(&d, "b.jsonl", &[user_text("a"), exact, user_text("c")]);
    let r = super::read_opened(
        Rd {
            agent: AgentKind::Claude,
            images: false,
            big: false,
        },
        SID,
        opened(&p2),
        "",
        None,
        &mut 0,
    )
    .unwrap();
    assert_eq!(texts(&r).len(), 3, "all three, no big line");
    let r = super::read_opened(
        Rd {
            agent: AgentKind::Claude,
            images: false,
            big: false,
        },
        SID,
        opened(&p2),
        r["conv"].as_str().unwrap(),
        Some(ChatPos::After(0)),
        &mut 0,
    )
    .unwrap();
    assert_eq!(texts(&r).len(), 3);
}

#[test]
fn a_big_line_is_charged_by_its_own_length_once() {
    let d = TempDir::new().unwrap();
    let small_pic = user_with(Some("tiny"), &[b64(&png(1_000))]);
    let five = user_with(Some("five"), &[b64(&bytes_of(&PNG, 5_000_000 * 3 / 4, 1))]);
    let p = write(
        &d,
        "a.jsonl",
        &[small_pic.clone(), five.clone(), user_text("x")],
    );
    let mut work = 0;
    super::read_opened(
        Rd {
            agent: AgentKind::Claude,
            images: true,
            big: true,
        },
        SID,
        opened(&p),
        "",
        None,
        &mut work,
    )
    .unwrap();
    assert_eq!(
        work,
        five.len() as u64,
        "exactly the 5 MB line, nothing for the small ones"
    );
    // A 1 KB picture costs nothing; a 5 MB one costs its line.
    let conv = rd(&p, "", None, true)["conv"].as_str().unwrap().to_string();
    let ids: Vec<String> = all_images(&rd(&p, "", None, true))
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    let mut cache = PicCache::default();
    let mut w = 0;
    super::image_opened(
        AgentKind::Claude,
        SID,
        opened(&p),
        PANE,
        &conv,
        &ids[0],
        0,
        true,
        &mut cache,
        &mut w,
    )
    .unwrap();
    assert_eq!(w, 0, "a small picture's line is not big");
    super::image_opened(
        AgentKind::Claude,
        SID,
        opened(&p),
        PANE,
        &conv,
        &ids[1],
        0,
        true,
        &mut cache,
        &mut w,
    )
    .unwrap();
    assert_eq!(w, five.len() as u64);
    // Over budget: a miss on the big line is busy, a hit is served, a small one is fine.
    let mut cache2 = PicCache::default();
    let busy = super::image_opened(
        AgentKind::Claude,
        SID,
        opened(&p),
        PANE,
        &conv,
        &ids[1],
        0,
        false,
        &mut cache2,
        &mut 0,
    )
    .unwrap();
    assert!(super::is_busy(&busy));
    let hit = super::image_opened(
        AgentKind::Claude,
        SID,
        opened(&p),
        PANE,
        &conv,
        &ids[1],
        1,
        false,
        &mut cache,
        &mut 0,
    )
    .unwrap();
    assert!(hit.get("data").is_some(), "a cache hit needs no read");
    let small = super::image_opened(
        AgentKind::Claude,
        SID,
        opened(&p),
        PANE,
        &conv,
        &ids[0],
        0,
        false,
        &mut cache2,
        &mut 0,
    )
    .unwrap();
    assert!(small.get("data").is_some());
}

#[test]
fn a_big_line_cut_off_by_page_overflow_is_charged_only_when_it_is_consumed() {
    let d = TempDir::new().unwrap();
    // 40 small items fill a page; the big line behind them does not fit.
    let mut lines: Vec<String> = (0..40).map(|i| user_text(&format!("m{i}"))).collect();
    lines.push(user_of_len(3 * 1024 * 1024));
    let p = write(&d, "a.jsonl", &lines);
    let rd = Rd {
        agent: AgentKind::Claude,
        images: false,
        big: true,
    };
    let mut work = 0;
    let r =
        super::read_opened(rd, SID, opened(&p), "", Some(ChatPos::After(0)), &mut work).unwrap();
    // (a conv of "" means the position is ignored: use the real one)
    let conv = r["conv"].as_str().unwrap().to_string();
    let mut work = 0;
    let fwd = super::read_opened(
        rd,
        SID,
        opened(&p),
        &conv,
        Some(ChatPos::After(0)),
        &mut work,
    )
    .unwrap();
    assert_eq!((texts(&fwd).len(), fwd["newer"].clone()), (40, json!(true)));
    assert_eq!(work, 0, "the big line was not part of the reply");
    // The next request consumes it and pays for it.
    let mut work = 0;
    super::read_opened(
        rd,
        SID,
        opened(&p),
        &conv,
        Some(ChatPos::After(n(&fwd, "end"))),
        &mut work,
    )
    .unwrap();
    assert_eq!(work, lines[40].len() as u64);
}

#[test]
fn a_cached_picture_is_not_served_after_its_bytes_left_the_file() {
    let d = TempDir::new().unwrap();
    let pics = [png(5_000), bytes_of(&PNG, 6_000, 2)];
    let p = write(
        &d,
        "a.jsonl",
        &[
            user_with(Some("a"), &[b64(&pics[0])]),
            user_with(Some("b"), &[b64(&pics[1])]),
        ],
    );
    let r = rd(&p, "", None, true);
    let conv = r["conv"].as_str().unwrap().to_string();
    let id = all_images(&r)[0]["id"].as_str().unwrap().to_string();
    let mut cache = PicCache::default();
    assert!(fetch(&p, &conv, &id, 0, &mut cache)
        .unwrap()
        .get("data")
        .is_some());
    assert_eq!(cache.len(), 1);
    // The file is cut short (same inode, same conversation): the entry goes.
    let orig = std::fs::read(&p).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&p)
        .unwrap()
        .set_len(100)
        .unwrap();
    assert_eq!(fetch(&p, &conv, &id, 0, &mut cache), gone());
    assert!(cache.is_empty(), "dropped, not served");
    // Rewritten in place with the same length but another line layout.
    std::fs::write(&p, &orig).unwrap();
    assert!(fetch(&p, &conv, &id, 0, &mut cache)
        .unwrap()
        .get("data")
        .is_some());
    let mut moved = orig.clone();
    let nl = moved.iter().position(|c| *c == b'\n').unwrap();
    moved[nl] = b' ';
    let last = moved.len() - 2;
    moved[last] = b'\n';
    std::fs::write(&p, &moved).unwrap();
    // The line at the offset does not end where it did: the entry goes, and
    // the merged line is not a picture line.
    assert_eq!(fetch(&p, &conv, &id, 0, &mut cache), gone());
    assert!(cache.is_empty());
}

#[test]
fn a_big_line_with_hundreds_of_thousands_of_values_is_skipped() {
    let junk = |n: usize| format!("[{}0]", "{\"a\":1},".repeat(n));
    let with = |n: usize| {
        format!(
            r#"{{"type":"user","message":{{"content":"hi"}},"junk":{}}}"#,
            junk(n)
        )
    };
    // Under 2 MiB the usual cap applies (190k objects = 380k values): parsed.
    let small = with(190_000);
    assert!((small.len() as u64) < MAX_LINE, "{}", small.len());
    assert_eq!(items(&small, false).len(), 1);
    // Over 2 MiB the cap is 200k values: 750k objects is nothing.
    let big = with(750_000);
    assert!(big.len() > 5_000_000 && (big.len() as u64) < IMAGE_LINE_MAX);
    assert!(items(&big, false).is_empty());
    assert!(items(&big, true).is_empty());
    // A big line of the same size with a picture and few values is fine.
    let line = user_with(Some("see"), &[b64(&png(3 * 1024 * 1024))]);
    assert!(line.len() as u64 > MAX_LINE);
    assert_eq!(items(&line, true).len(), 1);
}

#[test]
fn hostile_big_lines_yield_nothing_and_do_not_panic() {
    let d = TempDir::new().unwrap();
    let wide = format!("[{}0]", "1,".repeat(9 * 1024 * 1024));
    let deep = "{\"a\":".repeat(1_000_000);
    let nulls = "\0".repeat(5 * 1024 * 1024);
    let p = write(&d, "a.jsonl", &[wide, deep, nulls, user_text("ok")]);
    for images in [false, true] {
        let r = rd(&p, "", None, images);
        assert_eq!(texts(&r), ["ok"]);
        assert_eq!(r["older"], false);
    }
}

// ------------------------------------------------------------ fuzz-ish input

#[test]
fn damaged_and_random_lines_never_panic() {
    let p = b64(&png(300));
    let good = user_with(Some("t"), std::slice::from_ref(&p));
    for images in [false, true] {
        // Every prefix of a good line (a truncated write).
        for cut in (0..good.len()).step_by(7) {
            let _ = super::line_items(AgentKind::Claude, &good.as_bytes()[..cut], images);
        }
        // Pictures with every kind of bad data.
        for bad in [
            "",
            "=",
            "====",
            "A",
            "AAAAAAAAAAAAAAAAAAAAAAAAA",
            "iVBORw0KGgo",
            "iVBORw0KGgo=",
            "\u{1F600}",
            "iVBO\"Rw0K",
            "iVBORw0KGgoAAAAN%%%",
        ] {
            let _ = items(&user_with(Some("t"), &[bad.to_string()]), images);
            let url = json!({"type":"response_item","payload":{"type":"message","role":"user",
                "content":[{"type":"input_image","image_url":format!("data:image/png;base64,{bad}")},
                           {"type":"input_image","image_url":format!("data:{bad}")}]}});
            let _ = codex_items(&url, images);
        }
        // Random bytes.
        let mut x = 12345u32;
        for len in [0usize, 1, 2, 5, 40, 500, 5000] {
            for _ in 0..40 {
                let line: Vec<u8> = (0..len)
                    .map(|_| {
                        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (x >> 24) as u8
                    })
                    .collect();
                for agent in [AgentKind::Claude, AgentKind::Codex] {
                    let _ = super::line_items(agent, &line, images);
                }
            }
        }
    }
    // Garbage that looks like JSON all the way.
    for s in ["{", "[", "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"image\",\"source\":{\"type\":\"base64\",\"data\":\"iVBORw0KGgo", "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"image\",\"source\":\"x\"}]}}", "null", "7", "\"s\""] {
        assert!(items(s, true).is_empty(), "{s}");
    }
}

// -------------------------------------------------------------- image op

/// A transcript with a small picture, a 250 000-byte one (3 parts) and a tool
/// picture; returns the path, the picture bytes and the `chat` reply listing it.
fn transcript(d: &TempDir) -> (PathBuf, Vec<Vec<u8>>, Value) {
    let pics = vec![
        png(300),
        bytes_of(&JPG, 250_000, 7),
        bytes_of(b"GIF89a", 120_000, 8),
    ];
    let lines = vec![
        user_text("one"),
        user_with(Some("two"), &[b64(&pics[0]), b64(&pics[1])]),
        result_with("out", &[b64(&pics[2])]),
        user_text("four"),
    ];
    let p = write(d, "a.jsonl", &lines);
    let r = rd(&p, "", None, true);
    (p, pics, r)
}

fn fetch_all(p: &Path, conv: &str, id: &str, cache: &mut PicCache) -> (String, Vec<u8>, u64) {
    let first = fetch(p, conv, id, 0, cache).unwrap();
    let (parts, size, kind) = (
        n(&first, "parts"),
        n(&first, "size"),
        first["type"].as_str().unwrap().to_string(),
    );
    let mut data = Vec::new();
    for k in 0..parts {
        let r = fetch(p, conv, id, u32::try_from(k).unwrap(), cache).unwrap();
        assert_eq!(
            (
                r["conv"].as_str(),
                r["id"].as_str(),
                n(&r, "size"),
                n(&r, "parts")
            ),
            (Some(conv), Some(id), size, parts)
        );
        assert_eq!(r["type"], kind.as_str());
        let part = crate::remote::keys::b64u_decode(r["data"].as_str().unwrap()).unwrap();
        assert!(part.len() <= PART_BYTES);
        if k + 1 < parts {
            assert_eq!(part.len(), PART_BYTES, "every part but the last is 96 KiB");
        }
        data.extend(part);
    }
    assert_eq!(data.len() as u64, size);
    (kind, data, parts)
}

#[test]
fn every_listed_picture_is_fetched_whole_in_parts() {
    let d = TempDir::new().unwrap();
    let (p, pics, r) = transcript(&d);
    let conv = r["conv"].as_str().unwrap();
    let imgs = all_images(&r);
    assert_eq!(imgs.len(), 3);
    let mut cache = PicCache::default();
    let want = [("image/png", 1u64), ("image/jpeg", 3), ("image/gif", 2)];
    for (i, im) in imgs.iter().enumerate() {
        let (kind, data, parts) = fetch_all(&p, conv, im["id"].as_str().unwrap(), &mut cache);
        assert_eq!((kind.as_str(), parts), (want[i].0, want[i].1), "{i}");
        assert_eq!(data, pics[i], "{i}: the same bytes");
        assert_eq!(n(im, "size"), pics[i].len() as u64);
    }
}

#[test]
fn a_picture_is_read_once_for_all_its_parts() {
    let d = TempDir::new().unwrap();
    let (p, pics, r) = transcript(&d);
    let conv = r["conv"].as_str().unwrap();
    let id = all_images(&r)[1]["id"].as_str().unwrap().to_string();
    let mut cache = PicCache::default();
    let first = fetch(&p, conv, &id, 0, &mut cache).unwrap();
    assert_eq!(n(&first, "parts"), 3);
    assert_eq!(cache.len(), 1);
    // Overwrite the picture in place (same length, same file identity): parts
    // that come from the cache still carry the original bytes.
    let mut raw = std::fs::read(&p).unwrap();
    let text = b64(&pics[1]);
    let at = raw
        .windows(200)
        .position(|w| w == &text.as_bytes()[..200])
        .unwrap();
    raw[at..at + text.len()].fill(b'A');
    std::fs::write(&p, &raw).unwrap();
    let again = rd(&p, "", None, true);
    assert_eq!(again["conv"], conv, "same file identity, same conversation");
    let (_, data, _) = {
        let k2 = fetch(&p, conv, &id, 2, &mut cache).unwrap();
        let k1 = fetch(&p, conv, &id, 1, &mut cache).unwrap();
        let parts: Vec<u8> = [fetch(&p, conv, &id, 0, &mut cache).unwrap(), k1, k2]
            .iter()
            .flat_map(|r| crate::remote::keys::b64u_decode(r["data"].as_str().unwrap()).unwrap())
            .collect();
        (0, parts, 0)
    };
    assert_eq!(data, pics[1]);
    // Without the cache the damaged file is read, and fails the checks.
    cache.clear();
    assert_eq!(fetch(&p, conv, &id, 0, &mut cache), gone());
    assert!(cache.is_empty());
}

#[test]
fn another_conversation_or_a_bad_id_is_gone() {
    let d = TempDir::new().unwrap();
    let (p, _, r) = transcript(&d);
    let conv = r["conv"].as_str().unwrap();
    let id = all_images(&r)[0]["id"].as_str().unwrap().to_string();
    let off: u64 = id.split('.').next().unwrap().parse().unwrap();
    let mut cache = PicCache::default();
    assert!(fetch(&p, conv, &id, 0, &mut cache)
        .unwrap()
        .get("data")
        .is_some());
    // Not the current conversation (or none at all).
    assert_eq!(fetch(&p, "other", &id, 0, &mut cache), gone());
    assert_eq!(fetch(&p, "", &id, 0, &mut cache), gone());
    // The pane moves to another conversation file (another path, so another
    // conv): what was kept for the old one is dropped, even for a request that
    // names the old conv.
    assert_eq!(cache.len(), 1);
    let p2 = d.path().join("b.jsonl");
    std::fs::copy(&p, &p2).unwrap();
    assert_eq!(fetch(&p2, conv, &id, 0, &mut cache), gone());
    assert_eq!(
        cache.len(),
        0,
        "the pane's other-conversation pictures are dropped"
    );
    assert!(fetch(&p, conv, &id, 0, &mut cache)
        .unwrap()
        .get("data")
        .is_some());
    for bad in [
        "".to_string(),
        "x".into(),
        "5".into(),
        format!("{off}.0"),
        format!("{off}.99"),
        format!("{off}.102"),
        format!("{off}.420"),
        format!("{}.100", off + 1),
        format!("{}.100", off + 5),
        format!("{}.100", off.saturating_sub(1)),
        "0.100".into(),
        "999999999999.100".into(),
        format!("{off}.100.1"),
        format!("-{off}.100"),
        "99999999999999999999999999.100".into(),
    ] {
        assert_eq!(fetch(&p, conv, &bad, 0, &mut cache), gone(), "{bad:?}");
    }
    // Parts out of range, at the edge and far beyond.
    assert!(fetch(&p, conv, &id, 0, &mut cache)
        .unwrap()
        .get("data")
        .is_some());
    for part in [1u32, 2, 255, 256, u32::MAX] {
        assert_eq!(fetch(&p, conv, &id, part, &mut cache), gone(), "{part}");
    }
}

#[test]
fn ids_pointing_at_non_pictures_or_unfit_lines_are_gone() {
    let d = TempDir::new().unwrap();
    let svg = b64(b"<svg xmlns='http://www.w3.org/2000/svg'/>");
    let mut broken = b64(&png(5000));
    broken.replace_range(300..304, "!!!!");
    let side: String = {
        let mut v: Value = serde_json::from_str(&user_with(Some("s"), &[b64(&png(64))])).unwrap();
        v["isSidechain"] = json!(true);
        v.to_string()
    };
    let lines = vec![
        user_with(Some("svg"), &[svg]),
        user_with(Some("broken"), &[broken]),
        side,
        user_text("plain"),
        user_with(Some("ok"), &[b64(&png(64))]),
    ];
    let p = write(&d, "a.jsonl", &lines);
    let r = rd(&p, "", None, true);
    let conv = r["conv"].as_str().unwrap();
    // The reader lists the broken one (only its head was looked at), not the SVG.
    let imgs = all_images(&r);
    assert_eq!(imgs.len(), 2, "{imgs:?}");
    let mut cache = PicCache::default();
    let mut at = 0u64;
    let mut starts = Vec::new();
    for l in &lines {
        starts.push(at);
        at += l.len() as u64 + 1;
    }
    // Whatever block number is asked at any line: only the valid one answers.
    let mut served = 0;
    for (i, s) in starts.iter().enumerate() {
        for ord in 100..110 {
            if let Some(v) = fetch(&p, conv, &format!("{s}.{ord}"), 0, &mut cache) {
                if v.get("data").is_some() {
                    served += 1;
                    assert_eq!(i, 4, "only the last line has a servable picture");
                } else {
                    assert_eq!(v, json!({"result":"gone"}));
                }
            }
        }
    }
    assert_eq!(served, 1);
    // The broken picture is listed but is `gone` when asked for.
    let broken_id = imgs[0]["id"].as_str().unwrap();
    assert_eq!(fetch(&p, conv, broken_id, 0, &mut cache), gone());
}

#[test]
fn an_unfinished_line_and_a_too_long_line_are_gone() {
    let d = TempDir::new().unwrap();
    let ok = user_with(Some("ok"), &[b64(&png(64))]);
    let p = write(&d, "a.jsonl", std::slice::from_ref(&ok));
    let conv0 = rd(&p, "", None, true)["conv"].as_str().unwrap().to_string();
    // A last line that has not got its newline yet, picture and all.
    let unfinished = user_with(Some("late"), &[b64(&png(64))]);
    let off = std::fs::metadata(&p).unwrap().len();
    let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    std::io::Write::write_all(&mut f, unfinished.as_bytes()).unwrap();
    let mut cache = PicCache::default();
    assert_eq!(
        fetch(&p, &conv0, &format!("{off}.100"), 0, &mut cache),
        gone()
    );
    // Once it is complete it is served.
    std::io::Write::write_all(&mut f, b"\n").unwrap();
    assert!(fetch(&p, &conv0, &format!("{off}.100"), 0, &mut cache)
        .unwrap()
        .get("data")
        .is_some());
    // A line over 24 MiB is never parsed, picture or not.
    let huge = user_with(
        Some("huge"),
        &[
            b64(&png(18 * 1024 * 1024)),
            b64(&png(8 * 1024 * 1024)),
            "A".repeat(5 * 1024 * 1024),
        ],
    );
    assert!(huge.len() as u64 > IMAGE_LINE_MAX, "{}", huge.len());
    let off2 = std::fs::metadata(&p).unwrap().len();
    std::io::Write::write_all(&mut f, format!("{huge}\n").as_bytes()).unwrap();
    assert_eq!(
        fetch(&p, &conv0, &format!("{off2}.100"), 0, &mut cache),
        gone()
    );
    let listed = rd(&p, "", None, true);
    assert!(!all_images(&listed)
        .iter()
        .any(|i| i["id"].as_str().unwrap().starts_with(&format!("{off2}."))));
}

#[test]
fn an_eight_mib_picture_comes_in_86_parts_and_one_byte_more_is_not_listed() {
    let d = TempDir::new().unwrap();
    let exact = png(IMAGE_MAX as usize);
    let over = png(IMAGE_MAX as usize + 1);
    let p = write(
        &d,
        "a.jsonl",
        &[
            user_with(Some("a"), &[b64(&exact)]),
            user_with(Some("b"), &[b64(&over)]),
        ],
    );
    let r = rd(&p, "", None, true);
    let imgs = all_images(&r);
    assert_eq!(imgs.len(), 1, "the 8 MiB + 1 byte one is not listed");
    assert_eq!(n(&imgs[0], "size"), IMAGE_MAX);
    let conv = r["conv"].as_str().unwrap();
    let mut cache = PicCache::default();
    let id = imgs[0]["id"].as_str().unwrap().to_string();
    let last = fetch(&p, conv, &id, 85, &mut cache).unwrap();
    assert_eq!((n(&last, "parts"), n(&last, "part")), (86, 85));
    assert_eq!(fetch(&p, conv, &id, 86, &mut cache), gone());
    assert!(cache.bytes() <= pics::CACHE_BYTES);
    // And the over-sized one is not fetchable by guessing its id either.
    let second = std::fs::read(&p)
        .unwrap()
        .iter()
        .position(|c| *c == b'\n')
        .unwrap() as u64
        + 1;
    assert_eq!(
        fetch(&p, conv, &format!("{second}.100"), 0, &mut cache),
        gone()
    );
}

#[test]
fn a_reply_part_fits_a_frame() {
    // 96 KiB -> base64url -> the reply object -> its JSON with the longest
    // envelope a sealed reply gets (session, n, at_ms) -> AES-GCM sealed ->
    // the `msg` frame the server relays.
    let data = bytes_of(&PNG, PART_BYTES * 2, 3);
    let r = pics::part_reply(
        "abcdefghijklmnopqrstuvwxyz012345",
        "99999999999999999.259",
        "image/png",
        &data,
        0,
    )
    .unwrap();
    let checked = crate::remote::clip::reply_data(crate::remote::proto::Op::Image, &r).unwrap();
    let mut body = crate::remote::proto::reply_ok(9_007_199_254_740_991, checked);
    let o = body.as_object_mut().unwrap();
    o.insert("session".into(), json!("s".repeat(64)));
    o.insert("n".into(), json!(u64::MAX));
    o.insert("at_ms".into(), json!(u64::MAX));
    let key = crate::remote::keys::LinkKey::from_bytes([3u8; 32]);
    let sealed = key.seal("aad", body.to_string().as_bytes()).unwrap();
    let frame = crate::remote::proto::msg_frame_str("device", &sealed);
    assert!(
        frame.len() < crate::remote::proto::MAX_FRAME * 7 / 10,
        "{} of {}",
        frame.len(),
        crate::remote::proto::MAX_FRAME
    );
}

#[test]
fn the_image_op_goes_through_the_same_vetting_as_chat() {
    let d = TempDir::new().unwrap();
    let cwd = d.path().join("proj").to_string_lossy().into_owned();
    let dir = d
        .path()
        .join(crate::agent_scan_disk::claude_project_dir_name(&cwd));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{SID}.jsonl"));
    let head =
        json!({"type":"user","cwd":cwd,"entrypoint":"cli","message":{"content":"hi"}}).to_string();
    let bytes = png(1000);
    std::fs::write(
        &file,
        format!("{head}\n{}\n", user_with(Some("p"), &[b64(&bytes)])),
    )
    .unwrap();
    let hook = Binding {
        agent: AgentKind::Claude,
        session_id: SID,
        record_cwd: &cwd,
        source: IdSource::Hook,
    };
    let disk = Binding {
        source: IdSource::Disk,
        ..Binding {
            agent: AgentKind::Claude,
            session_id: SID,
            record_cwd: &cwd,
            source: IdSource::Hook,
        }
    };
    let mut paths = None;
    let listed = super::answer(
        &hook,
        d.path(),
        "/x",
        "",
        None,
        true,
        true,
        &mut paths,
        &mut 0,
    )
    .unwrap();
    let conv = listed["conv"].as_str().unwrap().to_string();
    let id = all_images(&listed)[0]["id"].as_str().unwrap().to_string();
    let mut cache = PicCache::default();
    let ask = |b: &Binding<'_>,
               cwd: &str,
               conv: &str,
               id: &str,
               cache: &mut PicCache,
               paths: &mut Option<PathBuf>| {
        super::image(
            b,
            d.path(),
            cwd,
            PANE,
            conv,
            id,
            0,
            true,
            paths,
            cache,
            &mut 0,
        )
    };
    let ok = ask(&hook, "/x", &conv, &id, &mut cache, &mut paths).unwrap();
    assert_eq!(
        (n(&ok, "size"), ok["type"].as_str()),
        (1000, Some("image/png"))
    );
    // A disk-inferred binding in the wrong directory is refused like chat.
    cache.clear();
    assert_eq!(
        ask(&disk, "/elsewhere", &conv, &id, &mut cache, &mut paths),
        gone()
    );
    assert_eq!(ask(&disk, "", &conv, &id, &mut cache, &mut paths), gone());
    assert!(ask(&disk, &cwd, &conv, &id, &mut cache, &mut paths)
        .unwrap()
        .get("data")
        .is_some());
    // No transcript at all.
    assert_eq!(
        super::image(
            &hook,
            &d.path().join("absent"),
            "/x",
            PANE,
            &conv,
            &id,
            0,
            true,
            &mut None,
            &mut cache,
            &mut 0
        ),
        gone()
    );
    // A root that cannot be listed is a read error, not "gone".
    let plain = d.path().join("plain-file");
    std::fs::write(&plain, "x").unwrap();
    assert_eq!(
        super::image(&hook, &plain, "/x", PANE, &conv, &id, 0, true, &mut None, &mut cache, &mut 0),
        None
    );
}
