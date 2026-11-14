//! Live end-to-end drivers (manual checks, not part of CI). The REAL socket
//! loop (`Shared::run`) and the REAL engine talk to a real yMux-Server; a fake
//! "page" answers `remote://request` with canned data and a fake user approves
//! the pairing request. The device side is a separate process (the PWA's own
//! protocol code, `web/scripts/e2e-*.ts` in the yMux-Server repo); the two
//! rendezvous through small files in `YMUX_E2E_DIR`.
//!
//! Three `#[ignore]`d tests, all started by scripts of the yMux-Server repo:
//!
//! - `live_pair_session_commands_reconnect_removal` (`scripts/e2e.ps1`): one
//!   desktop, one device, every rejection path of PROTOCOL.md, exact counts of
//!   what reached the page.
//!   Three pairing rounds first, each asked for by the device
//!   (`d.pair<k>` holds a plan name) and answered with `pair-url-<k>.txt`:
//!   1 the user types a wrong number three times, 2 two devices use one
//!   code, 3 the right number. Then, desktop -> device: `h.bumped` (the
//!   page's input generation advanced), `h.reconnected`, `h.capview`,
//!   `h.removed`; device -> desktop: `pair-check-device.txt` (the number the
//!   device shows after opening the sealed `pair_challenge`; the fake user
//!   "types" it, polling up to 20 s), `d.pairend<k>`, `d.bump`, `d.session`,
//!   `d.phaseA`, `d.phaseR`, `d.phaseB`, `d.done`.
//! - `live_fleet_member` (`scripts/e2e.ps1`, two instances with
//!   `YMUX_E2E_VARIANT=A|B`, each with its own `YMUX_E2E_DIR`): one of two
//!   desktops that one device pairs with.
//!   desktop -> device: `pair-url.txt`, `h.stopped`, `h.started`,
//!   `h.removed`; device -> desktop: `pair-check-device.txt`, `d.session`,
//!   `d.stop`, `d.start`, `d.remove`, `d.done`.
//! - `demo_fake_desktop` (`scripts/demo.ps1`): a desktop with sample data that
//!   stays up until killed, for looking at the PWA.
//!
//! Test code only: nothing here is compiled into a build of the app.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;

use super::*;

struct E2eHost {
    cfg: Mutex<RemoteConfig>,
    requests: Mutex<mpsc::Sender<Value>>,
    pairs: Mutex<mpsc::Sender<Value>>,
    /// Set when the engine reports two devices on one code.
    conflict: AtomicBool,
}

impl Host for E2eHost {
    fn cfg(&self) -> RemoteConfig {
        self.cfg.lock().clone()
    }
    fn emit_status(&self) {}
    fn emit_request(&self, payload: Value) {
        let _ = self.requests.lock().send(payload);
    }
    fn emit_pair(&self, payload: Value) {
        let _ = self.pairs.lock().send(payload);
    }
    fn emit_pair_conflict(&self) {
        self.conflict.store(true, Ordering::SeqCst);
    }
    fn drop_devices(&self, ids: &[String]) {
        self.cfg.lock().devices.retain(|d| !ids.contains(&d.id));
    }
    fn reset_identity(&self, new_desktop_id: &str) -> bool {
        let mut c = self.cfg.lock();
        c.desktop_id = new_desktop_id.to_string();
        c.registered = false;
        c.devices.clear();
        true
    }
    fn set_desktop_id(&self, id: &str) {
        self.cfg.lock().desktop_id = id.to_string();
    }
    fn mark_registered(&self, ws_url_of_connection: &str) {
        let mut c = self.cfg.lock();
        if super::super::ws_url(&c.server_url).as_deref() == Ok(ws_url_of_connection) {
            c.registered = true;
        }
    }
}

impl E2eHost {
    fn new(cfg: RemoteConfig) -> (Arc<Self>, mpsc::Receiver<Value>, mpsc::Receiver<Value>) {
        let (req_tx, req_rx) = mpsc::channel();
        let (pair_tx, pair_rx) = mpsc::channel();
        let host = Arc::new(E2eHost {
            cfg: Mutex::new(cfg),
            requests: Mutex::new(req_tx),
            pairs: Mutex::new(pair_tx),
            conflict: AtomicBool::new(false),
        });
        (host, req_rx, pair_rx)
    }
}

const WAIT: Duration = Duration::from_secs(90);

// ------------------------------------------------------------------ the page

/// What the fake page does with a `remote://request` and what it lists.
trait PageLogic: Send + Sync {
    fn panes(&self) -> Value;
    fn handle(&self, payload: &Value) -> Value;
}

const P1: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const P2: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
/// The one pane uuid both fleet desktops list, with a different title each.
const SHARED: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const ONLY_A: &str = "a0a0a0a0-a0a0-4a0a-8a0a-a0a0a0a0a0a0";
const ONLY_B: &str = "b0b0b0b0-b0b0-4b0b-8b0b-b0b0b0b0b0b0";

/// An option label longer than the 120-char clip, so the device must echo the
/// clipped copy back.
fn long_label() -> String {
    // Words, not one long run: the redactor treats a long unbroken run as a key.
    format!(
        "Yes, and allow this exact command for the rest of the session {}",
        "and so on ".repeat(12)
    )
}

/// The page's rule for an echoed option label (`labelMatches` in
/// `src/remote/remoteModel.ts`): equal, or a clipped copy (ellipsis) that is a
/// prefix of the screen's label.
fn label_matches(on_screen: &str, got: &str) -> bool {
    if on_screen == got {
        return true;
    }
    let head = got
        .strip_suffix('…')
        .or_else(|| got.strip_suffix("..."))
        .unwrap_or(got);
    head.len() != got.len() && head.chars().count() >= 20 && on_screen.starts_with(head)
}

/// Fixed answers with one waiting menu. `tag` ends up in the menu id and the
/// screen lines, so a body that reached the wrong desktop is recognisable.
struct Canned {
    tag: &'static str,
    panes: Mutex<Value>,
    /// The pane's input generation: anything written to its terminal advances
    /// it, and a menu id is only good for the generation it was minted at.
    generation: Mutex<u32>,
}

impl Canned {
    fn new(tag: &'static str, panes: Value) -> Arc<Self> {
        Arc::new(Self {
            tag,
            panes: Mutex::new(panes),
            generation: Mutex::new(0),
        })
    }
    fn menu_id(&self) -> String {
        match *self.generation.lock() {
            0 => format!("e2e-menu-{}", self.tag),
            g => format!("e2e-menu-{}-{g}", self.tag),
        }
    }
    /// Someone typed into the pane (another device, the user): every menu id
    /// minted so far is stale.
    fn bump_input_generation(&self) {
        *self.generation.lock() += 1;
    }
}

impl PageLogic for Canned {
    fn panes(&self) -> Value {
        self.panes.lock().clone()
    }

    fn handle(&self, payload: &Value) -> Value {
        match payload["op"].as_str().unwrap_or("") {
            "list" => json!({"panes": self.panes()}),
            "peek" => json!({
                "lines": [format!("$ claude ({})", self.tag), "Allow this command?", "ls -la"],
                "menu": [
                    {"n": 1, "label": "Yes", "selected": true, "input": false},
                    {"n": 2, "label": long_label(), "selected": false, "input": false},
                ],
                "menu_id": self.menu_id(),
                "question": ["Allow this command?"],
                "blocked": null,
                "status": "waiting",
            }),
            "answer" => {
                let n = payload["n"].as_u64().unwrap_or(0);
                let label = payload["label"].as_str().unwrap_or("");
                let screen = match n {
                    1 => "Yes".to_string(),
                    2 => long_label(),
                    _ => String::new(),
                };
                // The page's rule: the id must be the pane's current one AND
                // the option must still be on screen.
                let ok = payload["menu_id"] == self.menu_id().as_str()
                    && !screen.is_empty()
                    && label_matches(&screen, label);
                json!({"result": if ok { "sent" } else { "stale" }})
            }
            "interrupt" => json!({"result": "sent"}),
            "prompt" if payload["text"] == HANG_PROMPT => Value::Null,
            "prompt" => {
                let queued = payload["queue"].as_bool().unwrap_or(false);
                json!({"result": if queued { "queued" } else { "sent" }})
            }
            _ => json!({}),
        }
    }
}

/// A prompt the fake page never answers (it is "still typing").
const HANG_PROMPT: &str = "still typing";

fn single_panes(p2_status: &str, queued: u32) -> Value {
    json!([
        {"pane": P1, "ws": "main", "title": "Claude one", "kind": "claude",
         "status": "idle", "since_ms": 1_700_000_000_000u64, "queued": 0},
        {"pane": P2, "ws": "main", "title": "Codex two", "kind": "codex",
         "status": p2_status, "since_ms": 1_700_000_001_000u64, "queued": queued},
    ])
}

/// The two fleet desktops: one pane each of their own, and `SHARED` on both.
fn fleet_panes(variant: &str) -> Value {
    let (own, own_title, shared_title) = match variant {
        "A" => (ONLY_A, "Alpha only", "Shared as seen from A"),
        _ => (ONLY_B, "Beta only", "Shared as seen from B"),
    };
    json!([
        {"pane": SHARED, "ws": "fleet", "title": shared_title, "kind": "claude",
         "status": "idle", "since_ms": 1_700_000_000_000u64, "queued": 0},
        {"pane": own, "ws": "fleet", "title": own_title, "kind": "codex",
         "status": "idle", "since_ms": 1_700_000_001_000u64, "queued": 0},
    ])
}

// -------------------------------------------------------------- the desktop

struct Fake {
    sh: Arc<Shared>,
    host: Arc<E2eHost>,
    seen: Arc<Mutex<Vec<Value>>>,
    page: Arc<dyn PageLogic>,
    /// This desktop's rendezvous directory.
    dir: PathBuf,
    base: String,
    /// How the fake user answers the next pairing request.
    typing: Arc<Mutex<Typing>>,
    /// One line per request the fake user answered.
    pair_log: Arc<Mutex<Vec<String>>>,
    /// The device name of each pairing request, as the user was shown it.
    pair_names: Arc<Mutex<Vec<String>>>,
}

/// Write a rendezvous file so that a reader never sees half of it.
fn put(dir: &Path, name: &str, text: &str) {
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, text).unwrap();
    std::fs::rename(&tmp, dir.join(name)).unwrap();
}

fn touch(dir: &Path, name: &str) {
    put(dir, name, "1");
}

fn wait_file(dir: &Path, name: &str) {
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        if dir.join(name).exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {name} from the device side");
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

/// `Shared` of a desktop that is already paired: for `Fake::start`.
struct Start<'a> {
    base: &'a str,
    token: &'a str,
    dir: &'a Path,
    name: &'a str,
    page: Arc<dyn PageLogic>,
    /// Open a fresh pairing window after every request is answered (the demo).
    reopen_after_pair: bool,
    /// How the fake user answers requests until told otherwise.
    typing: Typing,
}

/// How the fake user gets the number to type.
#[derive(Clone, Debug)]
enum Typing {
    /// A person reading their phone: the number the device script wrote to
    /// `pair-check-device.txt` after opening the sealed `pair_challenge`.
    Device,
    /// A person who mistypes: the wrong number every time.
    Wrong,
    /// Two devices used one code: wait for the engine to say so; nothing can
    /// be approved afterwards.
    Conflict,
    /// Demo only (`-AutoApprove`): the number the engine minted itself, taken
    /// through a `#[cfg(test)]` accessor. No production path can do this.
    Auto,
    /// Demo: a person types the number into this file.
    File(PathBuf),
}

/// The way the app stores an approved device: link key to the secrets store,
/// device to the config, then the engine trusts it.
fn persist_with<'a>(
    sh: &'a Shared,
    host: &'a E2eHost,
) -> impl FnMut(&RemoteDevice, &LinkKey) -> bool + 'a {
    move |device, link| {
        if sh
            .secrets
            .set(&link_secret_id(&device.id), "secret", &link.to_b64u())
            .is_err()
        {
            return false;
        }
        let mut cfg = host.cfg.lock();
        cfg.devices.retain(|d| d.id != device.id);
        cfg.devices.push(device.clone());
        true
    }
}

/// The fake user's answer to one pairing request; the returned line says what
/// happened (the test asserts on it).
fn user_answers(sh: &Shared, host: &E2eHost, dir: &Path, id: &str, how: &Typing) -> String {
    let decide = |number: &str| -> Result<PairOutcome, &'static str> {
        let mut persist = persist_with(sh, host);
        sh.with_engine(|e| e.pair_decide(id, true, "prompt", number, now_ms(), &mut persist))
            .expect("engine")
    };
    let reject = || {
        let _ = sh.with_engine(|e| e.pair_decide(id, false, "", "", now_ms(), &mut |_, _| true));
    };
    let engine_check = || sh.with_engine(|e| e.pending_check_for_test(id)).flatten();
    match how {
        Typing::Conflict => {
            wait_until("the engine to report two devices on one code", || {
                host.conflict.load(Ordering::SeqCst)
            });
            // Whatever the user types now, both requests are already refused.
            format!("conflict, then {:?}", decide("000000"))
        }
        Typing::Wrong => {
            let Some(shown) = read_device_check(dir) else {
                reject();
                return "no number shown".into();
            };
            let wrong = if shown == "000000" {
                "000001"
            } else {
                "000000"
            };
            let mut seen = Vec::new();
            for _ in 0..4 {
                match decide(wrong) {
                    Ok(PairOutcome::Wrong { tries_left }) => {
                        seen.push(format!("wrong({tries_left})"))
                    }
                    Ok(PairOutcome::Refused) => {
                        seen.push("refused".into());
                        break;
                    }
                    other => {
                        seen.push(format!("unexpected {other:?}"));
                        break;
                    }
                }
            }
            seen.join(",")
        }
        Typing::Device | Typing::Auto | Typing::File(_) => loop {
            let number = match how {
                Typing::Auto => engine_check(),
                Typing::File(path) => read_typed_file(path),
                _ => read_device_check(dir),
            };
            let Some(number) = number else {
                reject();
                return "no number typed".into();
            };
            if matches!(how, Typing::Device) {
                // The phone opened the very number this desktop minted.
                assert_eq!(
                    engine_check().as_deref(),
                    Some(number.as_str()),
                    "the device's number is the desktop's"
                );
            }
            match decide(&number) {
                Ok(PairOutcome::Approved(_)) => return "approved".into(),
                // A person who mistyped tries again (up to the engine's limit).
                Ok(PairOutcome::Wrong { .. }) if matches!(how, Typing::File(_)) => continue,
                other => return format!("not approved: {other:?}"),
            }
        },
    }
}

/// A number a person typed into `path` (demo): wait up to 110 s for it, take
/// it and remove the file. Nothing in time: `None` and the user rejects.
fn read_typed_file(path: &Path) -> Option<String> {
    let end = Instant::now() + Duration::from_secs(110);
    while Instant::now() < end {
        if let Ok(text) = std::fs::read_to_string(path) {
            let n = text.trim().to_string();
            if !n.is_empty() {
                let _ = std::fs::remove_file(path);
                return Some(n);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

impl Fake {
    fn start(s: Start<'_>) -> Fake {
        std::fs::create_dir_all(s.dir.join("desktop")).unwrap();
        let desktop_id = uuid::Uuid::new_v4().to_string();
        let (host, req_rx, pair_rx) = E2eHost::new(RemoteConfig {
            enabled: true,
            server_url: s.base.to_string(),
            desktop_id,
            name: s.name.into(),
            ..RemoteConfig::default()
        });
        let secrets = Arc::new(SecretStore::new(&s.dir.join("desktop")));
        secrets.set(SECRET_ID, "api_key", s.token).unwrap();
        let sh = Shared::new(
            host.clone(),
            secrets,
            Audit::new(s.dir.join("desktop").join("audit.jsonl")),
            true,
        );
        sh.active.store(true, Ordering::Relaxed);
        {
            let worker = sh.clone();
            let h = std::thread::Builder::new()
                .name("ymux-remote".into())
                .spawn(move || worker.run())
                .unwrap();
            *sh.worker.lock() = Some(h.thread().clone());
        }
        let fake = Fake {
            sh,
            host,
            seen: Arc::new(Mutex::new(Vec::new())),
            page: s.page,
            dir: s.dir.to_path_buf(),
            base: s.base.to_string(),
            typing: Arc::new(Mutex::new(s.typing)),
            pair_log: Arc::new(Mutex::new(Vec::new())),
            pair_names: Arc::new(Mutex::new(Vec::new())),
        };

        // The fake page: answers every request, remembers what reached it.
        {
            let (sh, seen, page) = (fake.sh.clone(), fake.seen.clone(), fake.page.clone());
            std::thread::spawn(move || {
                for payload in req_rx {
                    let id = payload["id"].as_u64().unwrap();
                    let data = page.handle(&payload);
                    seen.lock().push(payload);
                    // `null`: a page that is still typing. It never answers, so
                    // the backend answers `timeout` after 5 s on its own.
                    if data.is_null() {
                        continue;
                    }
                    sh.with_engine(|e| e.frontend_reply(id, &data, now_ms()));
                }
            });
        }
        // The fake user: types the number the device shows and approves with
        // capability `prompt`.
        {
            let (sh, host, dir) = (fake.sh.clone(), fake.host.clone(), fake.dir.clone());
            let (base, reopen) = (fake.base.clone(), s.reopen_after_pair);
            let (typing, log) = (fake.typing.clone(), fake.pair_log.clone());
            let names = fake.pair_names.clone();
            std::thread::spawn(move || {
                for p in pair_rx {
                    let id = p["device_id"].as_str().unwrap().to_string();
                    let name = p["name"].as_str().unwrap_or("").to_string();
                    names.lock().push(name.clone());
                    // What a person would see: who is asking. (The number is
                    // never shown here; it is on their phone.)
                    put(&dir, "pair-pending.txt", &format!("{id} {name}"));
                    let how = typing.lock().clone();
                    let what = user_answers(&sh, &host, &dir, &id, &how);
                    let _ = std::fs::remove_file(dir.join("pair-pending.txt"));
                    eprintln!("e2e: pairing request {id}: {what}");
                    log.lock().push(what);
                    if reopen {
                        open_pairing(&sh, &host, &dir, &base, "pair-url.txt");
                    }
                }
            });
        }
        fake
    }

    fn connected(&self) -> bool {
        self.sh.with_engine(|e| e.connected()).unwrap_or(false)
    }

    fn wait_connected(&self) {
        wait_until("the server handshake (ready)", || self.connected());
        assert!(
            self.host.cfg().registered,
            "the server accepted the desktop"
        );
    }

    /// Hand the page's current panes to the engine (what the app does on change).
    fn publish(&self) {
        let panes = self.page.panes();
        self.sh.with_engine(|e| e.publish_panes(&panes, now_ms()));
    }

    /// Pairing round `k`, driven by the device script: it asks with
    /// `d.pair<k>`, the desktop opens a window and publishes
    /// `pair-url-<k>.txt`, the fake user answers as `typing` says. Returns
    /// once the device reports the round over (`d.pairend<k>`).
    fn pairing_round(&self, k: u32, typing: Typing) {
        self.pairing_round_start(k, typing);
        wait_file(&self.dir, &format!("d.pairend{k}"));
        wait_until("the fake user's answer to be logged", || {
            self.pair_log.lock().len() >= k as usize
        });
    }

    /// The first half of [`Fake::pairing_round`].
    fn pairing_round_start(&self, k: u32, typing: Typing) {
        wait_file(&self.dir, &format!("d.pair{k}"));
        *self.typing.lock() = typing;
        self.open_pairing_as(&format!("pair-url-{k}.txt"));
    }

    /// Open a pairing window and write the link to `pair-url.txt`.
    fn open_pairing(&self) {
        self.open_pairing_as("pair-url.txt");
    }

    fn open_pairing_as(&self, file: &str) {
        open_pairing(&self.sh, &self.host, &self.dir, &self.base, file);
    }

    fn set_link(&self, enabled: bool) {
        self.host.cfg.lock().enabled = enabled;
        self.sh.wake();
        if enabled {
            wait_until("the link to come back", || self.connected());
        } else {
            wait_until("the link to go down", || !self.connected());
        }
    }

    fn lower_cap_to_view(&self) {
        let devices = {
            let mut cfg = self.host.cfg.lock();
            for d in cfg.devices.iter_mut() {
                d.cap = "view".into();
            }
            cfg.devices.clone()
        };
        let id = devices[0].id.clone();
        self.sh.devset.mutate(|| {
            let paired = self.sh.paired(&devices).usable;
            self.sh.with_engine(|e| {
                e.set_devices(paired);
                e.device_cap_changed(&id, now_ms());
            });
        });
    }

    fn remove_device(&self) {
        let id = self.host.cfg().devices[0].id.clone();
        self.sh.devset.mutate(|| {
            self.host.cfg.lock().devices.clear();
            let _ = self.sh.secrets.set(&link_secret_id(&id), "secret", "");
            self.sh.with_engine(|e| {
                e.set_devices(Vec::new());
                e.device_removed(&id, now_ms());
            });
        });
    }

    /// Ops (and nothing else) that reached the page, counted; `list` is left
    /// out because the device asks for it on its own after every session.
    fn counts(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for p in self.seen.lock().iter() {
            let op = p["op"].as_str().unwrap_or("?").to_string();
            if op != "list" {
                *m.entry(op).or_insert(0) += 1;
            }
        }
        m
    }

    fn assert_counts(&self, when: &str, want: &[(&str, usize)]) {
        let want: BTreeMap<String, usize> = want.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        assert_eq!(
            self.counts(),
            want,
            "what reached the page {when} (a rejected command must never get there)"
        );
    }

    /// The texts of the `prompt`s that reached the page, in order.
    fn prompt_texts(&self) -> Vec<String> {
        self.seen
            .lock()
            .iter()
            .filter(|p| p["op"] == "prompt")
            .map(|p| p["text"].as_str().unwrap_or("").to_string())
            .collect()
    }
}

/// Open a pairing window and publish its link. The `pair_open` frame leaves
/// with the loop's next pump (<= 200 ms); a person needs seconds to scan the
/// code, the device script does not (hence the wait before the link appears).
fn open_pairing(sh: &Shared, host: &E2eHost, dir: &Path, base: &str, file: &str) {
    let desktop_id = host.cfg().desktop_id;
    host.conflict.store(false, Ordering::SeqCst);
    let Some(Ok((code, dk, _))) = sh.with_engine(|e| e.pair_open(now_ms())) else {
        return;
    };
    std::thread::sleep(Duration::from_millis(800));
    put(dir, file, &pair_link(base, &desktop_id, &code, &dk));
}

/// The number the device script wrote (it opened the sealed `pair_challenge`
/// and shows it), waiting up to 20 s for it. The file is removed once read, so
/// a later pairing in the same directory (the demo) never reads a stale one.
fn read_device_check(dir: &Path) -> Option<String> {
    let end = Instant::now() + Duration::from_secs(20);
    let path = dir.join("pair-check-device.txt");
    while Instant::now() < end {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let n = text.trim().to_string();
            if !n.is_empty() {
                let _ = std::fs::remove_file(&path);
                return Some(n);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

// ------------------------------------------------------------ single desktop

#[test]
#[ignore = "needs a running yMux-Server and the device script; use scripts/e2e.ps1"]
fn live_pair_session_commands_reconnect_removal() {
    let base = env("YMUX_E2E_URL");
    let dir = PathBuf::from(env("YMUX_E2E_DIR"));
    let page = Canned::new("S", single_panes("working", 0));
    let fake = Fake::start(Start {
        base: &base,
        token: &env("YMUX_E2E_TOKEN"),
        dir: &dir,
        name: "E2E PC",
        page: page.clone(),
        reopen_after_pair: false,
        typing: Typing::Device,
    });
    fake.wait_connected();
    fake.publish();

    // Round 1: the user types a WRONG number three times. Each try is
    // counted, the third refuses the request, and nothing is paired.
    fake.pairing_round(1, Typing::Wrong);
    assert_eq!(
        fake.pair_log.lock().last().unwrap(),
        "wrong(2),wrong(1),refused"
    );
    assert!(
        fake.host.cfg().devices.is_empty(),
        "a refused request pairs nothing"
    );
    // The phone asked as `Pixel 8” — your number: 482913 “`: the user was shown
    // a name with no digit and no quote or dash in it (the server cleans it on
    // the way, the desktop again).
    assert_eq!(fake.pair_names.lock()[0], "Pixel your number");

    // Round 2: two devices present a valid proof for one code. The desktop
    // refuses both, whatever is typed afterwards.
    fake.pairing_round(2, Typing::Conflict);
    assert_eq!(
        fake.pair_log.lock().last().unwrap(),
        "conflict, then Err(\"unknown_request\")"
    );
    assert!(
        fake.host.cfg().devices.is_empty(),
        "a conflict pairs nothing"
    );

    // Round 3: the user types the number the phone shows.
    fake.pairing_round_start(3, Typing::Device);

    // Device paired, connected, got its session.
    wait_file(&dir, "d.session");
    assert_eq!(fake.pair_log.lock().len(), 3);
    assert_eq!(fake.pair_log.lock().last().unwrap(), "approved");
    assert_eq!(
        fake.host.cfg().devices.len(),
        1,
        "pairing stored one device"
    );
    assert_eq!(fake.host.cfg().devices[0].cap, "prompt");
    assert_eq!(
        fake.host.cfg().devices[0].name,
        "Test phone",
        "the stored name is the cleaned one"
    );
    // A pane changes: a `panes` push must reach the device, and the waiting
    // transition makes the engine send a `notify` frame the server accepts.
    {
        let updated = single_panes("waiting", 1);
        *page.panes.lock() = updated.clone();
        fake.sh.with_engine(|e| e.publish_panes(&updated, now_ms()));
    }

    // The device peeked a menu and is about to answer it. Something else
    // types into the pane first (the user, another device): the page's input
    // generation advances, so the id the device holds is stale.
    wait_file(&dir, "d.bump");
    page.bump_input_generation();
    touch(&dir, "h.bumped");

    wait_file(&dir, "d.phaseA");
    fake.assert_counts(
        "in phase A",
        &[
            // 1 + one per answer that peeked first (3) + the two around the bump.
            ("peek", 6),
            // 1, long label, altered label, a made-up id, the id from before
            // the bump (stale), the id after it (sent).
            ("answer", 6),
            // Ran: the first, the one before the new session, and the one whose
            // reply a forged `desktop_offline` hid. Never got here: a
            // tampered body, a copy re-sent into a new session, one five
            // minutes old, a wrong-key body, one the relay lost.
            ("interrupt", 3),
            // "go on" and "later", and the one the page never answered (the
            // desktop said `timeout` after 5 s); the `!` one is refused on the
            // desktop and the replayed copy is a replay. A command the relay
            // lost, one that skipped a seq and a forged skip never got here.
            // "dup once" reached the page once: the relay sent its frame twice and
            // the desktop answered the second copy `replay`.
            ("prompt", 4),
        ],
    );
    assert_eq!(
        fake.prompt_texts(),
        ["go on", "later, please", HANG_PROMPT, "dup once"]
    );
    assert!(
        fake.seen.lock().iter().all(|p| p["deadline_ms"]
            .as_u64()
            .is_some_and(|d| d > 1_700_000_000_000)),
        "every request carries the epoch time of its 5 s deadline"
    );
    {
        // The long label reached the page clipped to 120 chars with an ellipsis.
        let seen = fake.seen.lock();
        let long = seen
            .iter()
            .find(|p| p["op"] == "answer" && p["n"] == 2)
            .expect("answer 2");
        let label = long["label"].as_str().unwrap();
        assert_eq!(label.chars().count(), 120);
        assert!(label.ends_with('…'));
    }

    // The desktop drops and re-establishes its link: the device must get a
    // fresh session without doing anything.
    fake.set_link(false);
    fake.set_link(true);
    touch(&dir, "h.reconnected");

    wait_file(&dir, "d.phaseR");
    // Lower the capability: the device gets a new session, `prompt` is refused.
    fake.lower_cap_to_view();
    touch(&dir, "h.capview");

    wait_file(&dir, "d.phaseB");
    // Remove the device: its socket is closed and it cannot come back.
    fake.remove_device();
    touch(&dir, "h.removed");

    wait_file(&dir, "d.done");
    fake.assert_counts(
        "at the end",
        &[("peek", 6), ("answer", 6), ("interrupt", 3), ("prompt", 5)],
    );
    assert_eq!(
        fake.prompt_texts(),
        [
            "go on",
            "later, please",
            HANG_PROMPT,
            "dup once",
            "after reconnect"
        ]
    );
    // Stop the link cleanly (this flushes the aggregated audit lines).
    fake.set_link(false);

    // The audit log: accepted reads are counted lines, every accepted write
    // has a line of its own, and the skipped seqs were noted.
    let log = std::fs::read_to_string(dir.join("desktop").join("audit.jsonl")).unwrap();
    let lines: Vec<Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let of = |op: &str, code: &str| -> Vec<&Value> {
        lines
            .iter()
            .filter(|l| l["op"] == op && l["code"] == code)
            .collect()
    };
    let peeks: u64 = of("peek", "ok")
        .iter()
        .map(|l| l["n"].as_u64().unwrap())
        .sum();
    assert_eq!(peeks, 6, "every accepted peek is counted");
    assert_eq!(
        of("interrupt", "ok").len(),
        3,
        "one line per accepted write"
    );
    assert!(
        of("interrupt", "ok").iter().all(|l| l["n"] == 1),
        "a write line is never a count"
    );
    assert_eq!(of("prompt", "timeout").len(), 1, "the unanswered prompt");
    assert!(
        lines.iter().any(|l| l["code"] == "seq_gap"),
        "a skipped seq is audited"
    );
}

// --------------------------------------------------------------- fleet member

#[test]
#[ignore = "needs a running yMux-Server and the device script; use scripts/e2e.ps1"]
fn live_fleet_member() {
    let base = env("YMUX_E2E_URL");
    let dir = PathBuf::from(env("YMUX_E2E_DIR"));
    let variant = env("YMUX_E2E_VARIANT");
    assert!(
        variant == "A" || variant == "B",
        "YMUX_E2E_VARIANT is A or B"
    );
    let (tag, name): (&'static str, &str) = if variant == "A" {
        ("A", "Desk A")
    } else {
        ("B", "Desk B")
    };
    let fake = Fake::start(Start {
        base: &base,
        token: &env("YMUX_E2E_TOKEN"),
        dir: &dir,
        name,
        page: Canned::new(tag, fleet_panes(&variant)),
        reopen_after_pair: false,
        typing: Typing::Device,
    });
    fake.wait_connected();
    fake.publish();
    fake.open_pairing();

    wait_file(&dir, "d.session");
    assert_eq!(
        fake.host.cfg().devices.len(),
        1,
        "pairing stored one device"
    );

    // Steps the device asks for, each once, until it says it is done.
    let mut done = [false; 3];
    while !dir.join("d.done").exists() {
        if !done[0] && dir.join("d.stop").exists() {
            done[0] = true;
            fake.set_link(false);
            touch(&dir, "h.stopped");
        }
        if !done[1] && dir.join("d.start").exists() {
            done[1] = true;
            fake.set_link(true);
            touch(&dir, "h.started");
        }
        if !done[2] && dir.join("d.remove").exists() {
            done[2] = true;
            fake.remove_device();
            touch(&dir, "h.removed");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Only commands addressed to THIS desktop reached its page, and nothing
    // a wrong key, a wrong signature or another desktop's frame carried.
    let own_panes: [&str; 2] = if variant == "A" {
        [SHARED, ONLY_A]
    } else {
        [SHARED, ONLY_B]
    };
    for p in fake.seen.lock().iter() {
        if let Some(pane) = p["pane"].as_str() {
            assert!(
                own_panes.contains(&pane),
                "a foreign pane reached {variant}: {p}"
            );
        }
    }
    if variant == "A" {
        fake.assert_counts("on A", &[("peek", 1), ("prompt", 1)]);
        assert_eq!(fake.prompt_texts(), ["to-A"]);
    } else {
        fake.assert_counts("on B", &[("peek", 1), ("prompt", 2)]);
        assert_eq!(fake.prompt_texts(), ["to-B", "while-A-stopped"]);
    }
    fake.set_link(false);
}

// ----------------------------------------------------------------------- demo

#[derive(Clone)]
struct DemoPane {
    pane: &'static str,
    ws: &'static str,
    title: &'static str,
    kind: &'static str,
    status: &'static str,
    since_ms: u64,
    queued: u32,
    /// A waiting pane's menu is on screen until it is answered.
    menu: bool,
    lines: Vec<String>,
}

/// A page with state: answering a menu makes the pane work, a queued prompt
/// raises `queued`.
struct DemoPage {
    panes: Mutex<Vec<DemoPane>>,
    menu_seq: Mutex<u32>,
}

const DEMO_MENU: [&str; 3] = [
    "Yes",
    "Yes, and don't ask again for cargo test commands in this project",
    "No, and tell Claude what to do differently",
];

fn demo_transcript(task: &str) -> Vec<String> {
    let mut lines = vec![
        format!("> {task}"),
        String::new(),
        "● I'll start by reading the code that handles this.".into(),
        "● Read(src/workspace/WorkspaceManager.ts)".into(),
        "  ⎿  Read 412 lines".into(),
        "● Search(pattern: \"remote_publish_panes\", path: \"src\")".into(),
        "  ⎿  Found 6 files".into(),
        "● The panes are published from one place, so the fix is small.".into(),
        "● Update(src/remote/remoteModel.ts)".into(),
        "  ⎿  Updated src/remote/remoteModel.ts with 14 additions and 3 removals".into(),
        "      41   export function menuId(lines: string[]): string {".into(),
        "      42  -  return hash(lines.join(\"\\n\"));".into(),
        "      42  +  const text = lines.filter(Boolean).join(\"\\n\");".into(),
        "      43  +  return `${hash(text)}-${epoch}`;".into(),
        "      44   }".into(),
        "● Bash(pnpm vitest run src/remote)".into(),
        "  ⎿  ✓ src/remote/remoteModel.test.ts (23 tests) 41ms".into(),
        "     ✓ src/remote/remoteStatus.test.ts (9 tests) 18ms".into(),
        "     Test Files  2 passed (2)".into(),
        "          Tests  32 passed (32)".into(),
        "● Both files pass. Now the Rust side.".into(),
        "● Read(src-tauri/src/remote/engine.rs)".into(),
        "  ⎿  Read 2422 lines".into(),
        "● Bash(cargo test --no-default-features --lib -p ymux remote::)".into(),
        "  ⎿  test result: ok. 162 passed; 0 failed; 0 ignored".into(),
        "● Update(src-tauri/src/remote/audit.rs)".into(),
        "  ⎿  Updated src-tauri/src/remote/audit.rs with 21 additions".into(),
        "● Running the formatter and the linter before I stop.".into(),
        "● Bash(cargo fmt --all -- --check)".into(),
        "  ⎿  (no output)".into(),
        "● Bash(cargo clippy --workspace -- -D warnings)".into(),
        "  ⎿  Finished `dev` profile in 41.2s".into(),
        "● Everything is green. Summary of what changed:".into(),
        "  - menu ids now change when the question text changes".into(),
        "  - replayed frames are counted, not logged one by one".into(),
        String::new(),
    ];
    lines.truncate(40);
    lines
}

fn menu_lines(task: &str) -> Vec<String> {
    let mut lines = demo_transcript(task);
    lines.truncate(34);
    lines.extend([
        "● Bash(cargo test -p ymux --lib remote::engine)".into(),
        String::new(),
        "  Do you want to proceed?".into(),
        format!("  ❯ 1. {}", DEMO_MENU[0]),
        format!("    2. {}", DEMO_MENU[1]),
        format!("    3. {}", DEMO_MENU[2]),
    ]);
    lines
}

impl DemoPage {
    fn new(office: bool) -> Arc<Self> {
        let now = now_ms();
        let panes = if office {
            vec![
                DemoPane {
                    pane: "d1000000-0000-4000-8000-000000000001",
                    ws: "yMux",
                    title: "Fix the audit flood",
                    kind: "claude",
                    status: "waiting",
                    since_ms: now - 95_000,
                    queued: 0,
                    menu: true,
                    lines: menu_lines("Make replayed frames cheap for the audit log"),
                },
                DemoPane {
                    pane: "d1000000-0000-4000-8000-000000000002",
                    ws: "yMux",
                    title: "Usage view polish",
                    kind: "codex",
                    status: "working",
                    since_ms: now - 640_000,
                    queued: 0,
                    menu: false,
                    lines: demo_transcript("Tighten the usage heatmap spacing"),
                },
                DemoPane {
                    pane: "d1000000-0000-4000-8000-000000000003",
                    ws: "blog",
                    title: "Draft release notes",
                    kind: "claude",
                    status: "done",
                    since_ms: now - 1_800_000,
                    queued: 0,
                    menu: false,
                    lines: demo_transcript("Write the 0.17 release notes"),
                },
                DemoPane {
                    pane: "d1000000-0000-4000-8000-000000000004",
                    ws: "blog",
                    title: "Shell",
                    kind: "",
                    status: "idle",
                    since_ms: now - 7_200_000,
                    queued: 0,
                    menu: false,
                    lines: vec!["~/blog $ ".into()],
                },
            ]
        } else {
            vec![
                DemoPane {
                    pane: "d2000000-0000-4000-8000-000000000001",
                    ws: "home-lab",
                    title: "Migrate the NAS backup job",
                    kind: "claude",
                    status: "waiting",
                    since_ms: now - 40_000,
                    queued: 0,
                    menu: true,
                    lines: menu_lines("Move the nightly backup to restic"),
                },
                DemoPane {
                    pane: "d2000000-0000-4000-8000-000000000002",
                    ws: "home-lab",
                    title: "Ansible cleanup",
                    kind: "claude",
                    status: "working",
                    since_ms: now - 300_000,
                    queued: 1,
                    menu: false,
                    lines: demo_transcript("Remove the unused roles"),
                },
                DemoPane {
                    pane: "d2000000-0000-4000-8000-000000000003",
                    ws: "games",
                    title: "Save-file converter",
                    kind: "codex",
                    status: "done",
                    since_ms: now - 3_600_000,
                    queued: 0,
                    menu: false,
                    lines: demo_transcript("Convert the old save format"),
                },
                DemoPane {
                    pane: "d2000000-0000-4000-8000-000000000004",
                    ws: "games",
                    title: "Shell",
                    kind: "",
                    status: "idle",
                    since_ms: now - 86_400_000,
                    queued: 0,
                    menu: false,
                    lines: vec!["~/games $ ".into()],
                },
            ]
        };
        Arc::new(Self {
            panes: Mutex::new(panes),
            menu_seq: Mutex::new(1),
        })
    }

    fn menu_id(&self, pane: &str) -> String {
        format!("demo-{}-{}", &pane[pane.len() - 2..], self.menu_seq.lock())
    }
}

impl PageLogic for DemoPage {
    fn panes(&self) -> Value {
        Value::Array(
            self.panes
                .lock()
                .iter()
                .map(|p| {
                    json!({"pane": p.pane, "ws": p.ws, "title": p.title, "kind": p.kind,
                           "status": p.status, "since_ms": p.since_ms, "queued": p.queued})
                })
                .collect(),
        )
    }

    fn handle(&self, payload: &Value) -> Value {
        let op = payload["op"].as_str().unwrap_or("");
        let pane = payload["pane"].as_str().unwrap_or("");
        if op == "list" {
            return json!({"panes": self.panes()});
        }
        let mut panes = self.panes.lock();
        let Some(p) = panes.iter_mut().find(|p| p.pane == pane) else {
            return json!({"result": "gone"});
        };
        match op {
            "peek" => {
                let menu = if p.menu {
                    DEMO_MENU
                        .iter()
                        .enumerate()
                        .map(|(i, l)| {
                            json!({"n": i + 1, "label": l, "selected": i == 0, "input": false})
                        })
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                let want = payload["lines"].as_u64().unwrap_or(80) as usize;
                let from = p.lines.len().saturating_sub(want);
                let lines = p.lines[from..].to_vec();
                let menu_id = if p.menu {
                    self.menu_id(pane)
                } else {
                    String::new()
                };
                let question: Vec<&str> = if p.menu {
                    vec!["Do you want to proceed?"]
                } else {
                    Vec::new()
                };
                json!({
                    "lines": lines,
                    "menu": menu,
                    "menu_id": menu_id,
                    "question": question,
                    "blocked": null,
                    "status": p.status,
                })
            }
            "answer" => {
                let n = payload["n"].as_u64().unwrap_or(0) as usize;
                let label = payload["label"].as_str().unwrap_or("");
                let on_screen = DEMO_MENU.get(n.wrapping_sub(1)).copied().unwrap_or("");
                let fresh = p.menu
                    && payload["menu_id"] == self.menu_id(pane).as_str()
                    && label_matches(on_screen, label);
                if !fresh {
                    return json!({"result": "stale"});
                }
                p.menu = false;
                p.status = "working";
                p.since_ms = now_ms();
                p.lines.truncate(30);
                p.lines.push(format!("  ⎿  (answered: {on_screen})"));
                p.lines.push("● Running it now.".into());
                *self.menu_seq.lock() += 1;
                json!({"result": "sent"})
            }
            "interrupt" => {
                if p.status == "working" {
                    p.status = "idle";
                    p.since_ms = now_ms();
                    p.lines.push("  ⎿  Interrupted by user".into());
                }
                json!({"result": "sent"})
            }
            "prompt" => {
                let text = payload["text"].as_str().unwrap_or("").to_string();
                let queue = payload["queue"].as_bool().unwrap_or(false);
                if p.kind.is_empty() {
                    return json!({"result": "gone"});
                }
                if p.status == "working" || p.status == "waiting" {
                    if queue {
                        p.queued += 1;
                        return json!({"result": "queued"});
                    }
                    return json!({"result": "waiting"});
                }
                p.status = "working";
                p.since_ms = now_ms();
                p.lines.push(format!("> {text}"));
                json!({"result": "sent"})
            }
            _ => json!({}),
        }
    }
}

/// Two of these run side by side (`scripts/demo.ps1`), one per computer:
/// `YMUX_DEMO_URL`, `YMUX_DEMO_TOKEN`, `YMUX_DEMO_DIR` (a directory of its own),
/// `YMUX_DEMO_NAME` (`Office PC` or `Home PC`). The link goes to
/// `pair-url.txt` in the directory, renewed every 100 s and after each request.
///
/// A pairing request is written to `pair-pending.txt` (device id and name) in
/// the directory while it waits. The fake user then either
/// - `YMUX_DEMO_AUTOAPPROVE=1`: skips the typing and approves with the number
///   the engine minted itself (a `#[cfg(test)]` accessor; a demo-only
///   shortcut, nothing in the app can do this), or
/// - types the number a person puts into the file `YMUX_DEMO_TYPE_FILE`
///   (`type-check-<desktop>.txt` in the demo's temp dir), waiting 110 s.
#[test]
#[ignore = "a demo desktop for looking at the PWA; use scripts/demo.ps1"]
fn demo_fake_desktop() {
    let name = env("YMUX_DEMO_NAME");
    let page = DemoPage::new(name.starts_with("Office"));
    let dir = PathBuf::from(env("YMUX_DEMO_DIR"));
    let typing = if std::env::var("YMUX_DEMO_AUTOAPPROVE").is_ok_and(|v| v == "1") {
        Typing::Auto
    } else {
        Typing::File(PathBuf::from(env("YMUX_DEMO_TYPE_FILE")))
    };
    let fake = Fake::start(Start {
        base: &env("YMUX_DEMO_URL"),
        token: &env("YMUX_DEMO_TOKEN"),
        dir: &dir,
        name: &name,
        page: page.clone(),
        reopen_after_pair: true,
        typing,
    });
    fake.wait_connected();
    let mut published = String::new();
    let mut next_window = Instant::now();
    loop {
        // After a reconnect the engine starts empty: keep feeding it the list.
        let now = page.panes().to_string();
        if now != published || !fake.connected() {
            published = now;
            fake.publish();
        }
        if fake.connected()
            && Instant::now() >= next_window
            && !dir.join("pair-pending.txt").exists()
        {
            fake.open_pairing();
            next_window = Instant::now() + Duration::from_secs(100);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

// ------------------------------------------------------------ store failures

mod store_failures {
    use super::*;

    const X: &str = "11111111-1111-4111-8111-111111111111";
    const Y: &str = "22222222-2222-4222-8222-222222222222";

    fn device(id: &str) -> RemoteDevice {
        RemoteDevice {
            id: id.into(),
            name: "phone".into(),
            pubkey: "k".into(),
            cap: "view".into(),
            added_ms: 1,
        }
    }

    fn rig(dir: &Path, devices: Vec<RemoteDevice>) -> (Arc<Shared>, Arc<E2eHost>) {
        let (host, _r, _p) = E2eHost::new(RemoteConfig {
            enabled: true,
            server_url: "http://127.0.0.1:1".into(),
            desktop_id: uuid::Uuid::new_v4().to_string(),
            devices,
            ..RemoteConfig::default()
        });
        let sh = Shared::new(
            host.clone(),
            Arc::new(SecretStore::new(dir)),
            Audit::new(dir.join("audit.jsonl")),
            true,
        );
        (sh, host)
    }

    fn link() -> String {
        crate::remote::keys::random_b64u(32)
    }

    #[test]
    fn a_device_without_a_key_is_dropped_one_whose_key_cannot_be_read_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (sh, host) = rig(dir.path(), vec![device(X), device(Y)]);
        // The identity key exists, so this is not a lost-identity reset.
        sh.secrets
            .set(SECRET_ID, "secret", &Keypair::generate().secret_b64u())
            .unwrap();
        sh.secrets
            .set(&link_secret_id(X), "secret", &link())
            .unwrap();

        // The store answers: X has a key, Y has none, so Y is forgotten.
        sh.prepare().unwrap();
        let ids: Vec<String> = host.cfg().devices.iter().map(|d| d.id.clone()).collect();
        assert_eq!(ids, [X]);
        assert!(sh.unreadable.lock().is_empty());
        assert_eq!(status_of(&sh).devices[0].error, "");

        // The store cannot be read (here: its file is not a file): X stays in
        // the config, is not given to the engine, and the status says why.
        let store = dir.path().join("secrets.json");
        let saved = std::fs::read(&store).unwrap();
        std::fs::remove_file(&store).unwrap();
        std::fs::create_dir(&store).unwrap();
        sh.prepare().unwrap();
        let ids: Vec<String> = host.cfg().devices.iter().map(|d| d.id.clone()).collect();
        assert_eq!(ids, [X], "an unreadable store must not delete devices");
        assert_eq!(*sh.unreadable.lock(), [X]);
        assert_eq!(status_of(&sh).devices[0].error, "secret_store");
        let usable = sh.with_engine(|e| e.device_count_for_test()).unwrap();
        assert_eq!(usable, 0, "unusable for this run");

        // The next rebuild after the store is back uses it again.
        std::fs::remove_dir(&store).unwrap();
        std::fs::write(&store, saved).unwrap();
        sh.prepare().unwrap();
        assert!(sh.unreadable.lock().is_empty());
        assert_eq!(status_of(&sh).devices[0].error, "");
        assert_eq!(sh.with_engine(|e| e.device_count_for_test()), Some(1));
    }

    #[test]
    fn an_unreadable_store_never_costs_the_identity_or_a_byte_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("secrets.json");
        // A file that exists but cannot be opened or parsed as the store.
        let garbage = b"\x00\x01 not a sealed store \xff\xfe".to_vec();
        std::fs::write(&store, &garbage).unwrap();
        let (sh, host) = rig(dir.path(), vec![device(X)]);

        for _ in 0..2 {
            // No new identity is minted, nothing is written over the file,
            // and the link stays down with a fixed reason.
            assert_eq!(sh.prepare(), Err("secret_store"));
            assert_eq!(std::fs::read(&store).unwrap(), garbage);
            assert!(sh.with_engine(|e| e.device_count_for_test()).is_none());
        }
        // The configured device survives for when the store comes back.
        let ids: Vec<String> = host.cfg().devices.iter().map(|d| d.id.clone()).collect();
        assert_eq!(ids, [X]);
        assert!(!dir.path().join("secrets.json.tmp").exists());

        // Only a store that answers "not present" gets a fresh identity (and,
        // as the config had a device, a clean start: see the next tests).
        std::fs::remove_file(&store).unwrap();
        sh.prepare().unwrap();
        assert!(sh.secrets.get(SECRET_ID, "secret").is_some());
    }

    #[test]
    fn a_lost_identity_starts_clean_under_a_new_desktop_id() {
        let dir = tempfile::tempdir().unwrap();
        let (sh, host) = rig(dir.path(), vec![device(X)]);
        host.cfg.lock().registered = true;
        host.cfg.lock().name = "My PC".into();
        host.cfg.lock().notify_done = true;
        let old_id = host.cfg().desktop_id;
        // A link key is stored, the identity key is gone (a wiped store).
        sh.secrets
            .set(&link_secret_id(X), "secret", &link())
            .unwrap();

        sh.prepare().unwrap();
        let cfg = host.cfg();
        assert_ne!(cfg.desktop_id, old_id, "a new id goes with the new key");
        assert!(crate::remote::proto::is_uuid(&cfg.desktop_id));
        assert!(!cfg.registered);
        assert!(cfg.devices.is_empty());
        assert_eq!(cfg.server_url, "http://127.0.0.1:1", "the address stays");
        assert_eq!(cfg.name, "My PC");
        assert!(cfg.notify_done && cfg.enabled);
        assert_eq!(
            sh.secrets
                .get_checked(&link_secret_id(X), "secret")
                .unwrap(),
            None,
            "its link key is gone too"
        );
        assert!(sh.secrets.get(SECRET_ID, "secret").is_some(), "a new key");
        assert_eq!(status_of(&sh).notice, "identity_reset");

        // A second prepare finds a key and a clean config: nothing more happens.
        let id = cfg.desktop_id.clone();
        sh.prepare().unwrap();
        assert_eq!(host.cfg().desktop_id, id);
    }

    #[test]
    fn a_registered_desktop_without_devices_also_starts_clean_but_a_fresh_one_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let (sh, host) = rig(dir.path(), Vec::new());
        let id = host.cfg().desktop_id;
        // Fresh config, no key: the normal first run keeps its id and says nothing.
        sh.prepare().unwrap();
        assert_eq!(host.cfg().desktop_id, id);
        assert_eq!(status_of(&sh).notice, "");

        // Registered, and the key vanishes: clean start.
        let dir = tempfile::tempdir().unwrap();
        let (sh, host) = rig(dir.path(), Vec::new());
        host.cfg.lock().registered = true;
        let id = host.cfg().desktop_id;
        sh.prepare().unwrap();
        assert_ne!(host.cfg().desktop_id, id);
        assert!(!host.cfg().registered);
        assert_eq!(status_of(&sh).notice, "identity_reset");
    }

    #[test]
    fn an_unreadable_store_blocks_the_reset_and_every_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("secrets.json");
        let garbage = b"\x00 unreadable".to_vec();
        std::fs::write(&store, &garbage).unwrap();
        let (sh, host) = rig(dir.path(), vec![device(X)]);
        host.cfg.lock().registered = true;
        let before = host.cfg();
        assert_eq!(sh.prepare(), Err("secret_store"));
        assert_eq!(host.cfg().desktop_id, before.desktop_id);
        assert!(host.cfg().registered);
        assert_eq!(host.cfg().devices.len(), 1);
        assert_eq!(status_of(&sh).notice, "");
        // The writes the commands make (token, link key, clearing either).
        for (id, field, v) in [
            (SECRET_ID, "api_key", "tok"),
            (SECRET_ID, "api_key", ""),
            (SECRET_ID, "secret", "k"),
            ("remote-dev:x", "secret", "k"),
            ("remote-dev:x", "secret", ""),
        ] {
            assert!(sh.secrets.set_checked(id, field, v).is_err());
        }
        assert_eq!(std::fs::read(&store).unwrap(), garbage);
        assert!(!dir.path().join("secrets.json.corrupt").exists());
    }

    #[test]
    fn paired_sorts_usable_missing_and_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let (sh, _host) = rig(dir.path(), Vec::new());
        sh.secrets
            .set(&link_secret_id(X), "secret", &link())
            .unwrap();
        sh.secrets
            .set(&link_secret_id(Y), "secret", "not a key")
            .unwrap();
        let set = sh.paired(&[device(X), device(Y), device("z")]);
        assert_eq!(set.usable.len(), 1);
        assert_eq!(set.missing, [Y, "z"]);
        assert!(set.unreadable.is_empty());
    }
}
