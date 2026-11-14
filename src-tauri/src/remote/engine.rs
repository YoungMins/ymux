//! The protocol state machine. Pure: it takes frames, commands and the clock
//! in, and hands a list of [`Out`] actions back. The runtime owns the socket,
//! the Tauri events and the config file; this owns every decision.
//!
//! One `Engine` lives for the whole run. [`Engine::on_connect`] resets the
//! per-connection state (phase, sessions, pairing window, commands waiting on
//! the app); the pane bookkeeping and the device list survive a reconnect.
//!
//! Everything for or from a paired device is sealed with that device's link
//! key (PROTOCOL.md section 1) and every command is signed by the device. The
//! server's `device_online` / `device_offline` / `conn` are display hints
//! only: a session exists because a device sent a valid `open`, nothing else.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use super::audit::{Aggregate, Entry};
use super::clip;
use super::keys::{self, Keypair, LinkKey};
use super::pairing::{Decided, Pairing, Verdict, MAX_DEVICES, TTL_S};
use super::panes::{Throttle, Tracker};
use super::proto::{self, Cap, Code, Op, OpenBody, Request, ServerFrame, MAX_BODY, VERSION};
use super::session::{Decision, Incoming, Opened, Sessions};
use crate::config::model::RemoteDevice;

/// How long the app has to answer a command.
pub const REPLY_TIMEOUT_MS: u64 = 5_000;
/// Commands waiting on the app at once; more are answered `busy`.
pub const MAX_INFLIGHT: usize = 8;
/// Unsent actions kept; beyond this new ones are dropped.
const OUT_CAP: usize = 2048;
/// Connection hints kept (`device_online` is only a display hint).
const MAX_HINTS: usize = 64;

/// What the runtime must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// A text frame for the server.
    Send(String),
    /// Emit `remote://request`.
    Request {
        id: u64,
        payload: Value,
    },
    /// Emit `remote://pair-request` (the proof already verified). It carries
    /// no check number: that goes to the phone, sealed, and the user types it.
    PairRequest {
        device_id: String,
        name: String,
    },
    /// Emit `remote://pair-conflict`: two devices used one code.
    PairConflict,
    Audit(Entry),
    /// Something in the status changed: emit `remote://status`.
    Changed,
    /// The server accepted this desktop.
    Ready,
    /// Drop the connection; the code is why (short, safe to show).
    Close(String),
}

/// What the user's answer to a pairing request did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairOutcome {
    /// The right number: the device is stored and trusted.
    Approved(RemoteDevice),
    /// The user said no.
    Rejected,
    /// A wrong number; the request waits for another try.
    Wrong { tries_left: u8 },
    /// The last wrong try: refused, and the window is closed.
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    AwaitChallenge,
    AwaitReady,
    Ready,
}

/// What an outgoing body carries besides its own fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stamp {
    /// The device's current session and the next `n`. Nothing is sent when
    /// the device has no session.
    Session,
    /// `"session":""`, no `n` and no `at_ms`, for a failure reply with `re:0`
    /// (a device acts on it only when its `ref` names a command in flight).
    Unsessioned,
    /// The `session` reply itself: it names the new session in its own field.
    Raw,
}

#[derive(Debug)]
struct InFlight {
    device_id: String,
    /// The session the command ran in; the reply is dropped if it ended.
    session: String,
    re: u64,
    op: Op,
    pane: String,
    deadline: u64,
}

pub struct Engine {
    desktop_id: String,
    name: String,
    setup_token: String,
    /// `scheme://host[:port]` of the configured server address.
    origin: String,
    keys: Keypair,
    devices: Vec<RemoteDevice>,
    unusable: Vec<String>,
    links: HashMap<String, LinkKey>,
    phase: Phase,
    sessions: Sessions,
    /// conn -> device id, from `device_online`. Display only.
    online: HashMap<String, String>,
    pairing: Option<Pairing>,
    inflight: HashMap<u64, InFlight>,
    next_id: u64,
    tracker: Tracker,
    throttle: Throttle,
    agg: Aggregate,
    fail_budget: FailBudget,
    /// Budget of the `re:0 bad_sig` replies (their own, so a flood of one
    /// kind cannot use up the other's).
    sig_budget: FailBudget,
    /// `ref` of the command being handled: put on its `re:0` failure replies.
    cur_ref: Option<String>,
    /// The clock of the entry point being run (stamped as `at_ms`).
    now: u64,
    notify_waiting: bool,
    notify_done: bool,
    out: Vec<Out>,
}

/// Sealed failure replies one device may be sent per window for frames that
/// fail the session rules (`replay`, `bad_session`, `expired`). A relay can
/// replay captured frames without limit; beyond this the frame is dropped
/// silently.
pub const FAIL_REPLIES_PER_WINDOW: u32 = 20;
const FAIL_WINDOW_MS: u64 = 60_000;

#[derive(Debug, Default)]
struct FailBudget {
    /// device id -> (window start, replies sent in it)
    windows: HashMap<String, (u64, u32)>,
}

impl FailBudget {
    fn allow(&mut self, now: u64, device: &str) -> bool {
        let w = self.windows.entry(device.to_string()).or_insert((now, 0));
        if now.saturating_sub(w.0) >= FAIL_WINDOW_MS {
            *w = (now, 0);
        }
        if w.1 >= FAIL_REPLIES_PER_WINDOW {
            return false;
        }
        w.1 += 1;
        true
    }
}

/// A server error code made safe to keep and show.
fn safe_code(raw: &str) -> String {
    let c: String = raw
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
        .take(32)
        .collect();
    if c.is_empty() {
        "server_error".to_string()
    } else {
        c
    }
}

impl Engine {
    pub fn new(desktop_id: &str, name: &str, keys: Keypair) -> Self {
        Self {
            desktop_id: desktop_id.to_string(),
            name: name.to_string(),
            setup_token: String::new(),
            origin: String::new(),
            keys,
            devices: Vec::new(),
            unusable: Vec::new(),
            links: HashMap::new(),
            phase: Phase::Idle,
            sessions: Sessions::default(),
            online: HashMap::new(),
            pairing: None,
            inflight: HashMap::new(),
            next_id: 1,
            tracker: Tracker::default(),
            throttle: Throttle::default(),
            agg: Aggregate::default(),
            fail_budget: FailBudget::default(),
            sig_budget: FailBudget::default(),
            cur_ref: None,
            now: 0,
            notify_waiting: true,
            notify_done: false,
            out: Vec::new(),
        }
    }

    // ------------------------------------------------------------ settings

    /// `setup_token` is sent in the hello only when non-empty: the caller
    /// passes it only while the server has not accepted this desktop yet.
    pub fn set_identity(&mut self, name: &str, setup_token: &str) {
        self.name = name.to_string();
        self.setup_token = setup_token.to_string();
    }

    /// The origin of the configured server address. The server's challenge
    /// must name the same one, and the hello is signed for it.
    pub fn set_origin(&mut self, origin: &str) {
        self.origin = origin.to_string();
    }

    pub fn set_notify(&mut self, waiting: bool, done: bool) {
        self.notify_waiting = waiting;
        self.notify_done = done;
    }

    /// Device ids the config holds but that cannot be used this run (their
    /// link key could not be read). `on_ready` never tells the server to
    /// remove them.
    pub fn set_unusable(&mut self, ids: Vec<String>) {
        self.unusable = ids;
    }

    /// Replace the device list (config changed), each with its link key.
    /// Sessions of devices no longer in it are dropped on the spot.
    pub fn set_devices(&mut self, devices: Vec<(RemoteDevice, LinkKey)>) {
        let gone: Vec<String> = self
            .devices
            .iter()
            .filter(|d| !devices.iter().any(|(n, _)| n.id == d.id))
            .map(|d| d.id.clone())
            .collect();
        for id in gone {
            self.sessions.drop_device(&id);
            self.fail_budget.windows.remove(&id);
            self.sig_budget.windows.remove(&id);
        }
        self.links = devices
            .iter()
            .map(|(d, k)| (d.id.clone(), k.clone()))
            .collect();
        self.devices = devices.into_iter().map(|(d, _)| d).collect();
    }

    /// How many devices the engine trusts right now.
    #[cfg(test)]
    pub(crate) fn device_count_for_test(&self) -> usize {
        self.devices.len()
    }

    /// The capability the engine holds for a device.
    #[cfg(test)]
    pub(crate) fn device_cap_for_test(&self, device_id: &str) -> Option<String> {
        self.devices
            .iter()
            .find(|d| d.id == device_id)
            .map(|d| d.cap.clone())
    }

    /// The check number a waiting pairing request is held to. Test builds
    /// only: the demo's auto-approve shortcut ("types" the number the engine
    /// minted itself); no production path can read it.
    #[cfg(test)]
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))]
    pub(crate) fn pending_check_for_test(&self, device_id: &str) -> Option<String> {
        self.pairing.as_ref()?.check_for_test(device_id)
    }

    /// Give a device a session with a known id (recorded vectors).
    #[cfg(test)]
    pub(crate) fn adopt_session_for_test(&mut self, device_id: &str, session: &str) {
        self.sessions.insert_for_test(device_id, session);
    }

    /// The same, with `seq` commands already accepted in it.
    #[cfg(test)]
    pub(crate) fn adopt_session_with_seq_for_test(
        &mut self,
        device_id: &str,
        session: &str,
        seq: u64,
    ) {
        self.sessions
            .insert_with_seq_for_test(device_id, session, seq);
    }

    // ------------------------------------------------------------- queries

    pub fn connected(&self) -> bool {
        self.phase == Phase::Ready
    }

    /// Devices the server says are connected (for the status dot only).
    pub fn online_devices(&self) -> HashSet<String> {
        self.online
            .values()
            .filter(|id| self.devices.iter().any(|d| &d.id == *id))
            .cloned()
            .collect()
    }

    /// `(code_b64u, dk_b64u, expires_ms)` of the open pairing window.
    pub fn pairing_window(&self) -> Option<(String, String, u64)> {
        self.pairing
            .as_ref()
            .map(|p| (p.code_b64u(), p.dk_b64u(), p.expires_ms()))
    }

    /// Take the pending actions.
    pub fn drain(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    fn push(&mut self, o: Out) {
        if self.out.len() < OUT_CAP {
            self.out.push(o);
        }
    }

    fn send(&mut self, frame: String) {
        self.push(Out::Send(frame));
    }

    fn audit(&mut self, now: u64, device: &str, op: &'static str, pane: &str, code: &'static str) {
        self.push(Out::Audit(Entry {
            ts_ms: now,
            device: device.to_string(),
            op,
            pane: pane.to_string(),
            code,
            count: 1,
        }));
    }

    /// A rejection that happened before the sender was authenticated: counted
    /// under "no device", one line per code per minute.
    fn pre_auth(&mut self, now: u64, code: &'static str) {
        self.agg.note(now, code);
    }

    /// A rejection a remote party can repeat at will (a paired device or a
    /// relay replaying its frames): counted, one line per device and code per
    /// minute, never one line per frame.
    fn reject_note(&mut self, now: u64, device: &str, code: &'static str) {
        self.agg.note_device(now, device, code);
    }

    fn flush_agg(&mut self, now: u64, all: bool) {
        for e in self.agg.flush(now, all) {
            self.push(Out::Audit(e));
        }
    }

    // ------------------------------------------------------------- sealing

    /// The sealed `msg` frame for a body, or `None` (nothing to send) when the
    /// device is unknown, has no session where one is needed, or sealing fails.
    fn seal_frame(&mut self, device_id: &str, body: &Value, stamp: Stamp) -> Option<String> {
        let link = self.links.get(device_id)?.clone();
        let mut full = body.clone();
        match stamp {
            Stamp::Session => {
                if !self.sessions.stamp(device_id, &mut full, self.now) {
                    return None;
                }
            }
            // `session:""`, no `n`, no `at_ms`: a literal reading of 5.1 (it
            // is no body of the session; the device matches its `ref`).
            Stamp::Unsessioned => {
                full["session"] = Value::String(String::new());
            }
            Stamp::Raw => {}
        }
        let sealed = link.seal(
            &proto::aad_d2p(&self.desktop_id, device_id),
            full.to_string().as_bytes(),
        )?;
        Some(proto::msg_frame_str(device_id, &sealed))
    }

    /// Seal and queue a body for one device. A reply too large for a frame
    /// becomes an `internal` error reply instead.
    fn emit(&mut self, device_id: &str, body: &Value, stamp: Stamp) {
        let Some(frame) = self.seal_frame(device_id, body, stamp) else {
            return;
        };
        if frame.len() <= proto::MAX_FRAME {
            self.send(frame);
            return;
        }
        let Some(re) = body.get("re").and_then(Value::as_u64) else {
            return;
        };
        if body.get("ok") == Some(&Value::Bool(true)) {
            let fallback = proto::reply_err(re, Code::Internal);
            if let Some(f) = self.seal_frame(device_id, &fallback, stamp) {
                if f.len() <= proto::MAX_FRAME {
                    self.send(f);
                }
            }
        }
    }

    /// A failure reply. `re == 0` means the body was not trusted enough to
    /// read its `seq`; it then carries `ref`, the signature of the frame it
    /// answers, so the device can tell which command that was.
    fn fail(&mut self, device_id: &str, re: u64, code: Code) {
        if re == 0 {
            let body = match self.cur_ref.as_deref() {
                Some(r) => proto::reply_err_ref(code, r),
                None => proto::reply_err(0, code),
            };
            self.emit(device_id, &body, Stamp::Unsessioned);
        } else {
            self.emit(device_id, &proto::reply_err(re, code), Stamp::Session);
        }
    }

    // ---------------------------------------------------------- connection

    pub fn on_connect(&mut self) {
        self.phase = Phase::AwaitChallenge;
        self.sessions.clear();
        self.online.clear();
        self.pairing = None;
        self.inflight.clear();
        self.push(Out::Changed);
    }

    pub fn on_disconnect(&mut self, now: u64) {
        self.now = now;
        self.phase = Phase::Idle;
        self.sessions.clear();
        self.online.clear();
        self.pairing = None;
        self.inflight.clear();
        self.flush_agg(now, true);
        self.push(Out::Changed);
    }

    // -------------------------------------------------------------- frames

    /// One text frame from the server. Anything that does not parse, or
    /// arrives in the wrong phase, is ignored.
    pub fn on_text(&mut self, text: &str, now: u64) {
        let Some(frame) = proto::parse_server_frame(text) else {
            return;
        };
        self.now = now;
        match frame {
            ServerFrame::Challenge { nonce, v, origin } => self.on_challenge(&nonce, v, &origin),
            ServerFrame::Ready { devices } => self.on_ready(&devices),
            ServerFrame::Error { code } => {
                if matches!(self.phase, Phase::AwaitChallenge | Phase::AwaitReady) {
                    self.phase = Phase::Idle;
                    self.push(Out::Close(safe_code(&code)));
                }
            }
            ServerFrame::DeviceOnline { device_id, conn } if self.phase == Phase::Ready => {
                self.on_device_online(&device_id, &conn);
            }
            ServerFrame::DeviceOffline { device_id, conn } if self.phase == Phase::Ready => {
                self.on_device_offline(&device_id, &conn);
            }
            ServerFrame::PairRequest {
                device_id,
                pubkey,
                ek,
                name,
                proof,
            } if self.phase == Phase::Ready => {
                self.on_pair_request(now, &device_id, &pubkey, &ek, &name, &proof);
            }
            ServerFrame::Cmd {
                device_id,
                conn: _,
                body,
                sig,
            } if self.phase == Phase::Ready => {
                self.on_cmd(now, &device_id, &body, &sig);
            }
            _ => {}
        }
    }

    fn on_challenge(&mut self, nonce: &str, v: u32, origin: &str) {
        if self.phase != Phase::AwaitChallenge {
            return;
        }
        if v != VERSION {
            self.phase = Phase::Idle;
            self.push(Out::Close("version".into()));
            return;
        }
        // Another server (or a proxy in front of one) answering for the
        // address the user typed: sign nothing for it.
        if self.origin.is_empty() || origin != self.origin {
            self.phase = Phase::Idle;
            self.push(Out::Close("origin_mismatch".into()));
            return;
        }
        if !matches!(keys::b64u_decode(nonce), Some(n) if n.len() >= 16) {
            self.phase = Phase::Idle;
            self.push(Out::Close("bad_challenge".into()));
            return;
        }
        let sig = self
            .keys
            .sign(proto::hello_signed(&self.desktop_id, &self.origin, nonce).as_bytes());
        let hello = proto::hello_frame(
            &self.desktop_id,
            &self.keys.public_b64u(),
            &self.name,
            &self.setup_token,
            &sig,
        );
        self.phase = Phase::AwaitReady;
        self.send(hello);
    }

    fn on_ready(&mut self, server_devices: &[String]) {
        if self.phase != Phase::AwaitReady {
            return;
        }
        self.phase = Phase::Ready;
        // A device the server still holds but this desktop does not know can
        // never run a command here, yet it would keep receiving pushes.
        // A device the config still holds but whose link key could not be
        // read this run is kept, not removed: the secret store may recover.
        for id in server_devices {
            if !self.devices.iter().any(|d| &d.id == id) && !self.unusable.contains(id) {
                self.send(proto::device_remove_frame(id));
            }
        }
        self.push(Out::Ready);
        self.push(Out::Changed);
    }

    /// A display hint, nothing more: it creates no session.
    fn on_device_online(&mut self, device_id: &str, conn: &str) {
        if !self.devices.iter().any(|d| d.id == device_id) {
            return;
        }
        if self.online.len() >= MAX_HINTS && !self.online.contains_key(conn) {
            return;
        }
        self.online.insert(conn.to_string(), device_id.to_string());
        self.push(Out::Changed);
    }

    /// The device's last connection went away: its session ends too (it may
    /// outlive nothing). A relay that hides this only keeps the hint stale;
    /// the session still ends with the next `open` or a desktop reconnect.
    fn on_device_offline(&mut self, device_id: &str, conn: &str) {
        self.online.remove(conn);
        if !self.online.values().any(|d| d == device_id) {
            self.sessions.revoke(device_id);
        }
        self.push(Out::Changed);
    }

    // ------------------------------------------------------------ commands

    /// A relayed `cmd` frame. Order: the device is paired, its signature
    /// verifies against the key this desktop stored, the body opens under its
    /// link key; only then is anything read.
    fn on_cmd(&mut self, now: u64, device_id: &str, body: &str, sig: &str) {
        let Some(device) = self.devices.iter().find(|d| d.id == device_id).cloned() else {
            self.pre_auth(now, "unknown_device");
            return;
        };
        let Some(link) = self.links.get(device_id).cloned() else {
            self.pre_auth(now, "unknown_device");
            return;
        };
        // A huge body costs nothing.
        if body.len() > proto::MAX_SEALED {
            self.pre_auth(now, "too_big");
            return;
        }
        self.cur_ref = Some(proto::sig_ref(sig));
        self.on_known_cmd(now, &device, &link, body, sig);
        self.cur_ref = None;
    }

    /// `on_cmd` past the lookups: the failure replies of this frame carry its
    /// `ref` (set by the caller).
    fn on_known_cmd(
        &mut self,
        now: u64,
        device: &RemoteDevice,
        link: &LinkKey,
        body: &str,
        sig: &str,
    ) {
        let device_id = device.id.as_str();
        // A bad signature or a body that does not open is what a relay forging
        // or tampering produces at will: counted per device, and only a small
        // budget of them is answered (the answer is what lets the phone settle
        // a command whose reply it never got).
        if !keys::verify(&device.pubkey, &proto::cmd_signed(body), sig) {
            self.reject_note(now, device_id, Code::BadSig.as_str());
            if self.sig_budget.allow(now, device_id) {
                self.fail(device_id, 0, Code::BadSig);
            }
            return;
        }
        let aad = proto::aad_p2d(&self.desktop_id, device_id);
        let Some(plain) = link.open(&aad, body) else {
            self.reject_note(now, device_id, "bad_seal");
            if self.sig_budget.allow(now, device_id) {
                self.fail(device_id, 0, Code::BadSig);
            }
            return;
        };
        if plain.len() > MAX_BODY {
            self.reject_note(now, device_id, Code::BadRequest.as_str());
            self.fail(device_id, 0, Code::BadRequest);
            return;
        }
        let Ok(plain) = String::from_utf8(plain) else {
            self.reject_note(now, device_id, Code::BadRequest.as_str());
            self.fail(device_id, 0, Code::BadRequest);
            return;
        };
        let kind = serde_json::from_str::<Value>(&plain)
            .ok()
            .and_then(|v| v.get("k").and_then(Value::as_str).map(str::to_string));
        match kind.as_deref() {
            Some("open") => self.on_open(now, device, &plain),
            Some("cmd") => self.on_command(now, device, &plain),
            _ => {
                self.reject_note(now, device_id, Code::BadRequest.as_str());
                self.fail(device_id, 0, Code::BadRequest);
            }
        }
    }

    /// A device asks for a session. The only way one comes to exist.
    fn on_open(&mut self, now: u64, device: &RemoteDevice, plain: &str) {
        let valid = serde_json::from_str::<OpenBody>(plain).ok().filter(|o| {
            o.desktop_id == self.desktop_id
                && o.device_id == device.id
                && proto::is_open_nonce(&o.nonce)
        });
        let Some(open) = valid else {
            self.reject_note(now, &device.id, Code::BadRequest.as_str());
            self.fail(&device.id, 0, Code::BadRequest);
            return;
        };
        let Some(cap) = Cap::parse(&device.cap) else {
            self.reject_note(now, &device.id, Code::Forbidden.as_str());
            self.fail(&device.id, 0, Code::Forbidden);
            return;
        };
        match self.sessions.open(&device.id, now) {
            // Old `open` frames replayed by the relay land here.
            Opened::Limited => self.reject_note(now, &device.id, "open_limited"),
            Opened::Issued { session, prev } => {
                let body = proto::session_body(&open.nonce, &session, cap, &self.name, now, &prev);
                self.emit(&device.id, &body, Stamp::Raw);
                self.push(Out::Changed);
            }
        }
    }

    /// The §5.1 checks on an opened command, then the app (or an answer given
    /// here).
    fn on_command(&mut self, now: u64, device: &RemoteDevice, plain: &str) {
        let device_id = device.id.as_str();
        let decision = self.sessions.authorise(
            &Incoming {
                desktop_id: &self.desktop_id,
                device,
                plaintext: plain,
            },
            now,
        );
        match decision {
            Decision::Gap => {
                // The relay dropped or reordered a command. Aggregated like
                // every other rejection a relay can repeat.
                self.reject_note(now, device_id, "seq_gap");
                if self.fail_budget.allow(now, device_id) {
                    self.fail(device_id, 0, Code::BadSession);
                }
                self.push(Out::Changed);
            }
            Decision::Reject { re, code, .. } => {
                // Every rejection is something the device (or a relay that
                // replays its frames) can repeat at will, a low-capability
                // device streaming `forbidden` included: counted, one audit
                // line per device and code per minute. Frames that can be
                // replayed verbatim get only a small budget of answers.
                self.reject_note(now, device_id, code.as_str());
                if matches!(code, Code::Replay | Code::BadSession | Code::Expired) {
                    if self.fail_budget.allow(now, device_id) {
                        self.fail(device_id, re, code);
                    }
                    return;
                }
                self.fail(device_id, re, code);
            }
            Decision::Allow {
                re,
                session,
                request,
            } => {
                // A text that starts with `!` may be run as a shell command
                // by an agent CLI, outside its permission rules. Never typed.
                if let Request::Prompt { text, .. } = &request {
                    if clip::starts_with_bang(text) {
                        self.reject_note(now, device_id, "refused");
                        let body = proto::reply_ok(re, json!({"result": "refused"}));
                        self.emit(device_id, &body, Stamp::Session);
                        return;
                    }
                }
                if self.inflight.len() >= MAX_INFLIGHT {
                    self.reject_note(now, device_id, Code::Busy.as_str());
                    self.fail(device_id, re, Code::Busy);
                    return;
                }
                let id = self.next_id;
                self.next_id += 1;
                let deadline = now.saturating_add(REPLY_TIMEOUT_MS);
                self.inflight.insert(
                    id,
                    InFlight {
                        device_id: device_id.to_string(),
                        session,
                        re,
                        op: request.op(),
                        pane: request.pane().to_string(),
                        deadline,
                    },
                );
                // The page must not press Enter after the moment this app
                // answers `timeout`: a late reply is ignored.
                let mut payload = request.event(id);
                if let Some(o) = payload.as_object_mut() {
                    o.insert("deadline_ms".into(), Value::from(deadline));
                }
                self.push(Out::Request { id, payload });
            }
        }
    }

    /// Deliver a reply to a command, if the session it ran in is still the
    /// device's current one (the reply's `re` is a number in that session).
    fn reply_in_session(&mut self, f: &InFlight, body: &Value, now: u64) {
        if self.sessions.current(&f.device_id) != Some(f.session.as_str()) {
            self.audit(now, &f.device_id, f.op.as_str(), &f.pane, "bad_session");
            return;
        }
        self.emit(&f.device_id, body, Stamp::Session);
    }

    /// The app's answer to request `id`. A late or unknown id is ignored.
    pub fn frontend_reply(&mut self, id: u64, data: &Value, now: u64) {
        self.now = now;
        let Some(f) = self.inflight.remove(&id) else {
            return;
        };
        let (body, code) = match clip::reply_data(f.op, data) {
            Some(d) => (proto::reply_ok(f.re, d), "ok"),
            None => (proto::reply_err(f.re, Code::Internal), "internal"),
        };
        // A polling phone repeats `peek` every couple of seconds: accepted
        // reads are one line per device, op and minute with a count. A write
        // keeps a line of its own: that is what an audit is for.
        if code == "ok" && matches!(f.op, proto::Op::List | proto::Op::Peek) {
            self.agg.note_op(now, &f.device_id, f.op.as_str(), "ok");
        } else {
            self.audit(now, &f.device_id, f.op.as_str(), &f.pane, code);
        }
        self.reply_in_session(&f, &body, now);
    }

    // --------------------------------------------------------------- timers

    /// Call a few times a second: answers commands the app left hanging,
    /// closes an expired pairing window, writes finished audit windows,
    /// releases a due `panes` frame.
    pub fn tick(&mut self, now: u64) {
        self.now = now;
        let late: Vec<u64> = self
            .inflight
            .iter()
            .filter(|(_, f)| now >= f.deadline)
            .map(|(id, _)| *id)
            .collect();
        for id in late {
            if let Some(f) = self.inflight.remove(&id) {
                self.audit(
                    now,
                    &f.device_id,
                    f.op.as_str(),
                    &f.pane,
                    Code::Timeout.as_str(),
                );
                let body = proto::reply_err(f.re, Code::Timeout);
                self.reply_in_session(&f, &body, now);
            }
        }
        if self.pairing.as_ref().is_some_and(|p| p.expired(now)) {
            self.close_pairing(false);
        }
        self.flush_agg(now, false);
        self.flush_panes(now);
    }

    /// The `panes` push goes to each device that has a session, sealed for it.
    fn flush_panes(&mut self, now: u64) {
        if self.phase != Phase::Ready || !self.sessions.any() {
            return;
        }
        let Some(list) = self.throttle.take(now) else {
            return;
        };
        let Ok(panes) = serde_json::from_str::<Value>(&list) else {
            return;
        };
        let body = json!({"k": "panes", "panes": panes});
        for id in self.sessions.device_ids() {
            self.emit(&id, &body, Stamp::Session);
        }
    }

    // ---------------------------------------------------------------- panes

    /// The frontend's current pane list. Notifications go out at once; the
    /// `panes` push is throttled.
    pub fn publish_panes(&mut self, v: &Value, now: u64) {
        self.now = now;
        let Some(list) = clip::panes(v) else {
            return;
        };
        let notices = self.tracker.observe(
            &list,
            now,
            self.notify_waiting,
            self.notify_done,
            self.phase == Phase::Ready,
        );
        for n in notices {
            self.send(proto::notify_frame(n.kind, &n.title, &n.body, &n.pane));
        }
        if let Ok(json) = serde_json::to_string(&list) {
            self.throttle.set(json);
        }
        self.flush_panes(now);
    }

    // -------------------------------------------------------------- pairing

    fn on_pair_request(
        &mut self,
        now: u64,
        device_id: &str,
        pubkey: &str,
        ek: &str,
        name: &str,
        proof: &str,
    ) {
        let had_window = self.pairing.is_some();
        let verdict = match self.pairing.as_mut() {
            Some(p) => p.on_request(
                now,
                &self.desktop_id,
                &self.devices,
                device_id,
                pubkey,
                ek,
                name,
                proof,
            ),
            None => Verdict::Reject("no_window"),
        };
        match verdict {
            Verdict::Reject(why) => {
                // A relay can send these at will, with any claimed device id:
                // counted under "no device" (one line per code per minute),
                // never one line per request, and with no window open not
                // even answered.
                self.pre_auth(now, why);
                if had_window {
                    self.send(proto::pair_result_frame(device_id, None));
                }
            }
            Verdict::Ask(a) => {
                // The number goes to the phone, sealed under the key only it
                // can derive, and nowhere else.
                let plain = json!({"k": "pair_check", "check": a.check.as_str()}).to_string();
                let sealed = a.link.seal(
                    &proto::aad_d2p(&self.desktop_id, &a.device_id),
                    plain.as_bytes(),
                );
                let Some(sealed) = sealed else {
                    self.pre_auth(now, "seal_failed");
                    self.send(proto::pair_result_frame(device_id, None));
                    return;
                };
                self.send(proto::pair_challenge_frame(&a.device_id, &sealed));
                self.push(Out::PairRequest {
                    device_id: a.device_id,
                    name: a.name,
                });
                self.push(Out::Changed);
            }
            Verdict::Conflict => {
                // Two devices hold a code that was in one QR. Neither is
                // trusted: both are refused and the window ends.
                self.audit(now, device_id, "pair", "", "conflict");
                self.send(proto::pair_result_frame(device_id, None));
                self.close_pairing(true);
                self.push(Out::PairConflict);
            }
        }
    }

    /// Open a pairing window with a fresh code and key-agreement key. The
    /// caller builds the QR link from `(code, dk)`.
    pub fn pair_open(&mut self, now: u64) -> Result<(String, String, u64), &'static str> {
        self.now = now;
        if self.phase != Phase::Ready {
            return Err("offline");
        }
        if self.pairing.is_some() {
            self.close_pairing(true);
        }
        let p = Pairing::open(now);
        let out = (p.code_b64u(), p.dk_b64u(), p.expires_ms());
        self.pairing = Some(p);
        self.send(proto::pair_open_frame(TTL_S));
        self.push(Out::Changed);
        Ok(out)
    }

    pub fn pair_cancel(&mut self) {
        if self.pairing.is_some() {
            self.close_pairing(true);
        }
    }

    /// End the window; everyone still waiting is told no.
    fn close_pairing(&mut self, tell_server: bool) {
        let Some(mut p) = self.pairing.take() else {
            return;
        };
        for id in p.drain_pending() {
            self.send(proto::pair_result_frame(&id, None));
        }
        if tell_server {
            self.send(proto::pair_close_frame());
        }
        self.push(Out::Changed);
    }

    /// The user's answer. `check` is the six digits they typed from the
    /// phone's screen. A wrong number is counted ([`MAX_TRIES`] in all) and
    /// leaves the request waiting; the last wrong one refuses it and closes
    /// the window. Only a right number approves, and then `persist` stores
    /// the device and its link key (secrets first, then config); when it
    /// returns `false` the phone is refused and nothing is kept.
    pub fn pair_decide(
        &mut self,
        device_id: &str,
        approve: bool,
        cap: &str,
        check: &str,
        now: u64,
        persist: &mut dyn FnMut(&RemoteDevice, &LinkKey) -> bool,
    ) -> Result<PairOutcome, &'static str> {
        self.now = now;
        if self.pairing.as_ref().is_some_and(|p| p.expired(now)) {
            self.close_pairing(false);
            return Err("expired");
        }
        let cap = if approve {
            Some(Cap::parse(cap).ok_or("bad_cap")?)
        } else {
            None
        };
        let decided = self
            .pairing
            .as_mut()
            .map(|p| p.decide(device_id, approve, check))
            .ok_or("unknown_request")?;
        match decided {
            Decided::Unknown => Err("unknown_request"),
            Decided::Rejected => {
                self.audit(now, device_id, "pair", "", "denied");
                self.send(proto::pair_result_frame(device_id, None));
                self.push(Out::Changed);
                Ok(PairOutcome::Rejected)
            }
            Decided::Wrong { tries_left } => {
                self.audit(now, device_id, "pair", "", "wrong_check");
                Ok(PairOutcome::Wrong { tries_left })
            }
            Decided::Refused => {
                self.audit(now, device_id, "pair", "", "refused");
                self.send(proto::pair_result_frame(device_id, None));
                self.close_pairing(true);
                Ok(PairOutcome::Refused)
            }
            Decided::Approved {
                device_id: id,
                pubkey,
                name,
                link,
            } => {
                let cap = cap.ok_or("bad_cap")?;
                let known = self.devices.iter().any(|d| d.id == id);
                if !known && self.devices.len() >= MAX_DEVICES {
                    self.send(proto::pair_result_frame(&id, None));
                    return Err("limit");
                }
                let device = RemoteDevice {
                    id,
                    name,
                    pubkey,
                    cap: cap.as_str().to_string(),
                    added_ms: now,
                };
                if !persist(&device, &link) {
                    self.audit(now, device_id, "pair", "", "store_failed");
                    self.send(proto::pair_result_frame(device_id, None));
                    self.push(Out::Changed);
                    return Err("store_failed");
                }
                // A device paired again starts from nothing.
                self.sessions.drop_device(&device.id);
                self.devices.retain(|d| d.id != device.id);
                self.devices.push(device.clone());
                self.links.insert(device.id.clone(), link);
                self.audit(now, device_id, "pair", "", "ok");
                self.send(proto::pair_result_frame(
                    device_id,
                    Some((&device.pubkey, &device.name)),
                ));
                self.close_pairing(true);
                Ok(PairOutcome::Approved(device))
            }
        }
    }

    // -------------------------------------------------------------- devices

    /// A device's capability changed (the list was already replaced through
    /// [`Engine::set_devices`]): tell it to open a new session, in its current
    /// one, and end that one. Its next `open` is answered at the new level.
    pub fn device_cap_changed(&mut self, device_id: &str, now: u64) {
        self.now = now;
        let known = self
            .devices
            .iter()
            .find(|d| d.id == device_id)
            .is_some_and(|d| Cap::parse(&d.cap).is_some());
        if !known {
            self.sessions.revoke(device_id);
            return;
        }
        self.audit(now, device_id, "cap", "", "changed");
        if self.phase == Phase::Ready && self.sessions.current(device_id).is_some() {
            self.emit(device_id, &proto::reopen_body(), Stamp::Session);
        }
        self.sessions.revoke(device_id);
        self.push(Out::Changed);
    }

    /// A device was removed (already out of the list): end its sessions and
    /// tell the server to forget it.
    pub fn device_removed(&mut self, device_id: &str, now: u64) {
        self.now = now;
        self.sessions.drop_device(device_id);
        self.online.retain(|_, d| d != device_id);
        self.audit(now, device_id, "unpair", "", "ok");
        if self.phase == Phase::Ready {
            self.send(proto::device_remove_frame(device_id));
        }
        self.push(Out::Changed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::keys::pair_proof;

    const DESK: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
    const DEV: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
    const DEV2: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    const PANE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const ORIGIN: &str = "https://r.example.com";
    const CONN: &str = "c1";
    const NOW: u64 = 1_700_000_000_000;
    const OPEN_NONCE: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn device(id: &str, phone: &Keypair, cap: &str) -> RemoteDevice {
        RemoteDevice {
            id: id.into(),
            name: "Phone".into(),
            pubkey: phone.public_b64u(),
            cap: cap.into(),
            added_ms: 1,
        }
    }

    fn link() -> LinkKey {
        LinkKey::from_bytes([7u8; 32])
    }

    fn engine_with(devices: Vec<(RemoteDevice, LinkKey)>) -> Engine {
        let mut e = Engine::new(DESK, "My PC", Keypair::generate());
        e.set_origin(ORIGIN);
        e.set_devices(devices);
        e
    }

    fn challenge() -> String {
        json!({"t":"challenge","nonce":keys::random_b64u(32),"v":1,"server":"0.1.0",
               "origin":ORIGIN})
        .to_string()
    }

    fn frames(out: &[Out]) -> Vec<Value> {
        out.iter()
            .filter_map(|o| match o {
                Out::Send(s) => serde_json::from_str(s).ok(),
                _ => None,
            })
            .collect()
    }

    fn handshake(e: &mut Engine) {
        e.on_connect();
        e.on_text(&challenge(), 0);
        e.on_text(&json!({"t":"ready","devices":[]}).to_string(), 0);
        e.drain();
    }

    /// The phone's side of the wire, for one device.
    struct Rig {
        e: Engine,
        phone: Keypair,
        link: LinkKey,
        session: String,
        seq: u64,
        /// Last `n` the phone accepted.
        last_n: u64,
    }

    impl Rig {
        /// Past the handshake with one paired device that is online but has
        /// no session yet.
        fn bare(cap: &str) -> Rig {
            let phone = Keypair::generate();
            let mut e = engine_with(vec![(device(DEV, &phone, cap), link())]);
            handshake(&mut e);
            e.on_text(
                &json!({"t":"device_online","device_id":DEV,"conn":CONN}).to_string(),
                NOW,
            );
            e.drain();
            Rig {
                e,
                phone,
                link: link(),
                session: String::new(),
                seq: 0,
                last_n: 0,
            }
        }

        /// A rig whose device has opened a session.
        fn new(cap: &str) -> Rig {
            let mut r = Rig::bare(cap);
            r.open(NOW);
            r
        }

        fn sealed(&self, plain: &Value) -> String {
            self.link
                .seal(&proto::aad_p2d(DESK, DEV), plain.to_string().as_bytes())
                .unwrap()
        }

        fn frame_for(&self, body: &str) -> String {
            let sig = self.phone.sign(&proto::cmd_signed(body));
            json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":body,"sig":sig}).to_string()
        }

        fn send_plain(&mut self, plain: &Value, now: u64) -> Vec<Out> {
            let frame = self.frame_for(&self.sealed(plain));
            self.e.on_text(&frame, now);
            self.e.drain()
        }

        /// Everything the desktop sent this device, opened.
        fn opened(&self, out: &[Out]) -> Vec<Value> {
            frames(out)
                .iter()
                .filter(|f| f["t"] == "msg")
                .map(|f| {
                    assert_eq!(f["to"], DEV);
                    let plain = self
                        .link
                        .open(&proto::aad_d2p(DESK, DEV), f["body"].as_str().unwrap())
                        .expect("a body sealed for this device");
                    serde_json::from_slice(&plain).unwrap()
                })
                .collect()
        }

        /// Like a real device: take the `session` reply, then check `n`.
        fn open(&mut self, now: u64) -> Vec<Out> {
            let out = self.send_plain(
                &json!({"k":"open","desktop_id":DESK,"device_id":DEV,"nonce":OPEN_NONCE}),
                now,
            );
            let bodies = self.opened(&out);
            if let Some(s) = bodies.iter().find(|b| b["k"] == "session") {
                assert_eq!(s["open"], OPEN_NONCE);
                self.session = s["session"].as_str().unwrap().to_string();
                self.seq = 0;
                self.last_n = 0;
            }
            out
        }

        fn cmd_plain(&mut self, cmd: Value, ts: u64) -> Value {
            self.seq += 1;
            json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":self.session,
                   "seq":self.seq,"ts_ms":ts,"cmd":cmd})
        }

        fn cmd(&mut self, cmd: Value, now: u64) -> Vec<Out> {
            let plain = self.cmd_plain(cmd, now);
            self.send_plain(&plain, now)
        }

        /// The one body for this device in `out`, checking `session` and that
        /// `n` went up.
        fn reply(&mut self, out: &[Out]) -> Value {
            let bodies = self.opened(out);
            assert_eq!(bodies.len(), 1, "{bodies:?}");
            let b = bodies[0].clone();
            if b["session"] != "" {
                assert_eq!(b["session"], self.session.as_str());
                let n = b["n"].as_u64().unwrap();
                assert!(
                    n > self.last_n,
                    "n must increase: {n} after {}",
                    self.last_n
                );
                self.last_n = n;
            }
            b
        }
    }

    fn request_id(out: &[Out]) -> u64 {
        out.iter()
            .find_map(|o| match o {
                Out::Request { id, .. } => Some(*id),
                _ => None,
            })
            .expect("a request event")
    }

    fn has_request(out: &[Out]) -> bool {
        out.iter().any(|o| matches!(o, Out::Request { .. }))
    }

    fn audits(out: &[Out]) -> Vec<Entry> {
        out.iter()
            .filter_map(|o| match o {
                Out::Audit(a) => Some(a.clone()),
                _ => None,
            })
            .collect()
    }

    // ------------------------------------------------------------ handshake

    #[test]
    fn hello_is_bound_to_the_origin_and_sends_the_token() {
        let key = Keypair::generate();
        let public = key.public_b64u();
        let mut e = Engine::new(DESK, "My PC", key);
        e.set_origin(ORIGIN);
        e.set_identity("My PC", "setup-secret");
        e.on_connect();
        e.drain();
        let nonce = keys::random_b64u(32);
        e.on_text(
            &json!({"t":"challenge","nonce":nonce,"v":1,"origin":ORIGIN}).to_string(),
            0,
        );
        let f = frames(&e.drain());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0]["t"], "hello");
        assert_eq!(f[0]["role"], "desktop");
        assert_eq!(f[0]["desktop_id"], DESK);
        assert_eq!(f[0]["pubkey"], public);
        assert_eq!(f[0]["setup_token"], "setup-secret");
        let signed = format!("ymux-remote-v1|desktop|{DESK}|{ORIGIN}|{nonce}");
        assert!(keys::verify(
            &public,
            signed.as_bytes(),
            f[0]["sig"].as_str().unwrap()
        ));
        // Not valid for another origin.
        let other = format!("ymux-remote-v1|desktop|{DESK}|https://evil.example|{nonce}");
        assert!(!keys::verify(
            &public,
            other.as_bytes(),
            f[0]["sig"].as_str().unwrap()
        ));
        assert!(!e.connected());
        e.on_text(&json!({"t":"ready","devices":[]}).to_string(), 0);
        let out = e.drain();
        assert!(out.contains(&Out::Ready));
        assert!(e.connected());
    }

    #[test]
    fn no_token_is_sent_once_registered() {
        let mut e = engine_with(vec![]);
        e.set_identity("My PC", "");
        e.on_connect();
        e.drain();
        e.on_text(&challenge(), 0);
        let f = frames(&e.drain());
        assert!(f[0].get("setup_token").is_none());
    }

    #[test]
    fn a_challenge_from_another_origin_is_not_signed() {
        for origin in [
            json!("https://evil.example"),
            json!("https://r.example.com:8443"),
            json!("http://r.example.com"),
            json!("https://R.example.com"),
            json!(""),
            Value::Null,
        ] {
            let mut e = engine_with(vec![]);
            e.on_connect();
            e.drain();
            let mut c = json!({"t":"challenge","nonce":keys::random_b64u(32),"v":1});
            if !origin.is_null() {
                c["origin"] = origin.clone();
            }
            e.on_text(&c.to_string(), 0);
            assert_eq!(
                e.drain(),
                vec![Out::Close("origin_mismatch".into())],
                "{origin}"
            );
        }
        // An engine that was never told its origin signs nothing either.
        let mut e = Engine::new(DESK, "n", Keypair::generate());
        e.on_connect();
        e.drain();
        e.on_text(&challenge(), 0);
        assert_eq!(e.drain(), vec![Out::Close("origin_mismatch".into())]);
    }

    #[test]
    fn a_bad_challenge_or_version_closes() {
        let mut e = engine_with(vec![]);
        e.on_connect();
        e.drain();
        e.on_text(
            &json!({"t":"challenge","nonce":"AAAA","v":1,"origin":ORIGIN}).to_string(),
            0,
        );
        assert_eq!(e.drain(), vec![Out::Close("bad_challenge".into())]);
        e.on_connect();
        e.drain();
        e.on_text(
            &json!({"t":"challenge","nonce":keys::random_b64u(32),"v":2,"origin":ORIGIN})
                .to_string(),
            0,
        );
        assert_eq!(e.drain(), vec![Out::Close("version".into())]);
    }

    #[test]
    fn server_errors_during_the_handshake_close_with_a_safe_code() {
        let mut e = engine_with(vec![]);
        e.on_connect();
        e.drain();
        e.on_text(&json!({"t":"error","code":"bad_token"}).to_string(), 0);
        assert_eq!(e.drain(), vec![Out::Close("bad_token".into())]);
        e.on_connect();
        e.drain();
        e.on_text(&json!({"t":"error","code":"<script>X\n"}).to_string(), 0);
        assert_eq!(e.drain(), vec![Out::Close("script".into())]);
    }

    #[test]
    fn frames_out_of_phase_are_ignored() {
        let phone = Keypair::generate();
        let mut e = engine_with(vec![(device(DEV, &phone, "prompt"), link())]);
        // Not even connected.
        e.on_text(
            &json!({"t":"device_online","device_id":DEV,"conn":"c"}).to_string(),
            0,
        );
        e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":"c","body":"{}","sig":""}).to_string(),
            0,
        );
        assert!(e.drain().is_empty());
        // Connected but before ready.
        e.on_connect();
        e.drain();
        e.on_text(
            &json!({"t":"device_online","device_id":DEV,"conn":"c"}).to_string(),
            0,
        );
        e.on_text(&json!({"t":"ready","devices":[]}).to_string(), 0);
        assert!(e.drain().is_empty());
        assert!(!e.connected());
    }

    #[test]
    fn ready_removes_devices_only_the_server_knows() {
        let mut e = engine_with(vec![]);
        e.on_connect();
        e.on_text(&challenge(), 0);
        e.drain();
        e.on_text(&json!({"t":"ready","devices":[DEV]}).to_string(), 0);
        let f = frames(&e.drain());
        assert_eq!(f, vec![json!({"t":"device_remove","device_id":DEV})]);
    }

    #[test]
    fn ready_keeps_a_device_the_config_holds_but_cannot_use_this_run() {
        let mut e = engine_with(vec![]);
        e.set_unusable(vec![DEV.to_string()]);
        e.on_connect();
        e.on_text(&challenge(), 0);
        e.drain();
        e.on_text(&json!({"t":"ready","devices":[DEV, DEV2]}).to_string(), 0);
        // Only the id the config does not contain at all is removed.
        let f = frames(&e.drain());
        assert_eq!(f, vec![json!({"t":"device_remove","device_id":DEV2})]);
    }

    // ------------------------------------------------------------- sessions

    #[test]
    fn a_skipped_seq_revokes_the_session_answers_bad_session_and_is_one_audit_line() {
        let mut r = Rig::new("view");
        let first = r.session.clone();
        for _ in 0..2 {
            let out = r.cmd(json!({"op":"list"}), NOW);
            r.e.frontend_reply(request_id(&out), &json!({"panes":[]}), NOW);
            r.e.drain();
        }
        // The relay drops seq 3; 4 arrives.
        r.seq += 1;
        let out = r.cmd(json!({"op":"list"}), NOW);
        assert!(!has_request(&out), "the gap command never runs");
        let b = r.opened(&out);
        assert_eq!(b[0]["code"], "bad_session");
        assert_eq!(b[0]["re"], 0);
        assert!(r.e.sessions.current(DEV).is_none());
        // Opening again says exactly what was accepted: 1 and 2, not 3 or 4.
        let out = r.open(NOW + 1_000);
        assert_eq!(
            r.opened(&out)[0]["prev"],
            json!({"session": first, "seq": 2})
        );
        // Repeating it yields one aggregated line, never one per frame.
        r.e.tick(NOW + 70_000);
        let lines = audits(&r.e.drain());
        assert_eq!(lines.iter().filter(|l| l.code == "seq_gap").count(), 1);
    }

    #[test]
    fn accepted_reads_are_one_counted_line_but_every_write_has_its_own() {
        let mut r = Rig::new("prompt");
        for _ in 0..5 {
            let out = r.cmd(json!({"op":"peek","pane":PANE}), NOW);
            r.e.frontend_reply(request_id(&out), &json!({"lines":[],"menu":[],"menu_id":"","question":[],"blocked":null,"status":"idle"}), NOW);
        }
        let mut lines = audits(&r.e.drain());
        assert!(lines.is_empty(), "no line per read yet: {lines:?}");
        for _ in 0..2 {
            let out = r.cmd(json!({"op":"interrupt","pane":PANE}), NOW);
            r.e.frontend_reply(request_id(&out), &json!({"result":"sent"}), NOW);
            lines.extend(audits(&r.e.drain()));
        }
        assert_eq!(lines.len(), 2, "one line per accepted write");
        assert!(lines.iter().all(|l| l.op == "interrupt" && l.code == "ok"));
        r.e.tick(NOW + 61_000);
        let agg = audits(&r.e.drain());
        assert_eq!(agg.len(), 1);
        assert_eq!((agg[0].op, agg[0].code, agg[0].count), ("peek", "ok", 5));
    }

    #[test]
    fn device_online_creates_no_session_and_sends_nothing() {
        let mut r = Rig::bare("view");
        // The hint is shown...
        assert!(r.e.online_devices().contains(DEV));
        // ...but a command (sealed and signed correctly) finds no session.
        r.session = "guessed".into();
        let out = r.cmd(json!({"op":"list"}), NOW);
        assert!(!has_request(&out));
        let b = r.opened(&out);
        assert_eq!(b[0]["code"], "bad_session");
        assert_eq!(b[0]["re"], 0);
        assert_eq!(b[0]["session"], "");
    }

    #[test]
    fn device_online_alone_sends_no_frame_at_all() {
        let phone = Keypair::generate();
        let mut e = engine_with(vec![(device(DEV, &phone, "view"), link())]);
        handshake(&mut e);
        e.on_text(
            &json!({"t":"device_online","device_id":DEV,"conn":CONN}).to_string(),
            NOW,
        );
        assert!(frames(&e.drain()).is_empty());
        // Nor does an unknown device's hint count.
        e.on_text(
            &json!({"t":"device_online","device_id":DEV2,"conn":"x"}).to_string(),
            NOW,
        );
        e.drain();
        assert!(!e.online_devices().contains(DEV2));
    }

    #[test]
    fn open_issues_a_session_that_echoes_the_nonce_and_carries_the_clock() {
        let mut r = Rig::bare("respond");
        let out = r.open(NOW + 5);
        let b = r.opened(&out);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0]["k"], "session");
        assert_eq!(b[0]["open"], OPEN_NONCE);
        assert_eq!(b[0]["cap"], "respond");
        assert_eq!(b[0]["name"], "My PC");
        assert_eq!(b[0]["now_ms"], NOW + 5);
        assert_eq!(b[0]["v"], 1);
        assert!(!r.session.is_empty());
        // A first session revoked nothing.
        assert_eq!(b[0]["prev"], json!({"session":"","seq":0}));
        // The session reply itself carries no `n`; the next body is n = 1.
        assert!(b[0].get("n").is_none());
        let out = r.cmd(json!({"op":"list"}), NOW + 6);
        let id = request_id(&out);
        r.e.frontend_reply(id, &json!({"panes":[]}), NOW + 7);
        let out = r.e.drain();
        assert_eq!(r.reply(&out)["n"], 1);
    }

    #[test]
    fn open_from_an_unpaired_or_unsigned_sender_gets_nothing() {
        let mut r = Rig::bare("view");
        // A stranger signs and seals with keys the desktop never stored.
        let stranger = Keypair::generate();
        let body = r.sealed(&json!({"k":"open","desktop_id":DESK,"device_id":DEV,
                                    "nonce":OPEN_NONCE}));
        let sig = stranger.sign(&proto::cmd_signed(&body));
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":body,"sig":sig}).to_string(),
            NOW,
        );
        let out = r.e.drain();
        // Known device, bad signature: the sealed bad_sig, no session.
        assert_eq!(r.opened(&out)[0]["code"], "bad_sig");
        assert!(r.e.sessions.current(DEV).is_none());
        // An id nobody paired: dropped silently.
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV2,"conn":CONN,"body":body,"sig":sig}).to_string(),
            NOW,
        );
        assert!(frames(&r.e.drain()).is_empty());
    }

    #[test]
    fn a_newer_open_revokes_the_older_session_and_kills_held_commands() {
        let mut r = Rig::new("prompt");
        let old = r.session.clone();
        // The relay holds back a validly signed, sealed command...
        let held = r.cmd_plain(json!({"op":"list"}), NOW);
        let held_frame = r.frame_for(&r.sealed(&held));
        // ...the device opens a new session...
        r.open(NOW + 1_000);
        assert_ne!(r.session, old);
        // ...and the held one, delivered later, runs nothing.
        r.e.on_text(&held_frame, NOW + 2_000);
        let out = r.e.drain();
        assert!(!has_request(&out));
        let b = r.opened(&out);
        assert_eq!(b[0]["code"], "bad_session");
        // The new session still works.
        let out = r.cmd(json!({"op":"list"}), NOW + 3_000);
        assert!(has_request(&out));
    }

    #[test]
    fn a_held_yes_cannot_answer_a_later_question() {
        // The reviews' top finding: "1 / Yes" withheld, delivered hours later.
        let mut r = Rig::new("respond");
        let held = r.cmd_plain(
            json!({"op":"answer","pane":PANE,"n":1,"label":"Yes","menu_id":"m-old"}),
            NOW,
        );
        let held_frame = r.frame_for(&r.sealed(&held));
        // Hours pass; the phone reconnects and opens a new session.
        let later = NOW + 4 * 3_600_000;
        r.open(later);
        r.e.on_text(&held_frame, later + 10);
        let out = r.e.drain();
        assert!(!has_request(&out));
        assert_eq!(r.opened(&out)[0]["code"], "bad_session");
        // And with no new session at all it is merely stale.
        let mut r = Rig::new("respond");
        let held = r.cmd_plain(
            json!({"op":"answer","pane":PANE,"n":1,"label":"Yes","menu_id":"m-old"}),
            NOW,
        );
        let held_frame = r.frame_for(&r.sealed(&held));
        r.e.on_text(&held_frame, NOW + 30_001);
        let out = r.e.drain();
        assert!(!has_request(&out));
        assert_eq!(r.opened(&out)[0]["code"], "expired");
    }

    #[test]
    fn device_offline_ends_its_session() {
        let mut r = Rig::new("view");
        r.e.on_text(
            &json!({"t":"device_offline","device_id":DEV,"conn":CONN}).to_string(),
            NOW,
        );
        r.e.drain();
        assert!(r.e.online_devices().is_empty());
        let out = r.cmd(json!({"op":"list"}), NOW);
        assert_eq!(r.opened(&out)[0]["code"], "bad_session");
    }

    #[test]
    fn offline_of_an_old_connection_keeps_a_session_while_another_is_online() {
        let mut r = Rig::new("view");
        r.e.on_text(
            &json!({"t":"device_online","device_id":DEV,"conn":"c2"}).to_string(),
            NOW,
        );
        r.e.on_text(
            &json!({"t":"device_offline","device_id":DEV,"conn":CONN}).to_string(),
            NOW,
        );
        r.e.drain();
        let out = r.cmd(json!({"op":"list"}), NOW);
        assert!(has_request(&out));
    }

    #[test]
    fn a_session_is_unusable_after_the_desktop_reconnects() {
        let mut r = Rig::new("view");
        r.e.on_disconnect(NOW);
        r.e.on_connect();
        r.e.on_text(&challenge(), NOW);
        r.e.on_text(&json!({"t":"ready","devices":[]}).to_string(), NOW);
        r.e.drain();
        assert!(r.e.online_devices().is_empty());
        let out = r.cmd(json!({"op":"list"}), NOW);
        assert!(!has_request(&out));
        assert_eq!(r.opened(&out)[0]["code"], "bad_session");
        // The device opens again and is back.
        r.open(NOW + 1);
        assert!(has_request(&r.cmd(json!({"op":"list"}), NOW + 2)));
    }

    #[test]
    fn opens_are_rate_limited_and_the_flood_is_one_audit_line() {
        let mut r = Rig::bare("view");
        let open = json!({"k":"open","desktop_id":DESK,"device_id":DEV,"nonce":OPEN_NONCE});
        let mut answered = 0;
        for i in 0..20 {
            let out = r.send_plain(&open, NOW + i);
            answered += r.opened(&out).len();
            // Never one audit line per replayed frame.
            assert!(audits(&out).is_empty());
        }
        assert_eq!(answered, 5, "at most 5 per 10 s");
        r.e.tick(NOW + 30_000);
        assert!(audits(&r.e.drain()).is_empty(), "the window is still open");
        r.e.tick(NOW + 61_000);
        let a = audits(&r.e.drain());
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].code, a[0].count), ("open_limited", 15));
        // After the window the device can open again.
        let out = r.send_plain(&open, NOW + 120_000);
        assert_eq!(r.opened(&out).len(), 1);
    }

    #[test]
    fn replayed_frames_are_one_audit_line_and_a_small_reply_budget() {
        let mut r = Rig::new("view");
        let plain = r.cmd_plain(json!({"op":"list"}), NOW);
        let frame = r.frame_for(&r.sealed(&plain));
        r.e.on_text(&frame, NOW);
        let first = r.e.drain();
        assert!(has_request(&first), "the first copy runs");
        r.e.frontend_reply(request_id(&first), &json!({"panes": []}), NOW);
        r.e.drain();
        let (mut replies, mut lines, mut requests) = (0, 0, 0);
        for i in 0..200 {
            r.e.on_text(&frame, NOW + i);
            let out = r.e.drain();
            replies += r.opened(&out).len();
            lines += audits(&out).len();
            requests += usize::from(has_request(&out));
        }
        assert_eq!(requests, 0, "a replay never reaches the app");
        assert_eq!(lines, 0, "no audit line per replayed frame");
        assert_eq!(replies, FAIL_REPLIES_PER_WINDOW as usize);
        r.e.tick(NOW + 30_000);
        assert!(audits(&r.e.drain()).is_empty(), "the window is still open");
        r.e.tick(NOW + 61_000);
        let all = audits(&r.e.drain());
        // The one accepted `list` is an aggregated read line of its own.
        let reads: Vec<_> = all.iter().filter(|e| e.code == "ok").collect();
        assert_eq!(reads.len(), 1);
        assert_eq!((reads[0].op, reads[0].count), ("list", 1));
        let a: Vec<_> = all.iter().filter(|e| e.code != "ok").collect();
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].code, a[0].count), ("replay", 200));
        assert_eq!(a[0].device, DEV);
        // A new window: the budget is back.
        r.e.on_text(&frame, NOW + 130_000);
        let out = r.e.drain();
        assert_eq!(r.opened(&out).len(), 1);
    }

    #[test]
    fn a_malformed_open_is_refused_and_does_not_use_the_allowance() {
        let mut r = Rig::bare("view");
        for bad in [
            json!({"k":"open","desktop_id":DESK,"device_id":DEV}),
            json!({"k":"open","desktop_id":DESK,"device_id":DEV,"nonce":"x"}),
            json!({"k":"open","desktop_id":"other","device_id":DEV,"nonce":OPEN_NONCE}),
            json!({"k":"open","desktop_id":DESK,"device_id":DEV2,"nonce":OPEN_NONCE}),
        ] {
            let out = r.send_plain(&bad, NOW);
            assert_eq!(r.opened(&out)[0]["code"], "bad_request", "{bad}");
        }
        assert!(r.e.sessions.current(DEV).is_none());
        assert_eq!(frames(&r.open(NOW)).len(), 1);
        assert!(r.e.sessions.current(DEV).is_some());
    }

    // ------------------------------------------------------- command checks

    #[test]
    fn list_goes_to_the_app_and_the_sealed_answer_is_relayed() {
        let mut r = Rig::new("view");
        let out = r.cmd(json!({"op":"list"}), NOW);
        let id = request_id(&out);
        let payload = out
            .iter()
            .find_map(|o| match o {
                Out::Request { payload, .. } => Some(payload.clone()),
                _ => None,
            })
            .unwrap();
        // The deadline is the moment the backend answers `timeout` (an epoch time).
        assert_eq!(
            payload,
            json!({"id":id,"op":"list","deadline_ms": NOW + REPLY_TIMEOUT_MS})
        );
        // Nothing is sent to the phone until the app answers.
        assert!(frames(&out).is_empty());
        r.e.frontend_reply(
            id,
            &json!({"panes":[{"pane":PANE,"ws":"w","title":"t","kind":"claude",
            "status":"waiting","since_ms":5,"queued":0}]}),
            NOW + 500,
        );
        let out = r.e.drain();
        // The frame on the wire is opaque.
        let wire = frames(&out)[0].to_string();
        assert!(!wire.contains("waiting") && !wire.contains("panes"));
        let reply = r.reply(&out);
        assert_eq!(reply["k"], "reply");
        assert_eq!(reply["re"], 1);
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["data"]["panes"][0]["status"], "waiting");
    }

    #[test]
    fn n_strictly_increases_across_replies_panes_and_reopen() {
        let mut r = Rig::new("prompt");
        let mut seen = Vec::new();
        for i in 0..3 {
            let out = r.cmd(json!({"op":"list"}), NOW + i);
            let id = request_id(&out);
            r.e.frontend_reply(id, &json!({"panes":[]}), NOW + i);
            let out = r.e.drain();
            seen.push(r.reply(&out)["n"].as_u64().unwrap());
        }
        r.e.publish_panes(&json!([pane_json(PANE, "working")]), NOW + 10_000);
        let out = r.e.drain();
        let b = r.reply(&out);
        assert_eq!(b["k"], "panes");
        seen.push(b["n"].as_u64().unwrap());
        let mut d = device(DEV, &r.phone, "view");
        d.added_ms = 1;
        r.e.set_devices(vec![(d, link())]);
        r.e.device_cap_changed(DEV, NOW + 11_000);
        let out = r.e.drain();
        let b = r.reply(&out);
        assert_eq!(b["k"], "reopen");
        seen.push(b["n"].as_u64().unwrap());
        assert_eq!(seen, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn the_checks_run_in_order_and_each_failure_is_a_sealed_reply() {
        // expired
        let mut r = Rig::new("view");
        let out = r.cmd_ts(json!({"op":"list"}), NOW - 30_001, NOW);
        assert!(!has_request(&out));
        let b = r.opened(&out);
        assert_eq!(b[0]["code"], "expired");
        assert_eq!(b[0]["re"], 1);
        // replay (and the expired one consumed its seq)
        let again = r.cmd_plain(json!({"op":"list"}), NOW);
        let mut again = again;
        again["seq"] = 1.into();
        let out = r.send_plain(&again, NOW);
        assert_eq!(r.opened(&out)[0]["code"], "replay");
        // None of the rejections below consumes a seq: the next real command
        // is 2 (a skipped number would end the session).
        r.seq = 1;
        // wrong session
        let mut bad = r.cmd_plain(json!({"op":"list"}), NOW);
        bad["session"] = "nope".into();
        let out = r.send_plain(&bad, NOW);
        let b = r.opened(&out);
        assert_eq!(b[0]["code"], "bad_session");
        assert_eq!(b[0]["re"], 0);
        r.seq = 1;
        // wrong desktop id
        let mut bad = r.cmd_plain(json!({"op":"list"}), NOW);
        bad["desktop_id"] = DEV2.into();
        let out = r.send_plain(&bad, NOW);
        assert_eq!(r.opened(&out)[0]["code"], "bad_request");
        r.seq = 1;
        // capability
        let out = r.cmd(json!({"op":"prompt","pane":PANE,"text":"hi"}), NOW);
        assert_eq!(r.opened(&out)[0]["code"], "forbidden");
        // unknown op
        let out = r.cmd(json!({"op":"keys","pane":PANE}), NOW);
        assert_eq!(r.opened(&out)[0]["code"], "unknown_op");
        // not a command, not an open
        let out = r.send_plain(&json!({"k":"reply"}), NOW);
        assert_eq!(r.opened(&out)[0]["code"], "bad_request");
    }

    impl Rig {
        fn cmd_ts(&mut self, cmd: Value, ts: u64, now: u64) -> Vec<Out> {
            let plain = self.cmd_plain(cmd, ts);
            self.send_plain(&plain, now)
        }
    }

    #[test]
    fn rejections_are_counted_without_reaching_the_app() {
        let mut r = Rig::new("view");
        let out = r.cmd(json!({"op":"prompt","pane":PANE,"text":"rm -rf /"}), NOW);
        assert!(!has_request(&out));
        assert_eq!(r.opened(&out)[0]["code"], "forbidden");
        // Counted, not one line per rejection.
        assert!(audits(&out).is_empty());
        r.e.tick(NOW + 61_000);
        let a = audits(&r.e.drain());
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].code, a[0].count), ("forbidden", 1));
        assert_eq!(a[0].device, DEV);
        assert!(!format!("{:?}", a[0]).contains("rm -rf"));
    }

    // -------------------------------------------- audit aggregation classes

    /// Lines the engine writes for `noise`, run at `NOW`, flushed a minute on.
    fn aggregated(r: &mut Rig, noise: impl FnOnce(&mut Rig)) -> Vec<Entry> {
        r.e.drain();
        noise(r);
        let immediate = audits(&r.e.drain());
        assert!(immediate.is_empty(), "no line per rejection: {immediate:?}");
        r.e.tick(NOW + 61_000);
        audits(&r.e.drain())
    }

    #[test]
    fn forbidden_streamed_by_a_low_capability_device_is_one_line() {
        let mut r = Rig::new("view");
        let a = aggregated(&mut r, |r| {
            for _ in 0..300 {
                r.cmd(json!({"op":"prompt","pane":PANE,"text":"hi"}), NOW);
            }
        });
        assert_eq!(a.len(), 1);
        assert_eq!(
            (a[0].code, a[0].count, a[0].device.as_str()),
            ("forbidden", 300, DEV)
        );
    }

    #[test]
    fn every_rejection_a_device_can_repeat_is_aggregated() {
        let mut r = Rig::new("prompt");
        let a = aggregated(&mut r, |r| {
            for _ in 0..40 {
                r.cmd(json!({"op":"keys","pane":PANE}), NOW); // unknown_op
                r.cmd(json!({"op":"peek","pane":"nope"}), NOW); // bad_request
                r.cmd(json!({"op":"prompt","pane":PANE,"text":"!ls"}), NOW); // refused
                r.send_plain(&json!({"k":"reply"}), NOW); // bad_request (not a command)
            }
        });
        let by = |c: &str| a.iter().find(|e| e.code == c).map(|e| e.count);
        assert_eq!(by("unknown_op"), Some(40));
        assert_eq!(by("refused"), Some(40));
        // The device's two bad_request kinds land in one line.
        assert_eq!(by("bad_request"), Some(80));
        assert_eq!(a.len(), 3, "{a:?}");
    }

    #[test]
    fn busy_is_aggregated_too() {
        let mut r = Rig::new("view");
        let a = aggregated(&mut r, |r| {
            for _ in 0..(MAX_INFLIGHT + 25) {
                r.cmd(json!({"op":"list"}), NOW);
            }
        });
        // (The eight waiting commands time out at the flush: individual lines,
        // they were accepted.)
        let busy: Vec<_> = a.iter().filter(|e| e.code == "busy").collect();
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0].count, 25);
        assert!(a.iter().all(|e| e.code == "busy" || e.code == "timeout"));
    }

    #[test]
    fn pair_requests_with_no_window_are_aggregated_and_not_answered() {
        let mut e = engine_with(vec![]);
        handshake(&mut e);
        let phone = new_phone();
        let mut sent = 0;
        for i in 0..500u64 {
            let (pk, ek) = (phone.sign.public_b64u(), phone.ek.public_b64u());
            // A different claimed id each time.
            let id = format!("00000000-0000-4000-8000-{i:012}");
            e.on_text(
                &json!({"t":"pair_request","device_id":id,"pubkey":pk,"ek":ek,"name":"x",
                    "proof":"AAAA"})
                .to_string(),
                NOW + i,
            );
            let out = e.drain();
            sent += frames(&out).len();
            assert!(audits(&out).is_empty());
        }
        assert_eq!(sent, 0, "nothing to answer without a window");
        e.tick(NOW + 61_000);
        let a = audits(&e.drain());
        assert_eq!(a.len(), 1, "{a:?}");
        assert_eq!(
            (a[0].code, a[0].count, a[0].device.as_str()),
            ("no_window", 500, "")
        );
    }

    #[test]
    fn invalid_proofs_are_aggregated_whatever_id_they_claim() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let good = w.code.clone();
        // The window answers at most MAX_REQUESTS of them; the rest are
        // `budget`, also counted. Claimed ids never become keys.
        for i in 0..30u64 {
            w.code = keys::b64u(&keys::random_bytes(16));
            let id = format!("00000000-0000-4000-8000-{i:012}");
            let out = pair_request_as(&mut w, &phone, &id, 2_000 + i);
            assert!(audits(&out).is_empty());
        }
        w.code = good;
        w.e.tick(NOW);
        let a = audits(&w.e.drain());
        assert!(a.iter().all(|e| e.device.is_empty()), "{a:?}");
        let total: u32 = a.iter().map(|e| e.count).sum();
        assert_eq!(total, 30);
        assert_eq!(
            a.iter().find(|e| e.code == "bad_proof").map(|e| e.count),
            Some(5)
        );
        assert_eq!(
            a.iter().find(|e| e.code == "budget").map(|e| e.count),
            Some(25)
        );
    }

    #[test]
    fn a_wrong_check_is_audited_without_the_number() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let number = request_and_number(&mut w, &phone, DEV, 2_000);
        w.e.pair_decide(
            DEV,
            true,
            "view",
            wrong_number(&number),
            3_000,
            &mut |_, _| true,
        )
        .unwrap();
        let a = audits(&w.e.drain());
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].op, a[0].code), ("pair", "wrong_check"));
        assert!(!format!("{:?}", a[0]).contains(&number));
    }

    #[test]
    fn app_text_is_redacted_and_clipped_before_it_is_sealed() {
        let mut r = Rig::new("view");
        let out = r.cmd(json!({"op":"peek","pane":PANE,"lines":10}), NOW);
        let id = request_id(&out);
        let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEF";
        r.e.frontend_reply(
            id,
            &json!({"lines":[format!("token {key}"), "lorem ipsum ".repeat(300),
                             "hidden\u{202E}text\u{200B}!"],
                    "menu":[], "menu_id":"mid-1", "question":[], "blocked":null,
                    "status":"working"}),
            NOW + 10,
        );
        let out = r.e.drain();
        let reply = r.reply(&out);
        let raw = reply.to_string();
        assert!(!raw.contains("abcdefghijklmnop"), "{raw}");
        assert_eq!(
            reply["data"]["lines"][1].as_str().unwrap().chars().count(),
            2000
        );
        assert_eq!(reply["data"]["lines"][2], "hiddentext!");
        assert_eq!(reply["data"]["menu_id"], "mid-1");
    }

    #[test]
    fn answer_passes_the_menu_id_to_the_page_and_limits_n() {
        let mut r = Rig::new("respond");
        let out = r.cmd(
            json!({"op":"answer","pane":PANE,"n":2,"label":"No","menu_id":"abc_-9"}),
            NOW,
        );
        let payload = out
            .iter()
            .find_map(|o| match o {
                Out::Request { payload, .. } => Some(payload.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(payload["menu_id"], "abc_-9");
        assert_eq!(payload["n"], 2);
        for bad in [
            json!({"op":"answer","pane":PANE,"n":10,"label":"x","menu_id":"m"}),
            json!({"op":"answer","pane":PANE,"n":0,"label":"x","menu_id":"m"}),
            json!({"op":"answer","pane":PANE,"n":1,"label":"x"}),
            json!({"op":"answer","pane":PANE,"n":1,"label":"x","menu_id":"has space"}),
            json!({"op":"answer","pane":PANE,"n":1,"label":"x","menu_id":"y".repeat(33)}),
        ] {
            let out = r.cmd(bad.clone(), NOW);
            assert!(!has_request(&out), "{bad}");
            assert_eq!(r.opened(&out)[0]["code"], "bad_request", "{bad}");
        }
    }

    #[test]
    fn a_bang_prompt_is_refused_in_rust_without_asking_the_page() {
        let mut r = Rig::new("prompt");
        for text in ["!rm -rf /", "  \n\t!ls", "\u{200B}!x", "\x1b!ls"] {
            let out = r.cmd(json!({"op":"prompt","pane":PANE,"text":text}), NOW);
            assert!(!has_request(&out), "{text:?}");
            let reply = r.reply(&out);
            assert_eq!(reply["ok"], true);
            assert_eq!(reply["data"], json!({"result":"refused"}), "{text:?}");
            // Counted (one line per minute), not one line each.
            assert!(audits(&out).is_empty());
        }
        r.e.tick(NOW + 61_000);
        let a = audits(&r.e.drain());
        assert_eq!((a[0].code, a[0].count), ("refused", 4));
        // A bang later in the text is just text.
        let out = r.cmd(json!({"op":"prompt","pane":PANE,"text":"say hi!"}), NOW);
        assert!(has_request(&out));
    }

    #[test]
    fn prompt_text_reaches_the_page_without_controls() {
        let mut r = Rig::new("prompt");
        let out = r.cmd(
            json!({"op":"prompt","pane":PANE,"text":"a\u{1b}[2Jb\r\nc\rd\te","queue":true}),
            NOW,
        );
        let payload = out
            .iter()
            .find_map(|o| match o {
                Out::Request { payload, .. } => Some(payload.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(payload["text"], "a[2Jb\nc\nd\te");
        assert_eq!(payload["queue"], true);
    }

    #[test]
    fn the_page_cannot_make_the_desktop_say_refused() {
        let mut r = Rig::new("prompt");
        let out = r.cmd(json!({"op":"prompt","pane":PANE,"text":"hi"}), NOW);
        let id = request_id(&out);
        r.e.frontend_reply(id, &json!({"result":"refused"}), NOW);
        let out = r.e.drain();
        let reply = r.reply(&out);
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["code"], "internal");
    }

    #[test]
    fn an_invalid_app_answer_becomes_an_internal_error() {
        let mut r = Rig::new("respond");
        let out = r.cmd(
            json!({"op":"answer","pane":PANE,"n":1,"label":"Yes","menu_id":"m"}),
            NOW,
        );
        let id = request_id(&out);
        r.e.frontend_reply(id, &json!({"result":"typed everything"}), NOW + 1);
        let out = r.e.drain();
        let reply = r.reply(&out);
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["code"], "internal");
    }

    #[test]
    fn no_answer_in_5_seconds_is_a_timeout() {
        let mut r = Rig::new("view");
        let out = r.cmd(json!({"op":"list"}), NOW);
        let id = request_id(&out);
        r.e.tick(NOW + 4_999);
        assert!(frames(&r.e.drain()).is_empty());
        r.e.tick(NOW + 5_000);
        let out = r.e.drain();
        let reply = r.reply(&out);
        assert_eq!(reply["re"], 1);
        assert_eq!(reply["code"], "timeout");
        assert!(audits(&out).iter().any(|a| a.code == "timeout"));
        // The late answer is ignored.
        r.e.frontend_reply(id, &json!({"panes":[]}), NOW + 6_000);
        assert!(r.e.drain().is_empty());
    }

    #[test]
    fn a_reply_is_dropped_when_its_session_ended_meanwhile() {
        let mut r = Rig::new("view");
        let out = r.cmd(json!({"op":"list"}), NOW);
        let id = request_id(&out);
        // The device opens a new session while the app is thinking.
        r.open(NOW + 100);
        r.e.frontend_reply(id, &json!({"panes":[]}), NOW + 200);
        let out = r.e.drain();
        assert!(frames(&out).is_empty(), "re numbers belong to one session");
        assert!(audits(&out).iter().any(|a| a.code == "bad_session"));
    }

    #[test]
    fn too_many_commands_waiting_are_answered_busy() {
        let mut r = Rig::new("view");
        for _ in 0..MAX_INFLIGHT {
            let out = r.cmd(json!({"op":"list"}), NOW);
            assert!(has_request(&out));
        }
        let out = r.cmd(json!({"op":"list"}), NOW);
        assert!(!has_request(&out));
        assert_eq!(r.reply(&out)["code"], "busy");
    }

    // ------------------------------------------- frames the relay can forge

    #[test]
    fn a_forged_or_tampered_relay_frame_does_nothing() {
        let mut r = Rig::new("prompt");
        let plain = r.cmd_plain(json!({"op":"prompt","pane":PANE,"text":"hi"}), NOW);
        let body = r.sealed(&plain);
        let sig = r.phone.sign(&proto::cmd_signed(&body));
        // The relay invents a command signed by nobody.
        let stranger = Keypair::generate();
        let bad = stranger.sign(&proto::cmd_signed(&body));
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":body,"sig":bad}).to_string(),
            NOW,
        );
        let out = r.e.drain();
        assert!(!has_request(&out));
        assert_eq!(r.opened(&out)[0]["code"], "bad_sig");
        // One flipped character in the sealed body breaks the signature.
        let mut chars: Vec<char> = body.chars().collect();
        chars[20] = if chars[20] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":tampered,"sig":sig}).to_string(),
            NOW,
        );
        assert!(!has_request(&r.e.drain()));
        // A validly signed body that does not open under the link key
        // (sealed by someone else) is dropped and answered bad_sig.
        let alien = LinkKey::from_bytes([9u8; 32])
            .seal(&proto::aad_p2d(DESK, DEV), plain.to_string().as_bytes())
            .unwrap();
        let alien_sig = r.phone.sign(&proto::cmd_signed(&alien));
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":alien,"sig":alien_sig})
                .to_string(),
            NOW,
        );
        let out = r.e.drain();
        assert!(!has_request(&out));
        assert_eq!(r.opened(&out)[0]["code"], "bad_sig");
        // Sealed for the wrong direction or the wrong device does not open.
        let wrong_aad = r
            .link
            .seal(&proto::aad_d2p(DESK, DEV), plain.to_string().as_bytes())
            .unwrap();
        let s2 = r.phone.sign(&proto::cmd_signed(&wrong_aad));
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":wrong_aad,"sig":s2}).to_string(),
            NOW,
        );
        assert!(!has_request(&r.e.drain()));
        // The genuine frame still works afterwards.
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":body,"sig":sig}).to_string(),
            NOW,
        );
        assert!(has_request(&r.e.drain()));
    }

    #[test]
    fn a_plaintext_command_is_never_accepted() {
        let mut r = Rig::new("prompt");
        let plain = r.cmd_plain(json!({"op":"list"}), NOW).to_string();
        let sig = r.phone.sign(&proto::cmd_signed(&plain));
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":plain,"sig":sig}).to_string(),
            NOW,
        );
        assert!(!has_request(&r.e.drain()));
    }

    #[test]
    fn unknown_device_frames_are_dropped_and_counted_once_a_minute() {
        let mut r = Rig::new("view");
        let body = r.sealed(&json!({"k":"cmd"}));
        for i in 0..500 {
            r.e.on_text(
                &json!({"t":"cmd","device_id":DEV2,"conn":"x","body":body,"sig":"AA"}).to_string(),
                NOW + i,
            );
        }
        let out = r.e.drain();
        assert!(frames(&out).is_empty(), "no reply to a stranger");
        assert!(audits(&out).is_empty());
        r.e.tick(NOW + 61_000);
        let a = audits(&r.e.drain());
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].code, a[0].count), ("unknown_device", 500));
    }

    #[test]
    fn bad_signatures_and_seals_are_counted_not_logged_one_by_one() {
        let mut r = Rig::new("view");
        let stranger = Keypair::generate();
        let body = r.sealed(&json!({"k":"cmd"}));
        let sig = stranger.sign(&proto::cmd_signed(&body));
        for i in 0..50 {
            r.e.on_text(
                &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":body,"sig":sig}).to_string(),
                NOW + i,
            );
        }
        let out = r.e.drain();
        assert!(audits(&out).is_empty());
        assert_eq!(
            r.opened(&out).len(),
            FAIL_REPLIES_PER_WINDOW as usize,
            "the first few get the sealed bad_sig, the rest are dropped silently"
        );
        r.e.on_disconnect(NOW + 1_000);
        let a = audits(&r.e.drain());
        let bad: Vec<_> = a.iter().filter(|a| a.code == "bad_sig").collect();
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].count, 50);
        assert_eq!(bad[0].device, DEV);
    }

    #[test]
    fn a_bad_sig_reply_carries_the_ref_of_the_signature_it_answers() {
        let mut r = Rig::new("view");
        let stranger = Keypair::generate();
        let body = r.sealed(&json!({"k":"cmd"}));
        let sig = stranger.sign(&proto::cmd_signed(&body));
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":body,"sig":sig}).to_string(),
            NOW,
        );
        let out = r.e.drain();
        let b = r.opened(&out).remove(0);
        assert_eq!(b["code"], "bad_sig");
        assert_eq!(b["re"], 0);
        assert_eq!(b["session"], "");
        assert!(b.get("n").is_none(), "no n on a re:0 reply");
        assert!(
            b.get("at_ms").is_none(),
            "nor at_ms (the fixtures' reading of 5.1)"
        );
        // First 16 bytes of SHA-256 of the sig string as received.
        let digest = {
            use sha2::{Digest, Sha256};
            Sha256::digest(sig.as_bytes())
        };
        assert_eq!(b["ref"], keys::b64u(&digest[..16]));
        assert_eq!(b["ref"].as_str().unwrap().len(), 22);
        // A different sig string gives a different ref.
        let other = stranger.sign(&proto::cmd_signed(&format!("{body}x")));
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":body,"sig":other}).to_string(),
            NOW,
        );
        let out = r.e.drain();
        assert_ne!(r.opened(&out)[0]["ref"], b["ref"]);
    }

    #[test]
    fn bad_seal_replies_are_bad_sig_with_a_ref_too() {
        let mut r = Rig::new("view");
        let alien = LinkKey::from_bytes([9u8; 32])
            .seal(&proto::aad_p2d(DESK, DEV), b"{}")
            .unwrap();
        let sig = r.phone.sign(&proto::cmd_signed(&alien));
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":alien,"sig":sig}).to_string(),
            NOW,
        );
        let out = r.e.drain();
        let b = r.opened(&out).remove(0);
        assert_eq!(
            (b["code"].as_str(), b["re"].as_u64()),
            (Some("bad_sig"), Some(0))
        );
        assert_eq!(b["ref"], proto::sig_ref(&sig));
    }

    #[test]
    fn re_zero_replies_for_a_bad_session_carry_the_ref_too() {
        let mut r = Rig::new("view");
        r.session = "guessed".into();
        let plain = r.cmd_plain(json!({"op":"list"}), NOW);
        let frame = r.frame_for(&r.sealed(&plain));
        let sig = serde_json::from_str::<Value>(&frame).unwrap()["sig"]
            .as_str()
            .unwrap()
            .to_string();
        r.e.on_text(&frame, NOW);
        let out = r.e.drain();
        let b = r.opened(&out).remove(0);
        assert_eq!(b["code"], "bad_session");
        assert_eq!(b["ref"], proto::sig_ref(&sig));
        // A reply for a command that did get a `seq` is by `re`, with no ref.
        r.open(NOW + 1);
        let ok = r.cmd(json!({"op":"prompt","pane":PANE,"text":"x"}), NOW + 1);
        let b = r.opened(&ok).remove(0);
        assert_eq!(
            (b["code"].as_str(), b["re"].as_u64()),
            (Some("forbidden"), Some(1))
        );
        assert!(b.get("ref").is_none());
    }

    #[test]
    fn bad_sig_replies_are_budgeted_apart_from_the_other_failure_replies() {
        let mut r = Rig::new("view");
        let stranger = Keypair::generate();
        let body = r.sealed(&json!({"k":"cmd"}));
        let sig = stranger.sign(&proto::cmd_signed(&body));
        let bad = json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":body,"sig":sig}).to_string();
        let mut replies = 0;
        for i in 0..100 {
            r.e.on_text(&bad, NOW + i);
            let out = r.e.drain();
            replies += r.opened(&out).len();
        }
        assert_eq!(replies, 20);
        // The budget is per device and per minute: a later minute gets 20 more.
        r.e.on_text(&bad, NOW + 61_000);
        let out = r.e.drain();
        assert_eq!(r.opened(&out).len(), 1);
        // Using it up does not take the replay budget with it.
        let plain = r.cmd_plain(json!({"op":"list"}), NOW + 61_000);
        let frame = r.frame_for(&r.sealed(&plain));
        r.e.on_text(&frame, NOW + 61_000);
        assert!(has_request(&r.e.drain()));
        r.e.on_text(&frame, NOW + 61_001);
        let out = r.e.drain();
        assert_eq!(r.opened(&out)[0]["code"], "replay");
    }

    #[test]
    fn every_body_after_the_session_carries_at_ms() {
        let mut r = Rig::new("prompt");
        let at = NOW + 7_000;
        let out = r.cmd(json!({"op":"list"}), at);
        let id = request_id(&out);
        r.e.frontend_reply(id, &json!({"panes":[]}), at + 5);
        let out = r.e.drain();
        let b = r.reply(&out);
        assert_eq!(b["at_ms"], at + 5, "a reply");
        r.e.publish_panes(&json!([pane_json(PANE, "working")]), at + 2_000);
        let out = r.e.drain();
        let b = r.reply(&out);
        assert_eq!(
            (b["k"].as_str(), b["at_ms"].as_u64()),
            (Some("panes"), Some(at + 2_000))
        );
        r.e.set_devices(vec![(device(DEV, &r.phone, "view"), link())]);
        r.e.device_cap_changed(DEV, at + 3_000);
        let out = r.e.drain();
        let b = r.reply(&out);
        assert_eq!(
            (b["k"].as_str(), b["at_ms"].as_u64()),
            (Some("reopen"), Some(at + 3_000))
        );
        // A failure reply too.
        let mut r = Rig::new("view");
        let out = r.cmd(json!({"op":"prompt","pane":PANE,"text":"x"}), at);
        assert_eq!(r.opened(&out)[0]["at_ms"], at);
    }

    #[test]
    fn a_new_session_reports_what_the_revoked_one_accepted() {
        let mut r = Rig::new("view");
        let first = r.session.clone();
        // Three commands accepted in the first session, then a fourth that
        // the relay dropped (never arrived).
        for _ in 0..3 {
            let out = r.cmd(json!({"op":"list"}), NOW);
            r.e.frontend_reply(request_id(&out), &json!({"panes":[]}), NOW);
            r.e.drain();
        }
        r.seq += 1;
        // The device opens again (it lost an answer).
        let out = r.open(NOW + 1_000);
        let b = r.opened(&out);
        assert_eq!(b[0]["prev"], json!({"session": first, "seq": 3}));
        // A capability change ends this one with `reopen`; the next open says so.
        let second = r.session.clone();
        r.e.set_devices(vec![(device(DEV, &r.phone, "prompt"), link())]);
        r.e.device_cap_changed(DEV, NOW + 2_000);
        r.e.drain();
        let out = r.open(NOW + 3_000);
        assert_eq!(
            r.opened(&out)[0]["prev"],
            json!({"session": second, "seq": 0})
        );
    }

    #[test]
    fn prev_survives_the_desktop_reconnecting() {
        let mut r = Rig::new("view");
        let first = r.session.clone();
        let out = r.cmd(json!({"op":"list"}), NOW);
        r.e.frontend_reply(request_id(&out), &json!({"panes":[]}), NOW);
        r.e.drain();
        r.e.on_disconnect(NOW + 1);
        r.e.on_connect();
        r.e.on_text(&challenge(), NOW + 2);
        r.e.on_text(&json!({"t":"ready","devices":[]}).to_string(), NOW + 2);
        r.e.drain();
        let out = r.open(NOW + 3);
        assert_eq!(
            r.opened(&out)[0]["prev"],
            json!({"session": first, "seq": 1})
        );
    }

    #[test]
    fn the_session_reply_is_still_the_only_body_without_n() {
        let mut r = Rig::bare("view");
        let out = r.open(NOW);
        let b = r.opened(&out);
        assert!(b[0].get("n").is_none());
        assert!(b[0].get("at_ms").is_none(), "it carries now_ms instead");
    }

    #[test]
    fn an_oversize_body_is_dropped_before_any_crypto() {
        let mut r = Rig::new("view");
        let big = "A".repeat(proto::MAX_SEALED + 1);
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":big,"sig":"AA"}).to_string(),
            NOW,
        );
        assert!(frames(&r.e.drain()).is_empty());
    }

    // -------------------------------------------------------------- devices

    #[test]
    fn capability_change_sends_reopen_then_ends_the_session() {
        let mut r = Rig::new("view");
        let old = r.session.clone();
        r.e.set_devices(vec![(device(DEV, &r.phone, "prompt"), link())]);
        r.e.device_cap_changed(DEV, NOW + 5);
        let out = r.e.drain();
        assert!(audits(&out).iter().any(|a| a.op == "cap"));
        let b = r.reply(&out);
        assert_eq!(b["k"], "reopen");
        assert_eq!(b["session"], old.as_str());
        // The old session is dead.
        let out = r.cmd(json!({"op":"list"}), NOW + 6);
        assert_eq!(r.opened(&out)[0]["code"], "bad_session");
        // The device opens again and is served at the new level.
        let out = r.open(NOW + 7);
        assert_eq!(r.opened(&out)[0]["cap"], "prompt");
        let out = r.cmd(json!({"op":"prompt","pane":PANE,"text":"hi"}), NOW + 8);
        assert!(has_request(&out));
    }

    #[test]
    fn a_cap_change_without_a_session_sends_nothing() {
        let mut r = Rig::bare("view");
        r.e.set_devices(vec![(device(DEV, &r.phone, "prompt"), link())]);
        r.e.device_cap_changed(DEV, NOW);
        assert!(frames(&r.e.drain()).is_empty());
    }

    #[test]
    fn removing_a_device_tells_the_server_and_kills_the_session() {
        let mut r = Rig::new("prompt");
        r.e.set_devices(vec![]);
        r.e.device_removed(DEV, NOW + 9);
        let f = frames(&r.e.drain());
        assert_eq!(f, vec![json!({"t":"device_remove","device_id":DEV})]);
        // Anything it sends now is from nobody.
        let plain = r.cmd_plain(json!({"op":"list"}), NOW + 10);
        let out = r.send_plain(&plain, NOW + 10);
        assert!(frames(&out).is_empty());
        assert!(r.e.sessions.current(DEV).is_none());
    }

    // ------------------------------------------------------------- pairing

    /// A phone's keys: the one it signs commands with and its `ek`.
    struct Phone {
        sign: Keypair,
        ek: keys::EcdhKey,
    }

    fn new_phone() -> Phone {
        Phone {
            sign: Keypair::generate(),
            ek: keys::EcdhKey::generate(),
        }
    }

    /// An engine with an open window: the engine, the code and `dk`.
    struct Window {
        e: Engine,
        code: String,
        dk: String,
    }

    fn pairing_rig() -> Window {
        let mut e = engine_with(vec![]);
        handshake(&mut e);
        let (code, dk, expires) = e.pair_open(1_000).unwrap();
        assert_eq!(expires, 121_000);
        assert_eq!(
            frames(&e.drain()),
            vec![json!({"t":"pair_open","ttl_s":120})]
        );
        Window { e, code, dk }
    }

    fn pair_request_as(w: &mut Window, phone: &Phone, device_id: &str, now: u64) -> Vec<Out> {
        let code = keys::b64u_decode(&w.code).unwrap();
        let (pk, ek) = (phone.sign.public_b64u(), phone.ek.public_b64u());
        w.e.on_text(
            &json!({"t":"pair_request","device_id":device_id,"pubkey":pk,"ek":ek,
                "name":"iPhone","proof":pair_proof(&code, DESK, device_id, &pk, &ek)})
            .to_string(),
            now,
        );
        w.e.drain()
    }

    fn pair_request(w: &mut Window, phone: &Phone, now: u64) -> Vec<Out> {
        pair_request_as(w, phone, DEV, now)
    }

    /// The key the phone derives from its `ek` and the QR's code and `dk`.
    fn phone_link(w: &Window, phone: &Phone, device_id: &str) -> LinkKey {
        let (pk, ek) = (phone.sign.public_b64u(), phone.ek.public_b64u());
        LinkKey::derive(
            &phone.ek,
            &w.dk,
            &keys::b64u_decode(&w.code).unwrap(),
            &keys::LinkParams {
                desktop_id: DESK,
                device_id,
                pubkey: &pk,
                ek: &ek,
                dk: &w.dk,
            },
        )
        .unwrap()
    }

    /// What the phone sees: the number from the sealed `pair_challenge`.
    fn challenge_number(out: &[Out], link: &LinkKey, device_id: &str) -> String {
        let f: Vec<Value> = frames(out)
            .into_iter()
            .filter(|f| f["t"] == "pair_challenge")
            .collect();
        assert_eq!(f.len(), 1, "exactly one challenge: {f:?}");
        assert_eq!(f[0]["device_id"], device_id);
        let plain = link
            .open(
                &proto::aad_d2p(DESK, device_id),
                f[0]["body"].as_str().unwrap(),
            )
            .expect("opens under the phone's key");
        let v: Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(v["k"], "pair_check");
        assert_eq!(v.as_object().unwrap().len(), 2, "only k and check");
        v["check"].as_str().unwrap().to_string()
    }

    /// Request, read the number off the phone, return it.
    fn request_and_number(w: &mut Window, phone: &Phone, device_id: &str, now: u64) -> String {
        let out = pair_request_as(w, phone, device_id, now);
        challenge_number(&out, &phone_link(w, phone, device_id), device_id)
    }

    fn approve(
        e: &mut Engine,
        id: &str,
        cap: &str,
        check: &str,
        now: u64,
    ) -> (PairOutcome, Vec<LinkKey>) {
        let mut keys_seen = Vec::new();
        let d = e
            .pair_decide(id, true, cap, check, now, &mut |_, k| {
                keys_seen.push(k.clone());
                true
            })
            .unwrap();
        (d, keys_seen)
    }

    fn deny(e: &mut Engine, id: &str, now: u64) -> Result<PairOutcome, &'static str> {
        e.pair_decide(id, false, "", "", now, &mut |_, _| true)
    }

    fn wrong_number(n: &str) -> &'static str {
        if n == "000000" {
            "000001"
        } else {
            "000000"
        }
    }

    #[test]
    fn pairing_needs_a_connection() {
        let mut e = engine_with(vec![]);
        assert_eq!(e.pair_open(0), Err("offline"));
    }

    #[test]
    fn pairing_happy_path_the_phone_shows_the_number_the_user_types() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let out = pair_request(&mut w, &phone, 2_000);
        // The page is told only who asks, never the number.
        assert!(out.contains(&Out::PairRequest {
            device_id: DEV.into(),
            name: "iPhone".into(),
        }));
        let link = phone_link(&w, &phone, DEV);
        let number = challenge_number(&out, &link, DEV);
        assert_eq!(number.len(), 6);
        assert!(number.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(
            frames(&out).len(),
            1,
            "only the challenge goes to the phone yet"
        );
        let (outcome, stored) = approve(&mut w.e, DEV, "respond", &number, 3_000);
        let PairOutcome::Approved(dev) = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(dev.cap, "respond");
        assert_eq!(dev.pubkey, phone.sign.public_b64u());
        assert_eq!(dev.name, "iPhone");
        assert_eq!(dev.added_ms, 3_000);
        assert_eq!(stored.len(), 1, "the link key went to the store");
        let f = frames(&w.e.drain());
        assert_eq!(
            f,
            vec![
                json!({"t":"pair_result","device_id":DEV,"ok":true,
                       "pubkey":phone.sign.public_b64u(),"name":"iPhone"}),
                json!({"t":"pair_close"}),
            ]
        );
        assert!(w.e.pairing_window().is_none());
        // The key the phone derived from its private half is the stored one.
        assert_eq!(link, stored[0]);
        let Window { e, .. } = w;
        let mut r = Rig {
            e,
            phone: phone.sign,
            link,
            session: String::new(),
            seq: 0,
            last_n: 0,
        };
        r.e.on_text(
            &json!({"t":"device_online","device_id":DEV,"conn":"c9"}).to_string(),
            4_000,
        );
        let out = r.open(NOW);
        assert_eq!(r.opened(&out)[0]["cap"], "respond");
    }

    #[test]
    fn a_wrong_number_stores_nothing_and_three_refuse_the_request() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let number = request_and_number(&mut w, &phone, DEV, 2_000);
        let wrong = wrong_number(&number);
        let mut stored = 0;
        let mut persist = |_: &RemoteDevice, _: &LinkKey| {
            stored += 1;
            true
        };
        assert_eq!(
            w.e.pair_decide(DEV, true, "view", wrong, 3_000, &mut persist),
            Ok(PairOutcome::Wrong { tries_left: 2 })
        );
        assert_eq!(
            w.e.pair_decide(DEV, true, "view", "12", 3_001, &mut persist),
            Ok(PairOutcome::Wrong { tries_left: 1 })
        );
        // Nothing was said to the phone yet, the window is still open.
        assert!(frames(&w.e.drain()).is_empty());
        assert!(w.e.pairing_window().is_some());
        // The third wrong one refuses and closes the window.
        assert_eq!(
            w.e.pair_decide(DEV, true, "view", "", 3_002, &mut persist),
            Ok(PairOutcome::Refused)
        );
        let f = frames(&w.e.drain());
        assert_eq!(f[0], json!({"t":"pair_result","device_id":DEV,"ok":false}));
        assert_eq!(f.last().unwrap(), &json!({"t":"pair_close"}));
        assert!(w.e.pairing_window().is_none());
        // The right number now is too late; nothing was ever stored.
        assert_eq!(
            w.e.pair_decide(DEV, true, "view", &number, 3_003, &mut persist),
            Err("unknown_request")
        );
        assert_eq!(stored, 0);
        assert!(w.e.devices.is_empty() && w.e.links.is_empty());
    }

    #[test]
    fn a_right_number_after_two_wrong_ones_still_pairs() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let number = request_and_number(&mut w, &phone, DEV, 2_000);
        for i in 0..2 {
            let r = w.e.pair_decide(
                DEV,
                true,
                "view",
                wrong_number(&number),
                3_000 + i,
                &mut |_, _| true,
            );
            assert!(matches!(r, Ok(PairOutcome::Wrong { .. })));
        }
        assert!(matches!(
            approve(&mut w.e, DEV, "view", &number, 3_100).0,
            PairOutcome::Approved(_)
        ));
    }

    #[test]
    fn the_check_number_is_not_in_any_event_status_or_audit() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let out = pair_request(&mut w, &phone, 2_000);
        let number = challenge_number(&out, &phone_link(&w, &phone, DEV), DEV);
        let mut seen = format!("{out:?}");
        // Wrong tries, a rejection-free approval and the window's own state.
        let r = w.e.pair_decide(
            DEV,
            true,
            "view",
            wrong_number(&number),
            3_000,
            &mut |_, _| true,
        );
        assert!(matches!(r, Ok(PairOutcome::Wrong { .. })));
        seen.push_str(&format!("{:?}", w.e.drain()));
        let (outcome, _) = approve(&mut w.e, DEV, "view", &number, 3_100);
        seen.push_str(&format!("{outcome:?}"));
        let after = w.e.drain();
        seen.push_str(&format!("{after:?}"));
        // Audit lines as they would be written (small clock values: a big
        // epoch time could contain six digits by chance).
        w.e.tick(200_000);
        let tail = w.e.drain();
        seen.push_str(&format!("{tail:?}"));
        for a in audits(&after).iter().chain(audits(&tail).iter()) {
            assert!(!format!("{a:?}").contains(&number));
        }
        // The only place it may appear is the sealed body of the challenge,
        // which is opaque on the wire: strip that frame from the text first.
        let sealed = frames(&out)
            .iter()
            .find(|f| f["t"] == "pair_challenge")
            .map(|f| f["body"].as_str().unwrap().to_string())
            .unwrap();
        let wire: String = seen.replace(&sealed, "");
        assert!(!wire.contains(&number), "the check number leaked: {number}");
    }

    #[test]
    fn a_failed_store_refuses_the_phone_and_keeps_nothing() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let number = request_and_number(&mut w, &phone, DEV, 2_000);
        let r =
            w.e.pair_decide(DEV, true, "view", &number, 3_000, &mut |_, _| false);
        assert_eq!(r, Err("store_failed"));
        assert_eq!(
            frames(&w.e.drain()),
            vec![json!({"t":"pair_result","device_id":DEV,"ok":false})]
        );
        assert!(w.e.devices.is_empty() && w.e.links.is_empty());
    }

    #[test]
    fn a_bad_proof_shows_nothing_says_no_and_is_only_counted() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let good_code = w.code.clone();
        w.code = keys::b64u(&keys::random_bytes(16));
        let out = pair_request(&mut w, &phone, 2_000);
        w.code = good_code;
        assert!(!out.iter().any(|o| matches!(o, Out::PairRequest { .. })));
        assert!(!out.contains(&Out::PairConflict));
        assert_eq!(
            frames(&out),
            vec![json!({"t":"pair_result","device_id":DEV,"ok":false})]
        );
        // No audit line per request: it is counted for the minute.
        assert!(audits(&out).is_empty());
        w.e.tick(NOW);
        let a = audits(&w.e.drain());
        assert_eq!(a.len(), 1);
        assert_eq!(
            (a[0].code, a[0].count, a[0].device.as_str()),
            ("bad_proof", 1, "")
        );
        assert_eq!(deny(&mut w.e, DEV, 2_500), Err("unknown_request"));
    }

    #[test]
    fn an_ek_that_is_not_a_point_gets_no_challenge() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let code = keys::b64u_decode(&w.code).unwrap();
        let pk = phone.sign.public_b64u();
        let bad_ek = keys::b64u(&[4u8; 65]);
        w.e.on_text(
            &json!({"t":"pair_request","device_id":DEV,"pubkey":pk,"ek":bad_ek,
                "name":"x","proof":pair_proof(&code, DESK, DEV, &pk, &bad_ek)})
            .to_string(),
            2_000,
        );
        let out = w.e.drain();
        assert!(!out.iter().any(|o| matches!(o, Out::PairRequest { .. })));
        assert_eq!(
            frames(&out),
            vec![json!({"t":"pair_result","device_id":DEV,"ok":false})]
        );
        // A request with no `ek` at all (an old phone) is refused the same way.
        w.e.on_text(
            &json!({"t":"pair_request","device_id":DEV,"pubkey":pk,"name":"x",
                "proof":pair_proof(&code, DESK, DEV, &pk, "")})
            .to_string(),
            2_001,
        );
        assert!(!w
            .e
            .drain()
            .iter()
            .any(|o| matches!(o, Out::PairRequest { .. })));
    }

    #[test]
    fn two_devices_with_a_valid_proof_refuse_both_and_close_the_window() {
        let mut w = pairing_rig();
        let (a, b) = (new_phone(), new_phone());
        let out = pair_request_as(&mut w, &a, DEV, 2_000);
        assert!(out.iter().any(|o| matches!(o, Out::PairRequest { .. })));
        let out = pair_request_as(&mut w, &b, DEV2, 3_000);
        assert!(out.contains(&Out::PairConflict));
        assert!(!out.iter().any(|o| matches!(o, Out::PairRequest { .. })));
        let f = frames(&out);
        // Both refused, then the window closed.
        assert!(f.contains(&json!({"t":"pair_result","device_id":DEV,"ok":false})));
        assert!(f.contains(&json!({"t":"pair_result","device_id":DEV2,"ok":false})));
        assert_eq!(f.last().unwrap(), &json!({"t":"pair_close"}));
        assert!(w.e.pairing_window().is_none());
        // Neither can be approved afterwards.
        assert_eq!(
            w.e.pair_decide(DEV, true, "view", "123456", 4_000, &mut |_, _| true),
            Err("unknown_request")
        );
        assert!(w.e.devices.is_empty());
        // A third request finds no window.
        let c = new_phone();
        let out = pair_request_as(&mut w, &c, "11111111-1111-4111-8111-111111111111", 5_000);
        assert!(!out.iter().any(|o| matches!(o, Out::PairRequest { .. })));
    }

    #[test]
    fn a_replay_of_a_victims_public_fields_with_another_ek_is_a_conflict() {
        let mut w = pairing_rig();
        let victim = new_phone();
        pair_request(&mut w, &victim, 2_000);
        // Someone who read the QR sends the victim's id and signing key with
        // an `ek` of their own.
        let thief = Phone {
            sign: Keypair::from_b64u(&victim.sign.secret_b64u()).unwrap(),
            ek: keys::EcdhKey::generate(),
        };
        let out = pair_request(&mut w, &thief, 2_500);
        assert!(out.contains(&Out::PairConflict));
        assert!(w.e.pairing_window().is_none());
    }

    #[test]
    fn a_retry_by_the_same_device_is_not_a_conflict() {
        let mut w = pairing_rig();
        let phone = new_phone();
        pair_request(&mut w, &phone, 2_000);
        let out = pair_request(&mut w, &phone, 2_500);
        assert!(!out.contains(&Out::PairConflict));
        assert!(w.e.pairing_window().is_some());
    }

    #[test]
    fn an_expired_window_refuses_and_cannot_be_approved() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let number = request_and_number(&mut w, &phone, DEV, 2_000);
        assert_eq!(
            w.e.pair_decide(DEV, true, "view", &number, 122_000, &mut |_, _| true),
            Err("expired")
        );
        let f = frames(&w.e.drain());
        assert!(f.contains(&json!({"t":"pair_result","device_id":DEV,"ok":false})));
        let mut w = pairing_rig();
        let out = pair_request(&mut w, &phone, 125_000);
        assert!(!out.iter().any(|o| matches!(o, Out::PairRequest { .. })));
    }

    #[test]
    fn denying_tells_the_phone_no_and_keeps_the_window() {
        let mut w = pairing_rig();
        let phone = new_phone();
        pair_request(&mut w, &phone, 2_000);
        assert_eq!(deny(&mut w.e, DEV, 3_000), Ok(PairOutcome::Rejected));
        assert_eq!(
            frames(&w.e.drain()),
            vec![json!({"t":"pair_result","device_id":DEV,"ok":false})]
        );
        assert!(w.e.pairing_window().is_some());
    }

    #[test]
    fn approving_with_a_bad_capability_changes_nothing_and_costs_no_try() {
        let mut w = pairing_rig();
        let phone = new_phone();
        let number = request_and_number(&mut w, &phone, DEV, 2_000);
        assert_eq!(
            w.e.pair_decide(DEV, true, "root", &number, 3_000, &mut |_, _| true),
            Err("bad_cap")
        );
        assert!(matches!(
            approve(&mut w.e, DEV, "view", &number, 3_500).0,
            PairOutcome::Approved(_)
        ));
    }

    #[test]
    fn cancelling_closes_the_window_and_refuses_the_waiting() {
        let mut w = pairing_rig();
        let phone = new_phone();
        pair_request(&mut w, &phone, 2_000);
        let e = &mut w.e;
        e.pair_cancel();
        let f = frames(&e.drain());
        assert_eq!(
            f,
            vec![
                json!({"t":"pair_result","device_id":DEV,"ok":false}),
                json!({"t":"pair_close"})
            ]
        );
        assert!(e.pairing_window().is_none());
    }

    #[test]
    fn pairing_expiry_is_noticed_by_the_tick() {
        let Window { mut e, .. } = pairing_rig();
        e.tick(100_000);
        assert!(e.pairing_window().is_some());
        e.tick(121_000);
        assert!(e.pairing_window().is_none());
    }

    // ----------------------------------------------------- panes + notices

    fn pane_json(id: &str, status: &str) -> Value {
        json!({"pane":id,"ws":"proj","title":format!("Task {id}"),"kind":"claude",
               "status":status,"since_ms":1,"queued":0})
    }

    fn notify_frames(out: &[Out]) -> Vec<Value> {
        frames(out)
            .into_iter()
            .filter(|f| f["t"] == "notify")
            .collect()
    }

    #[test]
    fn waiting_notifies_with_a_clean_title_even_with_no_device_online() {
        let mut e = engine_with(vec![]);
        handshake(&mut e);
        e.publish_panes(&json!([pane_json(PANE, "working")]), 0);
        e.drain();
        let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEF";
        let mut p = pane_json(PANE, "waiting");
        p["title"] = json!(format!("use {key} now"));
        e.publish_panes(&json!([p]), 5_000);
        let n = notify_frames(&e.drain());
        assert_eq!(n.len(), 1);
        assert_eq!(n[0]["kind"], "waiting");
        assert_eq!(n[0]["pane"], PANE);
        assert_eq!(n[0]["body"], "proj");
        assert!(!n[0].to_string().contains("abcdefghijklmnop"));
    }

    #[test]
    fn done_is_off_by_default_and_switchable() {
        let mut e = engine_with(vec![]);
        handshake(&mut e);
        e.publish_panes(&json!([pane_json(PANE, "working")]), 0);
        e.publish_panes(&json!([pane_json(PANE, "done")]), 1_000);
        assert!(notify_frames(&e.drain()).is_empty());
        e.set_notify(true, true);
        e.publish_panes(&json!([pane_json(PANE, "working")]), 2_000);
        e.publish_panes(&json!([pane_json(PANE, "done")]), 40_000);
        let n = notify_frames(&e.drain());
        assert_eq!(n.len(), 1);
        assert_eq!(n[0]["kind"], "done");
        e.set_notify(false, false);
        e.publish_panes(&json!([pane_json(PANE, "working")]), 50_000);
        e.publish_panes(&json!([pane_json(PANE, "waiting")]), 90_000);
        assert!(notify_frames(&e.drain()).is_empty());
    }

    #[test]
    fn notices_are_not_sent_while_offline() {
        let mut e = engine_with(vec![]);
        e.publish_panes(&json!([pane_json(PANE, "working")]), 0);
        e.publish_panes(&json!([pane_json(PANE, "waiting")]), 1_000);
        assert!(e.drain().is_empty());
    }

    #[test]
    fn panes_are_sealed_per_device_and_only_for_devices_with_a_session() {
        let (p1, p2) = (Keypair::generate(), Keypair::generate());
        let k2 = LinkKey::from_bytes([8u8; 32]);
        let mut e = engine_with(vec![
            (device(DEV, &p1, "view"), link()),
            (device(DEV2, &p2, "view"), k2.clone()),
        ]);
        handshake(&mut e);
        // Both are online as far as the server says, only DEV has a session.
        for (d, c) in [(DEV, "c1"), (DEV2, "c2")] {
            e.on_text(
                &json!({"t":"device_online","device_id":d,"conn":c}).to_string(),
                NOW,
            );
        }
        e.drain();
        let mut r = Rig {
            e,
            phone: p1,
            link: link(),
            session: String::new(),
            seq: 0,
            last_n: 0,
        };
        r.open(NOW);
        r.e.publish_panes(&json!([pane_json(PANE, "working")]), NOW + 10_000);
        let out = r.e.drain();
        let f = frames(&out);
        assert_eq!(f.len(), 1, "one frame, for the device with a session");
        assert_eq!(f[0]["to"], DEV);
        assert!(!f[0].to_string().contains("working"));
        let b = r.reply(&out);
        assert_eq!(b["k"], "panes");
        assert_eq!(b["panes"][0]["pane"], PANE);
        // A device that opens too gets its own, sealed under its own key.
        let body = k2
            .seal(
                &proto::aad_p2d(DESK, DEV2),
                json!({"k":"open","desktop_id":DESK,"device_id":DEV2,"nonce":OPEN_NONCE})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
        let sig = p2.sign(&proto::cmd_signed(&body));
        r.e.on_text(
            &json!({"t":"cmd","device_id":DEV2,"conn":"c2","body":body,"sig":sig}).to_string(),
            NOW + 11_000,
        );
        r.e.drain();
        r.e.publish_panes(&json!([pane_json(PANE, "waiting")]), NOW + 12_000);
        let out = r.e.drain();
        // (the working -> waiting change also sends a `notify` frame)
        let f: Vec<Value> = frames(&out)
            .into_iter()
            .filter(|x| x["t"] == "msg")
            .collect();
        let to: Vec<&str> = f.iter().map(|x| x["to"].as_str().unwrap()).collect();
        assert!(to.contains(&DEV) && to.contains(&DEV2), "{to:?}");
        for x in &f {
            if x["to"] == DEV2 {
                let plain = k2
                    .open(&proto::aad_d2p(DESK, DEV2), x["body"].as_str().unwrap())
                    .unwrap();
                let v: Value = serde_json::from_slice(&plain).unwrap();
                assert_eq!(v["k"], "panes");
                assert_eq!(v["n"], 1);
            } else {
                // DEV's own key does not open DEV2's body and vice versa.
                assert!(k2
                    .open(&proto::aad_d2p(DESK, DEV2), x["body"].as_str().unwrap())
                    .is_none());
            }
        }
    }

    #[test]
    fn panes_frames_are_throttled_to_once_a_second() {
        let mut r = Rig::new("view");
        r.e.publish_panes(&json!([pane_json(PANE, "working")]), NOW + 10_000);
        let out = r.e.drain();
        assert_eq!(r.reply(&out)["k"], "panes");
        r.e.publish_panes(&json!([pane_json(PANE, "waiting")]), NOW + 10_300);
        assert!(notify_frames(&r.e.drain()).len() <= 1);
        r.e.tick(NOW + 10_900);
        assert!(frames(&r.e.drain()).is_empty());
        r.e.tick(NOW + 11_000);
        let out = r.e.drain();
        let b = r.reply(&out);
        assert_eq!(b["panes"][0]["status"], "waiting");
        // Identical content is not resent.
        r.e.publish_panes(&json!([pane_json(PANE, "waiting")]), NOW + 20_000);
        assert!(frames(&r.e.drain()).is_empty());
    }

    #[test]
    fn panes_are_not_pushed_with_no_session() {
        let mut r = Rig::bare("view");
        r.e.publish_panes(&json!([pane_json(PANE, "working")]), NOW + 10_000);
        assert!(frames(&r.e.drain()).is_empty());
    }

    #[test]
    fn a_non_list_publish_is_ignored() {
        let mut e = engine_with(vec![]);
        handshake(&mut e);
        e.publish_panes(&json!("nope"), 0);
        e.publish_panes(&Value::Null, 0);
        assert!(e.drain().is_empty());
    }

    // ----------------------------------------------------------------- fuzz

    #[test]
    fn garbage_frames_never_panic() {
        let phone = Keypair::generate();
        let mut e = engine_with(vec![(device(DEV, &phone, "prompt"), link())]);
        handshake(&mut e);
        e.pair_open(0).unwrap();
        let mut seeds: Vec<String> = vec![
            String::new(),
            "{".into(),
            "null".into(),
            "[]".into(),
            "\"x\"".into(),
            "{\"t\":null}".into(),
            "{\"t\":\"cmd\"}".into(),
            "{\"t\":\"cmd\",\"device_id\":1,\"conn\":2,\"body\":3,\"sig\":4}".into(),
            format!("{{\"t\":\"cmd\",\"device_id\":\"{DEV}\",\"conn\":\"c\",\"body\":\"\",\"sig\":\"\"}}"),
            format!("{{\"t\":\"cmd\",\"device_id\":\"{DEV}\",\"conn\":\"c\",\"body\":\"{{}}\",\"sig\":\"AAAA\"}}"),
            "{\"t\":\"pair_request\",\"device_id\":\"\",\"pubkey\":\"\",\"proof\":\"\"}".into(),
            "{\"t\":\"device_online\",\"device_id\":\"\\u0000\",\"conn\":\"\"}".into(),
            "{\"t\":\"challenge\",\"nonce\":\"\\ud800\"}".into(),
            "\u{0}\u{1}\u{2}".into(),
            "x".repeat(proto::MAX_FRAME + 1),
            format!("{{\"t\":\"{}\"}}", "z".repeat(10_000)),
        ];
        // Deterministic byte noise, valid and invalid UTF-8-ish.
        let mut x: u32 = 0x1234_5678;
        for _ in 0..300 {
            let mut s = String::new();
            for _ in 0..(x % 200) {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                s.push(char::from_u32(32 + (x >> 24) % 95).unwrap_or('?'));
            }
            seeds.push(s);
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        }
        for (i, s) in seeds.iter().enumerate() {
            e.on_text(s, i as u64);
            e.tick(i as u64);
            e.drain();
        }
        // Still functional afterwards.
        assert!(e.connected());
    }

    #[test]
    fn garbage_signed_and_sealed_bodies_never_panic() {
        let mut r = Rig::new("prompt");
        let weird = [
            json!(null),
            json!([]),
            json!("x"),
            json!({"op":null}),
            json!({"op":"peek","pane":PANE,"lines":1e300}),
            json!({"op":"peek","pane":PANE,"lines":u64::MAX}),
            json!({"op":"answer","pane":PANE,"n":u64::MAX,"label":"x","menu_id":"m"}),
            json!({"op":"prompt","pane":PANE,"text":"\u{0}"}),
            json!({"op":"prompt","pane":PANE,"text":"\u{1F600}".repeat(16_000)}),
        ];
        for w in weird {
            let out = r.cmd(w, NOW);
            // Whatever happened, only sealed protocol shapes left.
            for f in frames(&out) {
                assert!(f["t"] == "msg");
            }
        }
        // Signed and sealed noise as the whole plaintext.
        for plain in [
            "",
            "\u{0}",
            "{\"seq\":1e999}",
            "{\"seq\":18446744073709551616}",
            "[1]",
        ] {
            let body = r
                .link
                .seal(&proto::aad_p2d(DESK, DEV), plain.as_bytes())
                .unwrap();
            let sig = r.phone.sign(&proto::cmd_signed(&body));
            r.e.on_text(
                &json!({"t":"cmd","device_id":DEV,"conn":CONN,"body":body,"sig":sig}).to_string(),
                NOW,
            );
            r.e.drain();
        }
    }
}
