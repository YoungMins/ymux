//! The wire shapes of the remote protocol (version 1) and the strings that get
//! signed. Pure: no I/O, no clock. See `PROTOCOL.md` in the yMux-Server repo.
//!
//! Frames from the server are parsed leniently (unknown fields are ignored,
//! an unknown `t` becomes [`ServerFrame::Unknown`]); everything this side
//! sends is built from the helpers at the bottom so the outgoing JSON stays
//! strictly within the protocol's shapes.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const VERSION: u32 = 1;
/// Largest WebSocket frame either way.
pub const MAX_FRAME: usize = 256 * 1024;
/// Largest command body the desktop will look at (checked before the
/// signature, so a huge body costs nothing).
pub const MAX_BODY: usize = 64 * 1024;
/// Longest sealed body worth looking at: the base64 of a 64 KiB plaintext plus
/// nonce and tag, with a little slack. Longer is dropped before any crypto.
pub const MAX_SEALED: usize = (MAX_BODY + 28) / 3 * 4 + 8;
pub const MENU_ID_MAX: usize = 32;
/// A device clock and ours may differ by this much before a command is stale.
pub const TS_WINDOW_MS: u64 = 30_000;
pub const PROMPT_MAX_CHARS: usize = 16_000;
pub const LINES_MAX: u64 = 200;
pub const LINES_DEFAULT: u64 = 80;

// ------------------------------------------------------------ signed strings

/// What the desktop signs to log in:
/// `ymux-remote-v1|desktop|<id>|<origin>|<nonce>` (its own configured origin,
/// so a challenge passed through from another server does not verify there).
pub fn hello_signed(desktop_id: &str, origin: &str, nonce: &str) -> String {
    format!("ymux-remote-v1|desktop|{desktop_id}|{origin}|{nonce}")
}

/// The HKDF `info` of the link key (section 1). Every field is the string as
/// it travelled: the device's signing key, its `ek`, this window's `dk`.
pub fn link_info(desktop_id: &str, device_id: &str, pubkey: &str, ek: &str, dk: &str) -> String {
    format!("ymux-remote-v1|link|{desktop_id}|{device_id}|{pubkey}|{ek}|{dk}")
}

/// Additional data of a sealed body, device to desktop.
pub fn aad_p2d(desktop_id: &str, device_id: &str) -> String {
    format!("ymux-remote-v1|p2d|{desktop_id}|{device_id}")
}

/// Additional data of a sealed body, desktop to device.
pub fn aad_d2p(desktop_id: &str, device_id: &str) -> String {
    format!("ymux-remote-v1|d2p|{desktop_id}|{device_id}")
}

/// A menu id: 1 to 32 chars of `[A-Za-z0-9_-]`.
pub fn is_menu_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MENU_ID_MAX
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// What a device signs for a command: the prefix plus the body string.
pub fn cmd_signed(body: &str) -> Vec<u8> {
    let mut v = b"ymux-remote-v1|cmd|".to_vec();
    v.extend_from_slice(body.as_bytes());
    v
}

/// The message the pairing proof is an HMAC of.
pub fn pair_proof_msg(desktop_id: &str, device_id: &str, pubkey: &str, ek: &str) -> String {
    format!("ymux-remote-v1|pair|{desktop_id}|{device_id}|{pubkey}|{ek}")
}

/// A lowercase hyphenated UUID, the only spelling ids may have.
pub fn is_uuid(s: &str) -> bool {
    s.len() == 36 && uuid::Uuid::parse_str(s).is_ok() && s == s.to_ascii_lowercase()
}

// ------------------------------------------------------------- capabilities

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cap {
    View,
    Respond,
    Prompt,
}

impl Cap {
    pub fn parse(s: &str) -> Option<Cap> {
        match s {
            "view" => Some(Cap::View),
            "respond" => Some(Cap::Respond),
            "prompt" => Some(Cap::Prompt),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Cap::View => "view",
            Cap::Respond => "respond",
            Cap::Prompt => "prompt",
        }
    }
}

/// The five ops there are. Nothing else (raw keys, new panes, files) exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    List,
    Peek,
    Answer,
    Interrupt,
    Prompt,
}

impl Op {
    pub fn parse(s: &str) -> Option<Op> {
        match s {
            "list" => Some(Op::List),
            "peek" => Some(Op::Peek),
            "answer" => Some(Op::Answer),
            "interrupt" => Some(Op::Interrupt),
            "prompt" => Some(Op::Prompt),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Op::List => "list",
            Op::Peek => "peek",
            Op::Answer => "answer",
            Op::Interrupt => "interrupt",
            Op::Prompt => "prompt",
        }
    }

    /// The lowest capability that may run this op.
    pub fn needs(self) -> Cap {
        match self {
            Op::List | Op::Peek => Cap::View,
            Op::Answer | Op::Interrupt => Cap::Respond,
            Op::Prompt => Cap::Prompt,
        }
    }
}

// -------------------------------------------------------------------- codes

/// Why a command did not run (the `code` of a failed reply).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    BadSig,
    BadSession,
    Replay,
    /// `ts_ms` is not within 30 s of this desktop's clock.
    Expired,
    Forbidden,
    BadRequest,
    UnknownOp,
    Timeout,
    /// Too many commands waiting on the app at once.
    Busy,
    /// The app's answer did not fit the protocol.
    Internal,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::BadSig => "bad_sig",
            Code::BadSession => "bad_session",
            Code::Replay => "replay",
            Code::Expired => "expired",
            Code::Forbidden => "forbidden",
            Code::BadRequest => "bad_request",
            Code::UnknownOp => "unknown_op",
            Code::Timeout => "timeout",
            Code::Busy => "busy",
            Code::Internal => "internal",
        }
    }
}

// ----------------------------------------------------------- server frames

/// A text frame from the server. Missing required fields make the whole
/// frame fail to parse, which the caller treats as "ignore".
#[derive(Debug, Deserialize)]
#[serde(tag = "t")]
pub enum ServerFrame {
    #[serde(rename = "challenge")]
    Challenge {
        nonce: String,
        #[serde(default)]
        v: u32,
        /// The server's public origin; empty when an old server omits it.
        #[serde(default)]
        origin: String,
    },
    #[serde(rename = "ready")]
    Ready {
        #[serde(default)]
        devices: Vec<String>,
    },
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        code: String,
    },
    #[serde(rename = "device_online")]
    DeviceOnline { device_id: String, conn: String },
    #[serde(rename = "device_offline")]
    DeviceOffline { device_id: String, conn: String },
    #[serde(rename = "pair_request")]
    PairRequest {
        device_id: String,
        pubkey: String,
        /// The device's key-agreement key; empty from an old PWA (refused).
        #[serde(default)]
        ek: String,
        #[serde(default)]
        name: String,
        proof: String,
    },
    #[serde(rename = "cmd")]
    Cmd {
        device_id: String,
        conn: String,
        body: String,
        sig: String,
    },
    #[serde(other)]
    Unknown,
}

pub fn parse_server_frame(text: &str) -> Option<ServerFrame> {
    if text.len() > MAX_FRAME {
        return None;
    }
    serde_json::from_str(text).ok()
}

// ------------------------------------------------------------ command body

/// The envelope of a signed command body.
#[derive(Debug, Deserialize)]
pub struct CmdBody {
    #[serde(default)]
    pub k: String,
    pub desktop_id: String,
    pub device_id: String,
    pub session: String,
    pub seq: u64,
    /// The device's estimate of this desktop's clock, epoch ms.
    pub ts_ms: i64,
    pub cmd: Value,
}

/// The plaintext a device opens a session with.
#[derive(Debug, Deserialize)]
pub struct OpenBody {
    pub desktop_id: String,
    pub device_id: String,
    pub nonce: String,
}

/// The nonce a device picked for its `open`: 16 random bytes as `b64u`; the
/// desktop only echoes it, so any short URL-safe token is fine.
pub fn is_open_nonce(s: &str) -> bool {
    (8..=64).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A validated command, ready to hand to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    List,
    Peek {
        pane: String,
        lines: u64,
    },
    Answer {
        pane: String,
        n: u32,
        label: String,
        menu_id: String,
    },
    Interrupt {
        pane: String,
    },
    Prompt {
        pane: String,
        text: String,
        queue: bool,
    },
}

impl Request {
    pub fn op(&self) -> Op {
        match self {
            Request::List => Op::List,
            Request::Peek { .. } => Op::Peek,
            Request::Answer { .. } => Op::Answer,
            Request::Interrupt { .. } => Op::Interrupt,
            Request::Prompt { .. } => Op::Prompt,
        }
    }

    pub fn pane(&self) -> &str {
        match self {
            Request::List => "",
            Request::Peek { pane, .. }
            | Request::Answer { pane, .. }
            | Request::Interrupt { pane }
            | Request::Prompt { pane, .. } => pane,
        }
    }

    /// The `remote://request` event payload.
    pub fn event(&self, id: u64) -> Value {
        match self {
            Request::List => json!({"id": id, "op": "list"}),
            Request::Peek { pane, lines } => {
                json!({"id": id, "op": "peek", "pane": pane, "lines": lines})
            }
            Request::Answer {
                pane,
                n,
                label,
                menu_id,
            } => {
                json!({"id": id, "op": "answer", "pane": pane, "n": n, "label": label,
                       "menu_id": menu_id})
            }
            Request::Interrupt { pane } => json!({"id": id, "op": "interrupt", "pane": pane}),
            Request::Prompt { pane, text, queue } => {
                json!({"id": id, "op": "prompt", "pane": pane, "text": text, "queue": queue})
            }
        }
    }
}

/// The op name of a command value, for rejection logging only.
pub fn op_of(cmd: &Value) -> Option<Op> {
    cmd.get("op")?.as_str().and_then(Op::parse)
}

fn pane_field(cmd: &Value) -> Result<String, Code> {
    let p = cmd
        .get("pane")
        .and_then(Value::as_str)
        .ok_or(Code::BadRequest)?;
    if is_uuid(p) {
        Ok(p.to_string())
    } else {
        Err(Code::BadRequest)
    }
}

/// Validate the `cmd` object. `Err(UnknownOp)` for an op that does not exist,
/// `Err(BadRequest)` for a field that is missing or out of range.
pub fn parse_request(cmd: &Value) -> Result<Request, Code> {
    let obj = cmd.as_object().ok_or(Code::BadRequest)?;
    let op = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or(Code::BadRequest)?;
    let op = Op::parse(op).ok_or(Code::UnknownOp)?;
    match op {
        Op::List => Ok(Request::List),
        Op::Peek => {
            let lines = match obj.get("lines") {
                None | Some(Value::Null) => LINES_DEFAULT,
                Some(v) => v.as_u64().ok_or(Code::BadRequest)?,
            };
            if !(1..=LINES_MAX).contains(&lines) {
                return Err(Code::BadRequest);
            }
            Ok(Request::Peek {
                pane: pane_field(cmd)?,
                lines,
            })
        }
        Op::Answer => {
            let n = obj
                .get("n")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (1..=9).contains(n))
                .ok_or(Code::BadRequest)?;
            let label = obj
                .get("label")
                .and_then(Value::as_str)
                .ok_or(Code::BadRequest)?;
            if label.chars().count() > 400 {
                return Err(Code::BadRequest);
            }
            let menu_id = obj
                .get("menu_id")
                .and_then(Value::as_str)
                .filter(|m| is_menu_id(m))
                .ok_or(Code::BadRequest)?;
            Ok(Request::Answer {
                pane: pane_field(cmd)?,
                n,
                label: label.to_string(),
                menu_id: menu_id.to_string(),
            })
        }
        Op::Interrupt => Ok(Request::Interrupt {
            pane: pane_field(cmd)?,
        }),
        Op::Prompt => {
            let text = obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or(Code::BadRequest)?;
            let n = text.chars().count();
            if n == 0 || n > PROMPT_MAX_CHARS {
                return Err(Code::BadRequest);
            }
            // Controls out (CR and CRLF become a newline) before anything
            // else looks at the text; what is left may still be blank.
            let text = super::clip::clean_prompt(text);
            if text.trim().is_empty() {
                return Err(Code::BadRequest);
            }
            let queue = match obj.get("queue") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err(Code::BadRequest),
            };
            Ok(Request::Prompt {
                pane: pane_field(cmd)?,
                text,
                queue,
            })
        }
    }
}

// ------------------------------------------------------------ shared shapes

/// A pane as the phone sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pane {
    pub pane: String,
    pub ws: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub since_ms: u64,
    pub queued: u32,
}

// ---------------------------------------------------------- outgoing frames

pub fn hello_frame(
    desktop_id: &str,
    pubkey: &str,
    name: &str,
    setup_token: &str,
    sig: &str,
) -> String {
    let mut v = json!({
        "t": "hello", "role": "desktop", "v": VERSION,
        "desktop_id": desktop_id, "pubkey": pubkey, "name": name, "sig": sig,
    });
    if !setup_token.is_empty() {
        v["setup_token"] = Value::String(setup_token.to_string());
    }
    v.to_string()
}

pub fn pair_open_frame(ttl_s: u32) -> String {
    json!({"t": "pair_open", "ttl_s": ttl_s}).to_string()
}

pub fn pair_close_frame() -> String {
    json!({"t": "pair_close"}).to_string()
}

/// `ok:true` carries the key and name the server stores.
pub fn pair_result_frame(device_id: &str, approved: Option<(&str, &str)>) -> String {
    match approved {
        Some((pubkey, name)) => json!({
            "t": "pair_result", "device_id": device_id, "ok": true,
            "pubkey": pubkey, "name": name,
        }),
        None => json!({"t": "pair_result", "device_id": device_id, "ok": false}),
    }
    .to_string()
}

pub fn device_remove_frame(device_id: &str) -> String {
    json!({"t": "device_remove", "device_id": device_id}).to_string()
}

/// `to` is a device id; `body` is a sealed body, carried as a string.
pub fn msg_frame_str(to: &str, body: &str) -> String {
    json!({"t": "msg", "to": to, "body": body}).to_string()
}

pub fn notify_frame(kind: &str, title: &str, body: &str, pane: &str) -> String {
    json!({"t": "notify", "kind": kind, "title": title, "body": body, "pane": pane}).to_string()
}

/// What happened to the session a new one revoked: the device uses it to
/// settle commands whose outcome it does not know (section 5.1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prev {
    /// The revoked session, or empty when there was none.
    pub session: String,
    /// The last `seq` accepted in it, or 0.
    pub seq: u64,
}

/// The answer to a device's `open`: echoes its nonce and carries this
/// desktop's clock so the device can stamp commands.
pub fn session_body(
    open: &str,
    session: &str,
    cap: Cap,
    name: &str,
    now_ms: u64,
    prev: &Prev,
) -> Value {
    json!({"k": "session", "open": open, "session": session, "cap": cap.as_str(),
           "name": name, "now_ms": now_ms,
           "prev": {"session": prev.session, "seq": prev.seq}, "v": VERSION})
}

/// `pair_challenge`: the check number, sealed for the one device that can
/// derive the link key. The server forwards `body` to that pairing socket.
pub fn pair_challenge_frame(device_id: &str, sealed: &str) -> String {
    json!({"t": "pair_challenge", "device_id": device_id, "body": sealed}).to_string()
}

/// What a failed `re:0` reply carries so the device can tell which of its
/// commands it answers: the first 16 bytes of SHA-256 of the signature string
/// as received, base64url.
pub fn sig_ref(sig: &str) -> String {
    use sha2::{Digest, Sha256};
    super::keys::b64u(&Sha256::digest(sig.as_bytes())[..16])
}

pub fn reopen_body() -> Value {
    json!({"k": "reopen"})
}

pub fn reply_ok(re: u64, data: Value) -> Value {
    json!({"k": "reply", "re": re, "ok": true, "data": data})
}

pub fn reply_err(re: u64, code: Code) -> Value {
    json!({"k": "reply", "re": re, "ok": false, "code": code.as_str()})
}

/// A `re:0` failure: `ref` names the command's signature (see [`sig_ref`]).
pub fn reply_err_ref(code: Code, sig_ref: &str) -> Value {
    json!({"k": "reply", "re": 0, "ok": false, "code": code.as_str(), "ref": sig_ref})
}

pub fn panes_body(panes: &[Pane]) -> Value {
    json!({"k": "panes", "panes": panes})
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    #[test]
    fn signed_strings_are_exact() {
        assert_eq!(
            hello_signed("d", "https://h.example", "n"),
            "ymux-remote-v1|desktop|d|https://h.example|n"
        );
        assert_eq!(cmd_signed("{}"), b"ymux-remote-v1|cmd|{}".to_vec());
        assert_eq!(
            pair_proof_msg("a", "b", "c", "e"),
            "ymux-remote-v1|pair|a|b|c|e"
        );
        assert_eq!(
            link_info("a", "b", "c", "e", "d"),
            "ymux-remote-v1|link|a|b|c|e|d"
        );
        assert_eq!(aad_p2d("a", "b"), "ymux-remote-v1|p2d|a|b");
        assert_eq!(aad_d2p("a", "b"), "ymux-remote-v1|d2p|a|b");
    }

    #[test]
    fn menu_ids_are_short_url_safe_tokens() {
        assert!(is_menu_id("aB3_-x"));
        assert!(is_menu_id(&"a".repeat(32)));
        assert!(!is_menu_id(&"a".repeat(33)));
        assert!(!is_menu_id(""));
        assert!(!is_menu_id("a b"));
        assert!(!is_menu_id("a/b"));
        assert!(!is_menu_id("é"));
    }

    #[test]
    fn uuid_must_be_lowercase_hyphenated() {
        assert!(is_uuid(P));
        assert!(!is_uuid(&P.to_uppercase()));
        assert!(!is_uuid(&P.replace('-', "")));
        assert!(!is_uuid("{aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa}"));
        assert!(!is_uuid(""));
    }

    #[test]
    fn capabilities_are_ordered_and_ops_map_to_them() {
        assert!(Cap::View < Cap::Respond && Cap::Respond < Cap::Prompt);
        assert_eq!(Op::List.needs(), Cap::View);
        assert_eq!(Op::Peek.needs(), Cap::View);
        assert_eq!(Op::Answer.needs(), Cap::Respond);
        assert_eq!(Op::Interrupt.needs(), Cap::Respond);
        assert_eq!(Op::Prompt.needs(), Cap::Prompt);
        assert_eq!(Cap::parse("admin"), None);
    }

    #[test]
    fn parses_each_op() {
        let r = parse_request(&json!({"op":"peek","pane":P})).unwrap();
        assert_eq!(
            r,
            Request::Peek {
                pane: P.into(),
                lines: 80
            }
        );
        let r = parse_request(&json!({"op":"answer","pane":P,"n":2,"label":"Yes","menu_id":"m-1"}))
            .unwrap();
        assert_eq!(r.op(), Op::Answer);
        assert_eq!(
            r,
            Request::Answer {
                pane: P.into(),
                n: 2,
                label: "Yes".into(),
                menu_id: "m-1".into()
            }
        );
        let r = parse_request(&json!({"op":"prompt","pane":P,"text":"hi"})).unwrap();
        assert_eq!(
            r,
            Request::Prompt {
                pane: P.into(),
                text: "hi".into(),
                queue: false
            }
        );
        assert_eq!(parse_request(&json!({"op":"list"})).unwrap(), Request::List);
    }

    #[test]
    fn rejects_out_of_range_fields() {
        let bad = |v: Value| assert_eq!(parse_request(&v), Err(Code::BadRequest), "{v}");
        bad(json!({"op":"peek","pane":P,"lines":0}));
        bad(json!({"op":"peek","pane":P,"lines":201}));
        bad(json!({"op":"peek","pane":P,"lines":-3}));
        bad(json!({"op":"peek","pane":P,"lines":1.5}));
        bad(json!({"op":"peek","pane":"nope"}));
        bad(json!({"op":"peek"}));
        bad(json!({"op":"answer","pane":P,"n":-1,"label":"x","menu_id":"m"}));
        bad(json!({"op":"answer","pane":P,"n":"1","label":"x","menu_id":"m"}));
        bad(json!({"op":"answer","pane":P,"n":1,"menu_id":"m"}));
        // n is 1..=9: "10" would press "1" first.
        bad(json!({"op":"answer","pane":P,"n":0,"label":"x","menu_id":"m"}));
        bad(json!({"op":"answer","pane":P,"n":10,"label":"x","menu_id":"m"}));
        // menu_id is required and well-formed.
        bad(json!({"op":"answer","pane":P,"n":1,"label":"x"}));
        bad(json!({"op":"answer","pane":P,"n":1,"label":"x","menu_id":""}));
        bad(json!({"op":"answer","pane":P,"n":1,"label":"x","menu_id":"a b"}));
        bad(json!({"op":"answer","pane":P,"n":1,"label":"x","menu_id":"a".repeat(33)}));
        bad(json!({"op":"answer","pane":P,"n":1,"label":"x","menu_id":5}));
        bad(json!({"op":"prompt","pane":P,"text":"\u{1b}\u{7}"}));
        bad(json!({"op":"prompt","pane":P,"text":""}));
        bad(json!({"op":"prompt","pane":P,"text":"x".repeat(16_001)}));
        bad(json!({"op":"prompt","pane":P,"text":"x","queue":"yes"}));
        bad(json!({"op":5}));
        bad(json!([]));
        bad(json!("list"));
        // The limits themselves are fine.
        assert!(parse_request(&json!({"op":"peek","pane":P,"lines":200})).is_ok());
        assert!(parse_request(&json!({"op":"peek","pane":P,"lines":1})).is_ok());
        assert!(parse_request(&json!({"op":"prompt","pane":P,"text":"x".repeat(16_000)})).is_ok());
        assert!(parse_request(
            &json!({"op":"answer","pane":P,"n":9,"label":"x","menu_id":"a".repeat(32)})
        )
        .is_ok());
        assert_eq!(
            parse_request(&json!({"op":"keys","pane":P})),
            Err(Code::UnknownOp)
        );
    }

    #[test]
    fn unknown_server_frames_are_not_errors() {
        assert!(matches!(
            parse_server_frame(r#"{"t":"something_new","x":1}"#),
            Some(ServerFrame::Unknown)
        ));
        assert!(parse_server_frame(r#"{"t":"cmd","device_id":"x"}"#).is_none());
        assert!(parse_server_frame("not json").is_none());
        assert!(parse_server_frame(r#"{"no_t":1}"#).is_none());
        assert!(matches!(
            parse_server_frame(r#"{"t":"ready","devices":["a"]}"#),
            Some(ServerFrame::Ready { .. })
        ));
    }

    #[test]
    fn event_payloads_match_the_ipc_contract() {
        let e = Request::Prompt {
            pane: P.into(),
            text: "go".into(),
            queue: true,
        }
        .event(7);
        assert_eq!(
            e,
            json!({"id":7,"op":"prompt","pane":P,"text":"go","queue":true})
        );
        assert_eq!(Request::List.event(1), json!({"id":1,"op":"list"}));
        let e = Request::Answer {
            pane: P.into(),
            n: 2,
            label: "Yes".into(),
            menu_id: "m-1".into(),
        }
        .event(3);
        assert_eq!(
            e,
            json!({"id":3,"op":"answer","pane":P,"n":2,"label":"Yes","menu_id":"m-1"})
        );
    }

    #[test]
    fn prompt_text_is_cleaned_when_parsed() {
        let r =
            parse_request(&json!({"op":"prompt","pane":P,"text":"a\u{1b}[31m\r\nb\rc\u{7}\td"}))
                .unwrap();
        let Request::Prompt { text, .. } = r else {
            panic!()
        };
        assert_eq!(text, "a[31m\nb\nc\td");
    }

    #[test]
    fn session_body_echoes_the_open_nonce_and_carries_the_clock() {
        let prev = Prev {
            session: "old".into(),
            seq: 4,
        };
        let b = session_body(
            "nonce123",
            "sess",
            Cap::Respond,
            "My PC",
            1_700_000_000_000,
            &prev,
        );
        assert_eq!(
            b,
            json!({"k":"session","open":"nonce123","session":"sess","cap":"respond",
                   "name":"My PC","now_ms":1_700_000_000_000u64,
                   "prev":{"session":"old","seq":4},"v":1})
        );
        // No earlier session: empty and 0.
        let b = session_body("n", "s", Cap::View, "x", 1, &Prev::default());
        assert_eq!(b["prev"], json!({"session":"","seq":0}));
        assert!(is_open_nonce("AAAAAAAAAAAAAAAAAAAAAA"));
        assert!(!is_open_nonce("short"));
        assert!(!is_open_nonce("has space here!!"));
    }

    #[test]
    fn sig_ref_is_the_first_16_bytes_of_the_sha256_of_the_string() {
        // SHA-256("abc") = ba7816bf 8f01cfea 414140de 5dae2223 b00361a3 ...
        let want = super::super::keys::b64u(&[
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23,
        ]);
        assert_eq!(sig_ref("abc"), want);
        assert_eq!(want.len(), 22);
        assert_ne!(sig_ref("abc"), sig_ref("abd"));
        let r = reply_err_ref(Code::BadSig, &want);
        assert_eq!(
            r,
            json!({"k":"reply","re":0,"ok":false,"code":"bad_sig","ref":want})
        );
    }

    #[test]
    fn hello_omits_an_empty_setup_token() {
        let v: Value = serde_json::from_str(&hello_frame("d", "pk", "n", "", "s")).unwrap();
        assert!(v.get("setup_token").is_none());
        let v: Value = serde_json::from_str(&hello_frame("d", "pk", "n", "tok", "s")).unwrap();
        assert_eq!(v["setup_token"], "tok");
        assert_eq!(v["role"], "desktop");
        assert_eq!(v["v"], 1);
    }
}
