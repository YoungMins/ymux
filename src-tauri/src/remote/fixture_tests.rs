//! The server project's fixtures (`fixtures/*.json`, copied verbatim from
//! `yMux-Server/fixtures`; its `README.md` says how they were made), run
//! through this side. They use fixed test keys (32 identical bytes), a fixed
//! pairing code, fixed key-agreement keys and fixed AES-GCM nonces, so a
//! mismatch here means the two implementations disagree about the wire.
//!
//! Beyond encoders and parsers this replays the recorded conversation through
//! the real [`Engine`]: the sealed `open` gets a `session` reply (with `prev`),
//! the sealed `answer` reaches the page with its `menu_id`, and what the
//! desktop seals in return (`reply`, `panes`, `reopen`, with `n` and `at_ms`,
//! and the `re:0` `bad_sig` with its `ref`) is exactly the recorded plaintext.
//! The link key is derived from either private key by ECDH + HKDF and the
//! pairing proof and the sealed `pair_check` are checked the same way.

use serde_json::{json, Value};

use super::engine::{Engine, Out};
use super::keys::{self, b64u, Check, EcdhKey, Keypair, LinkKey, LinkParams};
use super::proto::{self, ServerFrame};
use crate::config::model::RemoteDevice;

macro_rules! fixture {
    ($name:literal) => {
        serde_json::from_str::<Value>(include_str!(concat!("fixtures/", $name, ".json")))
            .expect(concat!($name, " is JSON"))
    };
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_else(|| panic!("{k}")).to_string()
}

fn hex_to_bytes(h: &str) -> Vec<u8> {
    (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn key_from_hex(h: &str) -> Keypair {
    Keypair::from_b64u(&b64u(&hex_to_bytes(h))).unwrap()
}

fn ecdh_from_hex(h: &str) -> EcdhKey {
    let scalar: [u8; 32] = hex_to_bytes(h).try_into().unwrap();
    EcdhKey::from_scalar_for_test(&scalar)
}

fn desktop_key() -> Keypair {
    key_from_hex(fixture!("keys")["desktop"]["private_hex"].as_str().unwrap())
}

fn device_key() -> Keypair {
    key_from_hex(fixture!("keys")["device"]["private_hex"].as_str().unwrap())
}

fn link() -> LinkKey {
    LinkKey::from_b64u(&s(&fixture!("link_key"), "key")).unwrap()
}

const SEALED: [&str; 8] = [
    "open",
    "session",
    "cmd",
    "reply",
    "panes",
    "reopen",
    "pair_check",
    "bad_sig_reply",
];

fn sealed(name: &str) -> Value {
    match name {
        "open" => fixture!("sealed_open"),
        "session" => fixture!("sealed_session"),
        "cmd" => fixture!("sealed_cmd"),
        "reply" => fixture!("sealed_reply"),
        "panes" => fixture!("sealed_panes"),
        "reopen" => fixture!("sealed_reopen"),
        "pair_check" => fixture!("sealed_pair_check"),
        "bad_sig_reply" => fixture!("sealed_bad_sig_reply"),
        _ => panic!("{name}"),
    }
}

#[test]
fn test_keys_derive_the_published_public_keys() {
    let k = fixture!("keys");
    assert_eq!(desktop_key().public_b64u(), s(&k["desktop"], "public"));
    assert_eq!(device_key().public_b64u(), s(&k["device"], "public"));
    // The key-agreement keys too: `dk` is the desktop's, `ek` the device's.
    assert_eq!(
        ecdh_from_hex(&s(&k["dk"], "private_hex")).public_b64u(),
        s(&k["dk"], "public")
    );
    assert_eq!(
        ecdh_from_hex(&s(&k["ek"], "private_hex")).public_b64u(),
        s(&k["ek"], "public")
    );
    assert!(keys::valid_ecdh_pubkey(&s(&k["dk"], "public")));
    assert!(keys::valid_ecdh_pubkey(&s(&k["ek"], "public")));
}

#[test]
fn signed_strings_match_the_server() {
    let k = fixture!("keys");
    let ss = &k["signed_strings"];
    let (desk, dev, pk, ek) = (
        s(&k["desktop"], "desktop_id"),
        s(&k["device"], "device_id"),
        s(&k["device"], "public"),
        s(&k["ek"], "public"),
    );
    assert_eq!(
        proto::hello_signed(&desk, &s(&k, "origin"), &s(&k, "nonce")),
        s(ss, "desktop_hello")
    );
    assert_eq!(
        proto::pair_proof_msg(&desk, &dev, &pk, &ek),
        s(ss, "pair_proof_message")
    );
    let cmd = sealed("cmd");
    assert_eq!(
        proto::cmd_signed(&s(&cmd, "body")),
        s(&cmd, "signed_string").into_bytes()
    );
    assert_eq!(s(ss, "cmd"), s(&cmd, "signed_string"));
    // The device signs its pair request over its own id and the nonce (not
    // over `ek`, which the proof covers).
    assert_eq!(
        s(ss, "pair_request"),
        format!("ymux-remote-v1|pair|{dev}|{}", s(&k, "nonce"))
    );
    assert!(keys::verify(
        &pk,
        s(ss, "pair_request").as_bytes(),
        &s(&fixture!("pair_request"), "sig")
    ));
}

#[test]
fn hello_matches_the_fixture_and_is_bound_to_the_origin() {
    let k = fixture!("keys");
    let want = fixture!("hello_desktop");
    let mut e = Engine::new(&s(&k["desktop"], "desktop_id"), "My PC", desktop_key());
    e.set_origin(&s(&k, "origin"));
    e.set_identity("My PC", &s(&k, "setup_token"));
    e.on_connect();
    e.drain();
    e.on_text(&fixture!("challenge").to_string(), 0);
    let sent: Vec<Value> = e
        .drain()
        .into_iter()
        .filter_map(|o| match o {
            Out::Send(f) => serde_json::from_str(&f).ok(),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1);
    let mut got = sent[0].clone();
    // Everything but the signature is the fixture, field for field.
    let got_sig = got.as_object_mut().unwrap().remove("sig").unwrap();
    let mut want_nosig = want.clone();
    let want_sig = want_nosig.as_object_mut().unwrap().remove("sig").unwrap();
    assert_eq!(got, want_nosig);
    // ECDSA here is RFC 6979 deterministic, but what must hold is that each
    // signature verifies for the same signed string.
    let signed = s(&k["signed_strings"], "desktop_hello");
    let public = s(&k["desktop"], "public");
    assert!(keys::verify(
        &public,
        signed.as_bytes(),
        got_sig.as_str().unwrap()
    ));
    assert!(keys::verify(
        &public,
        signed.as_bytes(),
        want_sig.as_str().unwrap()
    ));
    // The hello signed for another origin does not verify for ours, and does
    // for the one it names.
    let other = fixture!("hello_desktop_other_origin");
    let other_sig = other["sig"].as_str().unwrap();
    assert!(!keys::verify(&public, signed.as_bytes(), other_sig));
    let evil = proto::hello_signed(
        &s(&k["desktop"], "desktop_id"),
        &s(&other, "signed_for_origin"),
        &s(&k, "nonce"),
    );
    assert!(keys::verify(&public, evil.as_bytes(), other_sig));
}

#[test]
fn a_challenge_for_another_origin_is_not_signed() {
    let k = fixture!("keys");
    let mut e = Engine::new(&s(&k["desktop"], "desktop_id"), "My PC", desktop_key());
    e.set_origin("https://evil.example");
    e.on_connect();
    e.drain();
    e.on_text(&fixture!("challenge").to_string(), 0);
    assert_eq!(e.drain(), vec![Out::Close("origin_mismatch".into())]);
}

#[test]
fn the_link_key_is_hkdf_over_the_ecdh_secret_from_either_private_key() {
    let lk = fixture!("link_key");
    let code = keys::b64u_decode(&s(&lk, "code")).unwrap();
    assert_eq!(code, hex_to_bytes(&s(&lk, "code_hex")));
    assert_eq!(code, hex_to_bytes(&s(&lk, "salt_hex")));
    let dk = ecdh_from_hex(&s(&lk["dk"], "private_hex"));
    let ek = ecdh_from_hex(&s(&lk["ek"], "private_hex"));
    let (dk_pub, ek_pub) = (s(&lk["dk"], "public"), s(&lk["ek"], "public"));
    // The shared secret is the same from both sides, and the recorded one.
    let want_x = hex_to_bytes(&s(&lk, "shared_secret_hex"));
    assert_eq!(dk.shared_x(&ek_pub).unwrap().to_vec(), want_x);
    assert_eq!(ek.shared_x(&dk_pub).unwrap().to_vec(), want_x);
    // The info string is the exact recorded one.
    let (desk, dev, pk) = (s(&lk, "desktop_id"), s(&lk, "device_id"), s(&lk, "pubkey"));
    assert_eq!(
        proto::link_info(&desk, &dev, &pk, &ek_pub, &dk_pub),
        s(&lk, "info")
    );
    // And the key: derived by the desktop (dk private, ek public) and by the
    // device (ek private, dk public), both the recorded K.
    let params = LinkParams {
        desktop_id: &desk,
        device_id: &dev,
        pubkey: &pk,
        ek: &ek_pub,
        dk: &dk_pub,
    };
    let by_desktop = LinkKey::derive(&dk, &ek_pub, &code, &params).unwrap();
    let by_device = LinkKey::derive(&ek, &dk_pub, &code, &params).unwrap();
    assert_eq!(by_desktop.to_b64u(), s(&lk, "key"));
    assert_eq!(by_device.to_b64u(), s(&lk, "key"));
    assert_eq!(b64u(&hex_to_bytes(&s(&lk, "key_hex"))), s(&lk, "key"));
    // The code is the one in keys.json, and the key is the one the sealed
    // fixtures were made with.
    assert_eq!(s(&lk, "code"), s(&fixture!("keys"), "pair_code"));
    assert_eq!(
        by_desktop.to_b64u(),
        b64u(&hex_to_bytes(&s(&sealed("open"), "link_key_hex")))
    );
}

#[test]
fn the_pair_check_is_sealed_and_the_challenge_frame_matches() {
    let k = fixture!("keys");
    let want = sealed("pair_check");
    // 048213, as a draw: the number is random in real use, the shape is not.
    let c = Check::from_source(|| 48_213);
    assert_eq!(c.as_str(), s(&k, "pair_check"));
    let plain: Value = serde_json::from_str(&s(&want, "plaintext")).unwrap();
    assert_eq!(plain, json!({"k": "pair_check", "check": c.as_str()}));
    // The frame the desktop sends and what the pairing socket receives.
    let dev = s(&k["device"], "device_id");
    let frame: Value =
        serde_json::from_str(&proto::pair_challenge_frame(&dev, &s(&want, "body"))).unwrap();
    assert_eq!(frame, fixture!("pair_challenge"));
    let fwd = fixture!("pair_challenge_forwarded");
    assert_eq!(fwd["body"], frame["body"]);
    assert_eq!(fwd["t"], "pair_challenge");
    // Only the device's link key opens it, and only with the d2p data.
    let opened = link()
        .open(&s(&want, "aad"), &s(&want, "body"))
        .expect("opens under K");
    assert_eq!(String::from_utf8(opened).unwrap(), s(&want, "plaintext"));
}

#[test]
fn every_sealed_body_opens_and_reseals_to_the_same_bytes() {
    let k = link();
    for name in SEALED {
        let f = sealed(name);
        let (aad, body, plain) = (s(&f, "aad"), s(&f, "body"), s(&f, "plaintext"));
        let dir = s(&f, "direction");
        let desktop = s(&fixture!("keys")["desktop"], "desktop_id");
        let device = s(&fixture!("keys")["device"], "device_id");
        assert_eq!(
            aad,
            if dir == "p2d" {
                proto::aad_p2d(&desktop, &device)
            } else {
                proto::aad_d2p(&desktop, &device)
            },
            "{name}"
        );
        // Opens, to exactly the recorded plaintext.
        let opened = k
            .open(&aad, &body)
            .unwrap_or_else(|| panic!("{name} opens"));
        assert_eq!(String::from_utf8(opened).unwrap(), plain, "{name}");
        // The other direction's additional data refuses it.
        let other = if dir == "p2d" {
            proto::aad_d2p(&desktop, &device)
        } else {
            proto::aad_p2d(&desktop, &device)
        };
        assert!(k.open(&other, &body).is_none(), "{name} wrong direction");
        // Sealing the plaintext with the recorded nonce gives the body back.
        let nonce = hex_to_bytes(&s(&f, "nonce_hex"));
        assert_eq!(
            k.seal_with_nonce(&aad, plain.as_bytes(), &nonce).unwrap(),
            body,
            "{name} reseals"
        );
    }
}

#[test]
fn the_recorded_signatures_verify() {
    let device_pub = s(&fixture!("keys")["device"], "public");
    for name in ["open", "cmd"] {
        let f = sealed(name);
        assert!(
            keys::verify(
                &device_pub,
                s(&f, "signed_string").as_bytes(),
                &s(&f, "sig")
            ),
            "{name}"
        );
        assert_eq!(f["frame"]["sig"], f["sig"]);
        assert_eq!(f["frame"]["body"], f["body"]);
    }
    // The relay frames carry the same body and signature.
    let cmd = sealed("cmd");
    let device_frame = fixture!("cmd_device");
    assert_eq!(device_frame["body"], cmd["body"]);
    assert_eq!(device_frame["sig"], cmd["sig"]);
    let fwd = fixture!("cmd_forwarded");
    assert_eq!(fwd["body"], cmd["body"]);
    assert_eq!(fwd["sig"], cmd["sig"]);
}

#[test]
fn every_server_to_desktop_fixture_parses() {
    assert!(matches!(
        proto::parse_server_frame(&fixture!("challenge").to_string()),
        Some(ServerFrame::Challenge { v: 1, origin, .. }) if origin == "https://ymux.example.com"
    ));
    assert!(matches!(
        proto::parse_server_frame(&fixture!("ready_desktop").to_string()),
        Some(ServerFrame::Ready { devices }) if devices.len() == 1
    ));
    assert!(matches!(
        proto::parse_server_frame(&fixture!("device_online").to_string()),
        Some(ServerFrame::DeviceOnline { .. })
    ));
    assert!(matches!(
        proto::parse_server_frame(&fixture!("device_offline").to_string()),
        Some(ServerFrame::DeviceOffline { .. })
    ));
    assert!(matches!(
        proto::parse_server_frame(&fixture!("cmd_forwarded").to_string()),
        Some(ServerFrame::Cmd { .. })
    ));
    assert!(matches!(
        proto::parse_server_frame(&fixture!("pair_request_forwarded").to_string()),
        Some(ServerFrame::PairRequest { .. })
    ));
    assert!(matches!(
        proto::parse_server_frame(&fixture!("error").to_string()),
        Some(ServerFrame::Error { code }) if code == "bad_token"
    ));
    // Frames meant for the device are not this side's business.
    for f in [
        fixture!("desktop_online"),
        fixture!("msg_forwarded"),
        fixture!("pair_challenge_forwarded"),
    ] {
        assert!(matches!(
            proto::parse_server_frame(&f.to_string()),
            Some(ServerFrame::Unknown)
        ));
    }
}

#[test]
fn the_pairing_proof_matches_the_server() {
    let k = fixture!("keys");
    let code = keys::b64u_decode(&s(&k, "pair_code")).unwrap();
    let (desk, dev, pk, ek) = (
        s(&k["desktop"], "desktop_id"),
        s(&k["device"], "device_id"),
        s(&k["device"], "public"),
        s(&k["ek"], "public"),
    );
    assert_eq!(
        keys::pair_proof(&code, &desk, &dev, &pk, &ek),
        s(&k, "pair_proof")
    );
    assert_eq!(s(&fixture!("pair_request"), "proof"), s(&k, "pair_proof"));
    let fwd = fixture!("pair_request_forwarded");
    assert_eq!(s(&fwd, "proof"), s(&k, "pair_proof"));
    assert!(keys::verify_pair_proof(
        &code,
        &desk,
        &dev,
        &pk,
        &ek,
        &s(&fwd, "proof")
    ));
    // The proof covers `ek`: another one does not verify.
    let other = EcdhKey::generate().public_b64u();
    assert!(!keys::verify_pair_proof(
        &code,
        &desk,
        &dev,
        &pk,
        &other,
        &s(&fwd, "proof")
    ));
    match proto::parse_server_frame(&fwd.to_string()) {
        Some(ServerFrame::PairRequest {
            device_id,
            pubkey,
            ek: got_ek,
            ..
        }) => {
            assert_eq!((device_id, pubkey, got_ek), (dev, pk, ek));
        }
        f => panic!("{f:?}"),
    }
}

#[test]
fn outgoing_frames_match_the_fixtures() {
    let k = fixture!("keys");
    let dev = s(&k["device"], "device_id");
    let pane = s(&k, "pane");
    let json = |s: String| serde_json::from_str::<Value>(&s).unwrap();
    assert_eq!(json(proto::pair_open_frame(120)), fixture!("pair_open"));
    assert_eq!(json(proto::pair_close_frame()), fixture!("pair_close"));
    assert_eq!(
        json(proto::device_remove_frame(&dev)),
        fixture!("device_remove")
    );
    assert_eq!(
        json(proto::pair_result_frame(
            &dev,
            Some((&s(&k["device"], "public"), "iPhone"))
        )),
        fixture!("pair_result_desktop")
    );
    assert_eq!(
        json(proto::notify_frame(
            "waiting",
            "build",
            "needs an answer",
            &pane
        )),
        fixture!("notify")
    );
    let reply = sealed("reply");
    assert_eq!(
        json(proto::msg_frame_str(&dev, &s(&reply, "body"))),
        fixture!("msg")
    );
}

#[test]
fn the_bad_sig_reply_ref_is_the_hash_of_the_sig_string() {
    use sha2::{Digest, Sha256};
    let f = sealed("bad_sig_reply");
    let digest = Sha256::digest(s(&f, "ref_sig").as_bytes());
    assert_eq!(
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        s(&f, "ref_sha256_hex")
    );
    assert_eq!(
        hex_to_bytes(&s(&f, "ref_first16_hex")),
        digest[..16].to_vec()
    );
    let plain: Value = serde_json::from_str(&s(&f, "plaintext")).unwrap();
    assert_eq!(plain["ref"], proto::sig_ref(&s(&f, "ref_sig")));
    assert_eq!(
        proto::reply_err_ref(proto::Code::BadSig, &s(&plain, "ref")),
        json!({"k":"reply","re":0,"ok":false,"code":"bad_sig","ref":plain["ref"]})
    );
}

/// An engine past the handshake that knows the recorded device.
fn recorded_engine(cap: &str) -> (Engine, RemoteDevice) {
    let k = fixture!("keys");
    let device = RemoteDevice {
        id: s(&k["device"], "device_id"),
        name: "iPhone".into(),
        pubkey: s(&k["device"], "public"),
        cap: cap.into(),
        added_ms: 1,
    };
    let mut e = Engine::new(&s(&k["desktop"], "desktop_id"), "My PC", desktop_key());
    e.set_origin(&s(&k, "origin"));
    e.set_devices(vec![(device.clone(), link())]);
    e.on_connect();
    e.on_text(&fixture!("challenge").to_string(), 0);
    e.on_text(&fixture!("ready_desktop").to_string(), 0);
    e.drain();
    (e, device)
}

/// Everything the desktop sealed for the recorded device in `out`, opened.
fn opened(out: &[Out]) -> Vec<Value> {
    let k = link();
    let k_ids = fixture!("keys");
    let aad = proto::aad_d2p(
        &s(&k_ids["desktop"], "desktop_id"),
        &s(&k_ids["device"], "device_id"),
    );
    out.iter()
        .filter_map(|o| match o {
            Out::Send(f) => serde_json::from_str::<Value>(f).ok(),
            _ => None,
        })
        .filter(|f| f["t"] == "msg")
        .map(|f| {
            serde_json::from_slice(&k.open(&aad, f["body"].as_str().unwrap()).expect("opens"))
                .unwrap()
        })
        .collect()
}

fn plaintext(name: &str) -> Value {
    serde_json::from_str(&s(&sealed(name), "plaintext")).unwrap()
}

#[test]
fn the_recorded_conversation_runs_through_the_engine() {
    let k = fixture!("keys");
    let now = k["now_ms"].as_u64().unwrap();
    let (mut e, device) = recorded_engine("respond");

    // 1. A session (id 0x04.., three commands accepted in it) is replaced by
    //    the sealed, signed `open`: the `session` reply echoes the nonce, carries
    //    the clock and `prev` = what the revoked one had accepted. It is the
    //    recorded plaintext but for the (random) session id.
    e.adopt_session_with_seq_for_test(&device.id, &s(&k, "prev_session"), 3);
    let mut open = sealed("open")["frame"].clone();
    open["device_id"] = device.id.clone().into();
    open["conn"] = k["conn"].clone();
    e.on_text(&open.to_string(), now);
    let out = e.drain();
    let replies = opened(&out);
    assert_eq!(replies.len(), 1);
    let mut want = plaintext("session");
    assert_ne!(replies[0]["session"], want["session"], "a fresh random id");
    want["session"] = replies[0]["session"].clone();
    assert_eq!(replies[0], want);

    // From here on the recorded session id, as the recorded commands name it.
    e.adopt_session_for_test(&device.id, &s(&k, "session"));

    // 2. The sealed `answer` reaches the page with its menu id...
    e.on_text(&fixture!("cmd_forwarded").to_string(), now + 100);
    let out = e.drain();
    let (id, payload) = out
        .iter()
        .find_map(|o| match o {
            Out::Request { id, payload } => Some((*id, payload.clone())),
            _ => None,
        })
        .expect("the answer reached the page");
    let cmd = &plaintext("cmd")["cmd"];
    assert_eq!(payload["op"], "answer");
    assert_eq!(payload["pane"], cmd["pane"]);
    assert_eq!(payload["n"], cmd["n"]);
    assert_eq!(payload["label"], cmd["label"]);
    assert_eq!(payload["menu_id"], cmd["menu_id"]);

    // 3. ...and what goes back is exactly the recorded reply (`n` 1, `at_ms`).
    e.frontend_reply(id, &json!({"result": "sent"}), now + 150);
    let replies = opened(&e.drain());
    assert_eq!(replies, vec![plaintext("reply")]);

    // 4. A panes push, then a capability change's `reopen`, continue the `n`.
    e.publish_panes(
        &json!([{"pane": s(&k, "pane"), "ws": "main", "title": "build", "kind": "claude",
                 "status": "waiting", "since_ms": now, "queued": 0}]),
        now + 1_000,
    );
    let panes: Vec<Value> = opened(&e.drain());
    assert_eq!(panes, vec![plaintext("panes")]);

    let mut view = device.clone();
    view.cap = "view".into();
    e.set_devices(vec![(view, link())]);
    e.device_cap_changed(&device.id, now + 2_000);
    assert_eq!(opened(&e.drain()), vec![plaintext("reopen")]);

    // 5. The same command again is refused (its session is gone now anyway).
    e.on_text(&fixture!("cmd_forwarded").to_string(), now + 2_100);
    let out = e.drain();
    assert!(!out.iter().any(|o| matches!(o, Out::Request { .. })));
    assert_eq!(opened(&out)[0]["code"], "bad_session");
}

#[test]
fn a_command_whose_signature_fails_gets_the_recorded_bad_sig_reply() {
    let k = fixture!("keys");
    let now = k["now_ms"].as_u64().unwrap();
    let (mut e, device) = recorded_engine("respond");
    e.adopt_session_for_test(&device.id, &s(&k, "session"));
    // The recorded command, its body altered by one character: the signature
    // (the recorded string, whose hash is `ref`) no longer fits it.
    let mut frame = fixture!("cmd_forwarded");
    assert_eq!(s(&frame, "sig"), s(&sealed("bad_sig_reply"), "ref_sig"));
    let mut body: Vec<char> = s(&frame, "body").chars().collect();
    body[20] = if body[20] == 'A' { 'B' } else { 'A' };
    frame["body"] = body.into_iter().collect::<String>().into();
    e.on_text(&frame.to_string(), now + 50);
    let out = e.drain();
    assert!(!out.iter().any(|o| matches!(o, Out::Request { .. })));
    assert_eq!(opened(&out), vec![plaintext("bad_sig_reply")]);
}

#[test]
fn the_recorded_command_is_refused_without_a_session_and_when_stale() {
    let k = fixture!("keys");
    let now = k["now_ms"].as_u64().unwrap();
    // No `open` was ever sent.
    let (mut e, _) = recorded_engine("respond");
    e.on_text(&fixture!("cmd_forwarded").to_string(), now);
    let out = e.drain();
    assert!(!out.iter().any(|o| matches!(o, Out::Request { .. })));
    assert_eq!(opened(&out)[0]["code"], "bad_session");
    // Delivered more than 30 s late (the command is stamped now + 123).
    let (mut e, device) = recorded_engine("respond");
    e.adopt_session_for_test(&device.id, &s(&k, "session"));
    e.on_text(&fixture!("cmd_forwarded").to_string(), now + 123 + 30_001);
    let out = e.drain();
    assert!(!out.iter().any(|o| matches!(o, Out::Request { .. })));
    assert_eq!(opened(&out)[0]["code"], "expired");
    // ...and exactly 30 s late is still fresh.
    let (mut e, device) = recorded_engine("respond");
    e.adopt_session_for_test(&device.id, &s(&k, "session"));
    e.on_text(&fixture!("cmd_forwarded").to_string(), now + 123 + 30_000);
    assert!(e.drain().iter().any(|o| matches!(o, Out::Request { .. })));
    // A view-only device may not answer.
    let (mut e, device) = recorded_engine("view");
    e.adopt_session_for_test(&device.id, &s(&k, "session"));
    e.on_text(&fixture!("cmd_forwarded").to_string(), now);
    assert_eq!(opened(&e.drain())[0]["code"], "forbidden");
}
