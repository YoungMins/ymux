//! One current session per device and the authorisation of a command
//! (PROTOCOL.md section 5.1). Pure: the caller supplies the paired device, the
//! opened plaintext and the clock.
//!
//! A session exists only because a device asked for one with a sealed, signed
//! `open` ([`Sessions::open`]); nothing the server says (online, offline,
//! connection ids) creates, selects or extends one. Issuing a session revokes
//! the device's previous one, so a command held back by a hostile relay is
//! dead the moment its device opens a new session.
//!
//! [`Sessions::authorise`] is the only door a command goes through. The caller
//! has already checked the device and signature and opened the body (check 1);
//! it then runs, in order: size and shape, desktop and device ids, session and
//! sequence number, clock, capability, then the field values of the op. A
//! failure at any step is a [`Decision::Reject`] carrying the protocol code
//! and nothing else happens.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use super::keys::{self, random_b64u};
use super::proto::{self, Cap, CmdBody, Code, Prev, Request, MAX_BODY, TS_WINDOW_MS};
use crate::config::model::RemoteDevice;

/// A device may open this many sessions per [`OPEN_WINDOW_MS`]; the rest are
/// dropped, so a relay replaying old `open` frames cannot churn sessions.
pub const OPEN_LIMIT: usize = 5;
pub const OPEN_WINDOW_MS: u64 = 10_000;

#[derive(Debug, Clone)]
struct DevSession {
    session: String,
    /// Last accepted command `seq`.
    last_seq: u64,
    /// Last `n` put on a desktop to device body.
    n: u64,
}

#[derive(Debug, Default)]
pub struct Sessions {
    by_device: HashMap<String, DevSession>,
    /// The newest session of each device that was ended (replaced, revoked,
    /// or cleared by a reconnect) and the last `seq` accepted in it: what the
    /// next `session` reply reports as `prev`. Dropped only with the device.
    ended: HashMap<String, Prev>,
    /// Times of the recent `open`s per device.
    opens: HashMap<String, VecDeque<u64>>,
}

/// What the server relayed (already opened) plus what this desktop knows.
pub struct Incoming<'a> {
    pub desktop_id: &'a str,
    pub device: &'a RemoteDevice,
    /// The opened plaintext.
    pub plaintext: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow {
        /// The sequence number the reply answers.
        re: u64,
        /// The session the command ran in: the reply is only delivered while
        /// it is still the current one.
        session: String,
        request: Request,
    },
    /// `seq` skipped ahead of the last accepted one + 1: the session was
    /// revoked and the command not consumed; answered `bad_session`.
    Gap,
    Reject {
        /// The sequence number once the session and `seq` were accepted, else
        /// 0 (the body was not trusted enough to read it).
        re: u64,
        code: Code,
        /// The op the command named, if it parsed (for the audit log only).
        op: Option<proto::Op>,
        pane: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    Issued {
        session: String,
        /// The session this one revoked and the last `seq` accepted in it.
        prev: Prev,
    },
    /// Too many opens just now: dropped.
    Limited,
}

fn reject(re: u64, code: Code) -> Decision {
    Decision::Reject {
        re,
        code,
        op: None,
        pane: String::new(),
    }
}

impl Sessions {
    /// A fresh session for `device_id`, revoking its older one. The caller has
    /// checked that the device is paired and the `open` is valid.
    pub fn open(&mut self, device_id: &str, now_ms: u64) -> Opened {
        let times = self.opens.entry(device_id.to_string()).or_default();
        while times
            .front()
            .is_some_and(|t| now_ms.saturating_sub(*t) >= OPEN_WINDOW_MS)
        {
            times.pop_front();
        }
        if times.len() >= OPEN_LIMIT {
            return Opened::Limited;
        }
        times.push_back(now_ms);
        let session = random_b64u(16);
        let old = self.by_device.insert(
            device_id.to_string(),
            DevSession {
                session: session.clone(),
                last_seq: 0,
                n: 0,
            },
        );
        // Report the session this one replaced; with none live, the one that
        // was ended last (a `reopen` or a reconnect revoked it before this
        // `open`). Either way it is consumed: the next report is about this one.
        let ended = self.ended.remove(device_id);
        let prev = match old {
            Some(o) => Prev {
                session: o.session,
                seq: o.last_seq,
            },
            None => ended.unwrap_or_default(),
        };
        Opened::Issued { session, prev }
    }

    fn end(&mut self, device_id: &str) {
        if let Some(s) = self.by_device.remove(device_id) {
            self.ended.insert(
                device_id.to_string(),
                Prev {
                    session: s.session,
                    seq: s.last_seq,
                },
            );
        }
    }

    /// A session with a known id, so a recorded vector can run against it.
    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, device_id: &str, session: &str) {
        self.insert_with_seq_for_test(device_id, session, 0);
    }

    /// Like [`Sessions::insert_for_test`], with `last_seq` already accepted
    /// in it (what a `session` reply's `prev` reports once it is replaced).
    #[cfg(test)]
    pub(crate) fn insert_with_seq_for_test(&mut self, device_id: &str, session: &str, seq: u64) {
        self.by_device.insert(
            device_id.to_string(),
            DevSession {
                session: session.to_string(),
                last_seq: seq,
                n: 0,
            },
        );
    }

    /// End the device's session (the next command is `bad_session`).
    pub fn revoke(&mut self, device_id: &str) {
        self.end(device_id);
    }

    /// Every session ends (the desktop reconnected); what each had accepted
    /// is kept for the `prev` of the device's next session.
    pub fn clear(&mut self) {
        let ids: Vec<String> = self.by_device.keys().cloned().collect();
        for id in ids {
            self.end(&id);
        }
    }

    /// Forget a device that was removed.
    pub fn drop_device(&mut self, device_id: &str) {
        self.by_device.remove(device_id);
        self.ended.remove(device_id);
        self.opens.remove(device_id);
    }

    pub fn current(&self, device_id: &str) -> Option<&str> {
        self.by_device.get(device_id).map(|s| s.session.as_str())
    }

    /// Devices that hold a session right now.
    pub fn device_ids(&self) -> Vec<String> {
        self.by_device.keys().cloned().collect()
    }

    pub fn any(&self) -> bool {
        !self.by_device.is_empty()
    }

    /// Put the current session, the next `n` (1, 2, 3, ...) and this
    /// desktop's clock (`at_ms`) on an outgoing body. `false` (body
    /// untouched) when the device has no session.
    pub fn stamp(&mut self, device_id: &str, body: &mut Value, at_ms: u64) -> bool {
        let Some(s) = self.by_device.get_mut(device_id) else {
            return false;
        };
        s.n += 1;
        if let Some(o) = body.as_object_mut() {
            o.insert("session".into(), Value::String(s.session.clone()));
            o.insert("n".into(), Value::from(s.n));
            o.insert("at_ms".into(), Value::from(at_ms));
        }
        true
    }

    /// Run checks 2 to 6 of section 5.1 on an opened command plaintext. The
    /// `seq` is consumed as soon as the session and `seq` checks pass.
    pub fn authorise(&mut self, inc: &Incoming<'_>, now_ms: u64) -> Decision {
        // 6 (size half). Before parsing anything.
        if inc.plaintext.len() > MAX_BODY {
            return reject(0, Code::BadRequest);
        }
        // 6 (well-formed half).
        let Ok(body) = serde_json::from_str::<CmdBody>(inc.plaintext) else {
            return reject(0, Code::BadRequest);
        };
        if body.k != "cmd" {
            return reject(0, Code::BadRequest);
        }
        // 2. Addressed to this desktop, from the device the frame is for.
        if body.desktop_id != inc.desktop_id || body.device_id != inc.device.id {
            return reject(0, Code::BadRequest);
        }
        // 3. The device's current session, and a sequence number beyond the
        //    last accepted one. Until the session matches, `seq` is only a
        //    claim, so a failure answers `re: 0`.
        let Some(sess) = self.by_device.get_mut(&inc.device.id) else {
            return reject(0, Code::BadSession);
        };
        if !keys::ct_eq(sess.session.as_bytes(), body.session.as_bytes()) {
            return reject(0, Code::BadSession);
        }
        let re = body.seq;
        if re <= sess.last_seq {
            return reject(re, Code::Replay);
        }
        // Consecutive only: `prev.seq` must mean every seq up to it was
        // accepted. A skipped number means the relay dropped a command; the
        // session is no longer one the phone can reason about, so it ends
        // (the phone opens a new one and learns the truth from `prev`). The
        // gap command is not consumed.
        if re != sess.last_seq.saturating_add(1) {
            self.end(&inc.device.id);
            return Decision::Gap;
        }
        sess.last_seq = re;
        let session = sess.session.clone();
        // 4. Fresh: the device's estimate of our clock is within 30 s.
        let op = proto::op_of(&body.cmd);
        let pane = body
            .cmd
            .get("pane")
            .and_then(|p| p.as_str())
            .filter(|p| proto::is_uuid(p))
            .unwrap_or("")
            .to_string();
        let fail = |code| Decision::Reject {
            re,
            code,
            op,
            pane: pane.clone(),
        };
        let now = i64::try_from(now_ms).unwrap_or(i64::MAX);
        if now.abs_diff(body.ts_ms) > TS_WINDOW_MS {
            return fail(Code::Expired);
        }
        // 5. The capability allows the op. An op that does not exist is
        //    `unknown_op` (there is nothing to authorise).
        let Some(op_known) = op else {
            return match proto::parse_request(&body.cmd) {
                Err(Code::UnknownOp) => fail(Code::UnknownOp),
                _ => fail(Code::BadRequest),
            };
        };
        match Cap::parse(&inc.device.cap) {
            Some(cap) if cap >= op_known.needs() => {}
            _ => return fail(Code::Forbidden),
        }
        // 6. The op's own fields.
        match proto::parse_request(&body.cmd) {
            Ok(request) => Decision::Allow {
                re,
                session,
                request,
            },
            Err(code) => fail(code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DESK: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
    const DEV: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
    const OTHER: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    const PANE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const NOW: u64 = 1_700_000_000_000;

    struct Rig {
        sessions: Sessions,
        device: RemoteDevice,
        session: String,
    }

    fn rig(cap: &str) -> Rig {
        let device = RemoteDevice {
            id: DEV.into(),
            name: "Phone".into(),
            pubkey: "unused-here".into(),
            cap: cap.into(),
            added_ms: 0,
        };
        let mut sessions = Sessions::default();
        let Opened::Issued { session, prev } = sessions.open(DEV, NOW) else {
            panic!()
        };
        assert_eq!(prev, Prev::default());
        Rig {
            sessions,
            device,
            session,
        }
    }

    impl Rig {
        fn body_at(&self, seq: u64, ts: i64, cmd: Value) -> String {
            json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":self.session,
                   "seq":seq,"ts_ms":ts,"cmd":cmd})
            .to_string()
        }

        fn body(&self, seq: u64, cmd: Value) -> String {
            self.body_at(seq, NOW as i64, cmd)
        }

        fn run(&mut self, plaintext: &str) -> Decision {
            self.run_at(plaintext, NOW)
        }

        fn run_at(&mut self, plaintext: &str, now: u64) -> Decision {
            self.sessions.authorise(
                &Incoming {
                    desktop_id: DESK,
                    device: &self.device,
                    plaintext,
                },
                now,
            )
        }
    }

    fn code(d: &Decision) -> Option<Code> {
        match d {
            Decision::Reject { code, .. } => Some(*code),
            Decision::Gap => Some(Code::BadSession),
            Decision::Allow { .. } => None,
        }
    }

    fn re(d: &Decision) -> u64 {
        match d {
            Decision::Reject { re, .. } | Decision::Allow { re, .. } => *re,
            Decision::Gap => 0,
        }
    }

    #[test]
    fn a_good_command_is_allowed() {
        let mut r = rig("view");
        let b = r.body(1, json!({"op":"list"}));
        assert_eq!(
            r.run(&b),
            Decision::Allow {
                re: 1,
                session: r.session.clone(),
                request: Request::List
            }
        );
    }

    #[test]
    fn wrong_desktop_or_device_id_is_bad_request() {
        let mut r = rig("prompt");
        let b = json!({"k":"cmd","desktop_id":OTHER,"device_id":DEV,"session":r.session,
                       "seq":1,"ts_ms":NOW,"cmd":{"op":"list"}})
        .to_string();
        assert_eq!(code(&r.run(&b)), Some(Code::BadRequest));
        let b = json!({"k":"cmd","desktop_id":DESK,"device_id":OTHER,"session":r.session,
                       "seq":1,"ts_ms":NOW,"cmd":{"op":"list"}})
        .to_string();
        assert_eq!(code(&r.run(&b)), Some(Code::BadRequest));
        // Untrusted until the session matched: re 0.
        assert_eq!(re(&r.run(&b)), 0);
    }

    #[test]
    fn wrong_or_missing_session_is_bad_session_with_re_zero() {
        let mut r = rig("prompt");
        let b = json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":"nope",
                       "seq":7,"ts_ms":NOW,"cmd":{"op":"list"}})
        .to_string();
        let d = r.run(&b);
        assert_eq!(code(&d), Some(Code::BadSession));
        assert_eq!(re(&d), 0);
        // A device with no session at all.
        r.sessions.revoke(DEV);
        let b = r.body(1, json!({"op":"list"}));
        assert_eq!(code(&r.run(&b)), Some(Code::BadSession));
    }

    #[test]
    fn replayed_or_equal_or_lower_seq_is_replay() {
        let mut r = rig("view");
        for seq in 1..=5 {
            let b = r.body(seq, json!({"op":"list"}));
            assert!(code(&r.run(&b)).is_none(), "seq {seq}");
        }
        // The very same body again.
        let b5 = r.body(5, json!({"op":"list"}));
        assert_eq!(code(&r.run(&b5)), Some(Code::Replay));
        for seq in [5, 4, 0] {
            let b = r.body(seq, json!({"op":"list"}));
            assert_eq!(code(&r.run(&b)), Some(Code::Replay), "seq {seq}");
        }
        // The replay reply names the seq it answers.
        let d = r.run(&r.body(5, json!({"op":"list"})));
        assert!(matches!(
            d,
            Decision::Reject {
                re: 5,
                code: Code::Replay,
                ..
            }
        ));
    }

    #[test]
    fn a_skipped_seq_revokes_the_session_and_is_not_consumed() {
        let mut r = rig("view");
        for seq in 1..=3 {
            let b = r.body(seq, json!({"op":"list"}));
            assert!(code(&r.run(&b)).is_none());
        }
        // 4 was dropped by the relay; 5 arrives.
        let d = r.run(&r.body(5, json!({"op":"list"})));
        assert_eq!(d, Decision::Gap);
        assert_eq!(code(&d), Some(Code::BadSession));
        assert_eq!(re(&d), 0);
        // The session is gone: nothing more runs in it, 4 included.
        assert!(r.sessions.current(DEV).is_none());
        let d = r.run(&r.body(4, json!({"op":"list"})));
        assert_eq!(code(&d), Some(Code::BadSession));
        // The next `session` reports exactly what was accepted.
        match r.sessions.open(DEV, NOW + 1) {
            Opened::Issued { prev, .. } => assert_eq!(prev.seq, 3),
            other => panic!("{other:?}"),
        }
        // A first command that is not 1 is a gap too.
        let mut r2 = rig("view");
        assert_eq!(r2.run(&r2.body(2, json!({"op":"list"}))), Decision::Gap);
    }

    #[test]
    fn a_rejected_command_does_not_burn_a_sequence_number_before_the_session_check() {
        let mut r = rig("view");
        let bad = json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":"wrong",
                         "seq":10,"ts_ms":NOW,"cmd":{"op":"list"}})
        .to_string();
        assert_eq!(code(&r.run(&bad)), Some(Code::BadSession));
        let ok = r.body(1, json!({"op":"list"}));
        assert!(code(&r.run(&ok)).is_none());
    }

    #[test]
    fn the_clock_must_be_within_30_seconds() {
        let mut r = rig("view");
        let ts = NOW as i64;
        assert_eq!(TS_WINDOW_MS, 30_000);
        // Exactly 30 000 ms either way is fine.
        let b = r.body_at(1, ts - 30_000, json!({"op":"list"}));
        assert!(code(&r.run(&b)).is_none());
        let b = r.body_at(2, ts + 30_000, json!({"op":"list"}));
        assert!(code(&r.run(&b)).is_none());
        // One more is expired, past and future.
        let d = r.run(&r.body_at(3, ts - 30_001, json!({"op":"list"})));
        assert_eq!(code(&d), Some(Code::Expired));
        assert_eq!(re(&d), 3);
        let b = r.body_at(4, ts + 30_001, json!({"op":"list"}));
        assert_eq!(code(&r.run(&b)), Some(Code::Expired));
        // The old minute is no longer in range.
        let b = r.body_at(5, ts - 45_000, json!({"op":"list"}));
        assert_eq!(code(&r.run(&b)), Some(Code::Expired));
        // A command held for hours.
        let b = r.body_at(6, ts, json!({"op":"list"}));
        assert_eq!(
            code(&r.run_at(&b, NOW + 3 * 3_600_000)),
            Some(Code::Expired)
        );
    }

    #[test]
    fn an_expired_command_still_consumed_its_seq() {
        let mut r = rig("view");
        let b = r.body_at(1, NOW as i64 - 120_000, json!({"op":"list"}));
        assert_eq!(code(&r.run(&b)), Some(Code::Expired));
        // The same seq again is a replay, not a second chance.
        let b = r.body(1, json!({"op":"list"}));
        assert_eq!(code(&r.run(&b)), Some(Code::Replay));
        let b = r.body(2, json!({"op":"list"}));
        assert!(code(&r.run(&b)).is_none());
    }

    #[test]
    fn a_missing_or_fractional_ts_is_bad_request() {
        let mut r = rig("view");
        let b = json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":r.session,
                       "seq":1,"cmd":{"op":"list"}})
        .to_string();
        assert_eq!(code(&r.run(&b)), Some(Code::BadRequest));
        let b = json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":r.session,
                       "seq":1,"ts_ms":"now","cmd":{"op":"list"}})
        .to_string();
        assert_eq!(code(&r.run(&b)), Some(Code::BadRequest));
    }

    #[test]
    fn capability_must_cover_each_op() {
        let all = |pane: &str| {
            vec![
                json!({"op":"list"}),
                json!({"op":"peek","pane":pane}),
                json!({"op":"answer","pane":pane,"n":1,"label":"Yes","menu_id":"m1"}),
                json!({"op":"interrupt","pane":pane}),
                json!({"op":"prompt","pane":pane,"text":"hi","queue":false}),
            ]
        };
        for (cap, allowed) in [
            ("view", [true, true, false, false, false]),
            ("respond", [true, true, true, true, false]),
            ("prompt", [true, true, true, true, true]),
        ] {
            for (i, cmd) in all(PANE).into_iter().enumerate() {
                let mut r = rig(cap);
                let b = r.body(1, cmd.clone());
                let d = r.run(&b);
                if allowed[i] {
                    assert!(code(&d).is_none(), "{cap} should run {cmd}");
                } else {
                    assert_eq!(code(&d), Some(Code::Forbidden), "{cap} must not run {cmd}");
                }
            }
        }
    }

    #[test]
    fn an_unknown_capability_string_runs_nothing() {
        for cap in ["", "admin", "VIEW"] {
            let mut r = rig(cap);
            let b = r.body(1, json!({"op":"list"}));
            assert_eq!(code(&r.run(&b)), Some(Code::Forbidden), "{cap:?}");
        }
    }

    #[test]
    fn unknown_ops_are_unknown_op_not_a_crash() {
        let mut r = rig("prompt");
        for (i, op) in ["keys", "close", "fs_read", "spawn", "LIST", ""]
            .iter()
            .enumerate()
        {
            let b = r.body(i as u64 + 1, json!({"op":op,"pane":PANE}));
            assert_eq!(code(&r.run(&b)), Some(Code::UnknownOp), "{op}");
        }
        // An op that is not even a string is malformed.
        let b = r.body(7, json!({"op":7}));
        assert_eq!(code(&r.run(&b)), Some(Code::BadRequest));
        let b = r.body(8, json!({}));
        assert_eq!(code(&r.run(&b)), Some(Code::BadRequest));
    }

    #[test]
    fn oversize_plaintext_is_rejected_before_it_is_parsed() {
        let mut r = rig("prompt");
        let big = format!(
            r#"{{"k":"cmd","desktop_id":"{DESK}","device_id":"{DEV}","session":"{}","seq":1,"ts_ms":{NOW},"cmd":{{"op":"list","pad":"{}"}}}}"#,
            r.session,
            "x".repeat(MAX_BODY)
        );
        assert!(big.len() > MAX_BODY);
        assert_eq!(code(&r.run(&big)), Some(Code::BadRequest));
        let ok = r.body(1, json!({"op":"list"}));
        assert!(code(&r.run(&ok)).is_none());
    }

    #[test]
    fn malformed_bodies_are_bad_request() {
        let mut r = rig("prompt");
        let s = r.session.clone();
        for body in [
            "not json".to_string(),
            "[]".to_string(),
            "{}".to_string(),
            json!({"k":"cmd"}).to_string(),
            json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":s,
                   "seq":"1","ts_ms":NOW,"cmd":{"op":"list"}})
            .to_string(),
            json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":s,
                   "seq":-1,"ts_ms":NOW,"cmd":{"op":"list"}})
            .to_string(),
            json!({"k":"reply","desktop_id":DESK,"device_id":DEV,"session":s,
                   "seq":1,"ts_ms":NOW,"cmd":{"op":"list"}})
            .to_string(),
            json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":s,
                   "seq":1,"ts_ms":NOW,"cmd":"list"})
            .to_string(),
        ] {
            assert_eq!(code(&r.run(&body)), Some(Code::BadRequest), "{body}");
        }
    }

    #[test]
    fn field_errors_come_after_the_capability_check() {
        let mut r = rig("view");
        // A view device sending a malformed prompt is forbidden, not "bad".
        let b = r.body(1, json!({"op":"prompt","pane":"x","text":""}));
        assert_eq!(code(&r.run(&b)), Some(Code::Forbidden));
        // A device with the capability gets bad_request for the field.
        let mut r = rig("prompt");
        let b = r.body(1, json!({"op":"prompt","pane":"x","text":"hi"}));
        let d = r.run(&b);
        assert!(matches!(
            d,
            Decision::Reject {
                code: Code::BadRequest,
                op: Some(proto::Op::Prompt),
                ..
            }
        ));
    }

    #[test]
    fn a_newer_open_revokes_the_older_session() {
        let mut r = rig("view");
        for seq in 1..=7 {
            let b = r.body(seq, json!({"op":"list"}));
            assert!(code(&r.run(&b)).is_none());
        }
        let old = r.session.clone();
        let Opened::Issued { session: new, prev } = r.sessions.open(DEV, NOW + 1) else {
            panic!()
        };
        assert_ne!(new, old);
        // The device is told which session this revoked and how far it got.
        assert_eq!(
            prev,
            Prev {
                session: old.clone(),
                seq: 7
            }
        );
        // A command held under the old session, delivered now: dead.
        let held = r.body(8, json!({"op":"list"}));
        assert_eq!(code(&r.run(&held)), Some(Code::BadSession));
        // The new one works, counting from seq 1.
        r.session = new;
        let b = r.body(1, json!({"op":"list"}));
        assert!(code(&r.run(&b)).is_none());
        assert_eq!(r.sessions.device_ids(), vec![DEV.to_string()]);
    }

    #[test]
    fn sessions_are_per_device() {
        let mut r = rig("view");
        let Opened::Issued { session: other, .. } = r.sessions.open(OTHER, NOW) else {
            panic!()
        };
        // Opening for another device leaves this one alone.
        assert_eq!(r.sessions.current(DEV), Some(r.session.as_str()));
        assert_eq!(r.sessions.current(OTHER), Some(other.as_str()));
        // And this device's session id is no good as the other's.
        let b = json!({"k":"cmd","desktop_id":DESK,"device_id":DEV,"session":other,
                       "seq":1,"ts_ms":NOW,"cmd":{"op":"list"}})
        .to_string();
        assert_eq!(code(&r.run(&b)), Some(Code::BadSession));
    }

    #[test]
    fn opens_are_rate_limited_per_device() {
        let mut s = Sessions::default();
        for i in 0..OPEN_LIMIT as u64 {
            assert!(matches!(s.open(DEV, 1_000 + i), Opened::Issued { .. }));
        }
        let kept = s.current(DEV).unwrap().to_string();
        // The sixth within ten seconds is dropped and changes nothing.
        assert_eq!(s.open(DEV, 5_000), Opened::Limited);
        assert_eq!(s.current(DEV), Some(kept.as_str()));
        // Another device has its own allowance.
        assert!(matches!(s.open(OTHER, 5_000), Opened::Issued { .. }));
        // Ten seconds after the first, one slot frees up.
        assert!(matches!(
            s.open(DEV, 1_000 + OPEN_WINDOW_MS),
            Opened::Issued { .. }
        ));
        assert_eq!(s.open(DEV, 1_000 + OPEN_WINDOW_MS), Opened::Limited);
        // Clearing sessions (a reconnect) does not reset the allowance.
        s.clear();
        assert_eq!(s.open(DEV, 1_000 + OPEN_WINDOW_MS), Opened::Limited);
    }

    #[test]
    fn revoke_clear_and_drop_end_sessions() {
        let mut r = rig("view");
        r.sessions.revoke(DEV);
        assert!(r.sessions.current(DEV).is_none());
        assert!(!r.sessions.any());
        let b = r.body(1, json!({"op":"list"}));
        assert_eq!(code(&r.run(&b)), Some(Code::BadSession));
        let mut r = rig("view");
        r.sessions.clear();
        assert_eq!(
            code(&r.run(&r.body(1, json!({"op":"list"})))),
            Some(Code::BadSession)
        );
        let mut r = rig("view");
        r.sessions.drop_device(DEV);
        assert_eq!(
            code(&r.run(&r.body(1, json!({"op":"list"})))),
            Some(Code::BadSession)
        );
    }

    #[test]
    fn outgoing_n_is_strictly_increasing_from_one_and_carries_the_clock() {
        let mut r = rig("view");
        let mut ns = Vec::new();
        for i in 0..5 {
            let mut body = json!({"k":"panes","panes":[]});
            assert!(r.sessions.stamp(DEV, &mut body, NOW + i));
            assert_eq!(body["session"], r.session.as_str());
            assert_eq!(body["at_ms"], NOW + i);
            ns.push(body["n"].as_u64().unwrap());
        }
        assert_eq!(ns, vec![1, 2, 3, 4, 5]);
        // A new session starts again at 1.
        let Opened::Issued { .. } = r.sessions.open(DEV, NOW + 1) else {
            panic!()
        };
        let mut body = json!({"k":"reopen"});
        assert!(r.sessions.stamp(DEV, &mut body, NOW));
        assert_eq!(body["n"], 1);
        // No session, no stamp.
        let mut body = json!({"k":"panes"});
        assert!(!r.sessions.stamp(OTHER, &mut body, NOW));
        assert!(body.get("session").is_none());
        assert!(body.get("at_ms").is_none());
    }

    fn issued(s: &mut Sessions, device: &str, now: u64) -> (String, Prev) {
        match s.open(device, now) {
            Opened::Issued { session, prev } => (session, prev),
            Opened::Limited => panic!("limited"),
        }
    }

    #[test]
    fn prev_is_empty_for_a_first_session() {
        let mut s = Sessions::default();
        let (_, prev) = issued(&mut s, DEV, NOW);
        assert_eq!(
            prev,
            Prev {
                session: String::new(),
                seq: 0
            }
        );
    }

    #[test]
    fn prev_after_a_replace_with_and_without_accepted_commands() {
        let mut r = rig("view");
        // Without: nothing was accepted in the first session.
        let first = r.session.clone();
        let (second, prev) = issued(&mut r.sessions, DEV, NOW + 1);
        assert_eq!(
            prev,
            Prev {
                session: first,
                seq: 0
            }
        );
        // With: seq 1..=3 accepted in the second.
        r.session = second.clone();
        assert!(code(&r.run(&r.body(1, json!({"op":"list"})))).is_none());
        assert!(code(&r.run(&r.body(2, json!({"op":"list"})))).is_none());
        assert!(code(&r.run(&r.body(3, json!({"op":"list"})))).is_none());
        // A rejected one after it (replay of 2 < 3) does not move it.
        assert_eq!(
            code(&r.run(&r.body(2, json!({"op":"list"})))),
            Some(Code::Replay)
        );
        let (_, prev) = issued(&mut r.sessions, DEV, NOW + 2);
        assert_eq!(
            prev,
            Prev {
                session: second,
                seq: 3
            }
        );
    }

    #[test]
    fn prev_survives_a_revoke_a_reconnect_and_a_second_revoke() {
        let mut r = rig("view");
        let first = r.session.clone();
        for seq in 1..=5 {
            assert!(code(&r.run(&r.body(seq, json!({"op":"list"})))).is_none());
        }
        // A capability change revokes it; the next open still learns of it.
        r.sessions.revoke(DEV);
        r.sessions.revoke(DEV); // nothing to end: does not erase the record
        let (second, prev) = issued(&mut r.sessions, DEV, NOW + 1);
        assert_eq!(
            prev,
            Prev {
                session: first,
                seq: 5
            }
        );
        // The record was consumed: a later open reports the newer session.
        r.session = second.clone();
        assert!(code(&r.run(&r.body(1, json!({"op":"list"})))).is_none());
        // A desktop reconnect clears every session; the same holds.
        r.sessions.clear();
        let (third, prev) = issued(&mut r.sessions, DEV, NOW + 2);
        assert_eq!(
            prev,
            Prev {
                session: second,
                seq: 1
            }
        );
        assert_ne!(third, prev.session);
    }

    #[test]
    fn prev_is_per_device_and_forgotten_with_the_device() {
        let mut s = Sessions::default();
        let (a, _) = issued(&mut s, DEV, NOW);
        let (_, prev) = issued(&mut s, OTHER, NOW);
        assert_eq!(prev, Prev::default());
        s.revoke(DEV);
        s.drop_device(DEV);
        let (_, prev) = issued(&mut s, DEV, NOW + 1);
        assert_eq!(
            prev,
            Prev::default(),
            "removed devices start from nothing: {a}"
        );
    }

    #[test]
    fn rejection_carries_op_and_pane_for_the_audit_log() {
        let mut r = rig("view");
        let b = r.body(
            1,
            json!({"op":"answer","pane":PANE,"n":1,"label":"Yes","menu_id":"m"}),
        );
        match r.run(&b) {
            Decision::Reject { op, pane, code, re } => {
                assert_eq!(op, Some(proto::Op::Answer));
                assert_eq!(pane, PANE);
                assert_eq!(code, Code::Forbidden);
                assert_eq!(re, 1);
            }
            d => panic!("{d:?}"),
        }
    }
}
