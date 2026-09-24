//! Resolving and opening filesystem paths that the frontend lifted out of
//! terminal output.
//!
//! The frontend's path linkifier hands raw substrings of whatever a shell
//! printed — `cat`, `git log`, an agent's answer — and asks two things:
//! "does this exist?" and, on a click, "open it with the OS default
//! program". Both questions are answered here rather than in `commands.rs`
//! so the parts worth testing (validation, tilde expansion, the UNC policy,
//! the reveal-instead-of-run list) stay reachable from
//! `cargo test --no-default-features --lib -p ymux` on Linux CI.
//!
//! ## The threat model, because it is not the obvious one
//!
//! Terminal output is attacker-influenced. Anything the user `cat`s, any
//! repository they clone, any page an agent quotes can plant a string that
//! this module will be asked about. Two consequences shape the code:
//!
//!  1. **Probing is automatic.** Existence checks fire on hover, without a
//!     click. On Windows, statting `\\evil.example\x\y` makes the SMB client
//!     open a connection and offer NTLM credentials, which is a credential
//!     leak triggered by moving a mouse; an unreachable host also hangs the
//!     call for tens of seconds. So auto-probing a remote UNC host is
//!     refused unless it is a known-local pseudo-host or the host the pane's
//!     own cwd already lives on. See [`network_probe_allowed`].
//!  2. **"Open with the default program" runs executables.** `ShellExecuteW`
//!     on `evil.bat` does not open it, it executes it — and the user's
//!     mental model for clicking a link is "show me this", not "run this".
//!     Executables and scripts are therefore revealed in the file manager
//!     instead of launched. See [`should_reveal`].
//!
//! Nothing here ever builds a shell command line. `opener` uses
//! `ShellExecuteW` on Windows and `Command::new("open")` on macOS, both of
//! which take the path as an opaque argument, so a filename containing `&`,
//! `^`, `%` or a quote is inert.

use std::path::{Path, PathBuf};

/// Only the app's own main webview may call a `guard_local` command.
///
/// This is not theoretical: Tauri injects `invoke` into every webview it
/// creates, capability or not, and never ACL-checks ymux's own commands, so
/// the page loaded in an `eb-*` embedded browser pane can call any of them.
/// Without this check any website the user visits could spawn a program,
/// type into a shell or read the filesystem.
pub fn caller_allowed(label: &str) -> bool {
    label == "main"
}

/// Does an IPC request's `Origin` header name ymux's own document?
///
/// The label half ([`caller_allowed`]) is **not sufficient** on its own, and
/// this is the evidence, read out of the pinned dependency sources rather
/// than assumed:
///
///  - A `PaneKind::Browser` pane is an `<iframe>` *inside* the `main`
///    webview (`frame-src http: https:` in `tauri.conf.json`), so anything
///    running in it sees `webview.label() == "main"`.
///  - wry hands every initialization script to WebView2's
///    `AddScriptToExecuteOnDocumentCreated` and drops the
///    `for_main_frame_only` flag on the floor (`wry-0.54.4`
///    `src/webview2/mod.rs:494`; the flag is documented as ignored on
///    Windows at `src/lib.rs:1007`). Tauri asks for main-frame-only
///    (`tauri-2.10.3` `src/manager/webview.rs:156`) and does not get it, so
///    an iframe is handed `window.__TAURI_INTERNALS__` *including the
///    invoke key* — which is the only pre-dispatch check Tauri performs
///    (`src/webview/mod.rs:1729`).
///
/// The origin, unlike the label, does distinguish them. Tauri's own JS
/// reaches the IPC with `fetch()` (`scripts/ipc-protocol.js`), and on that
/// path the `Origin` header is read from the real HTTP request
/// (`src/ipc/protocol.rs:491`, stored at `:549`) and forwarded to the
/// command through `tauri::ipc::Request::headers()`. `Origin` is a
/// forbidden header name, so a page cannot set it — the browser does, from
/// the frame's own origin.
///
/// Compared component-wise on purpose: `Url::origin()` returns an *opaque*
/// origin for a non-special scheme, and two opaque origins never compare
/// equal, so `a.origin() == b.origin()` would reject macOS's own
/// `tauri://localhost`.
///
/// `allowed` is built from the **configuration** ([`allowed_origins`]), not
/// from whatever the webview currently shows. That distinction is
/// load-bearing: comparing against `webview.url()` would mean that if the
/// `main` webview were ever navigated to a remote page, that page's origin
/// would trivially equal the app origin and the guard would pass. Tauri's
/// own `is_local_url` compares against the configured app URL for the same
/// reason. (Today `BrowserPane`'s iframe carries a `sandbox` without
/// `allow-top-navigation` and no navigation handler exists, so the
/// navigation is not reachable — the guard simply does not depend on that
/// staying true.)
///
/// Fails closed. A missing header, `null` (a sandboxed or `data:` frame), an
/// unparseable value or a host-only near-miss such as
/// `http://tauri.localhost.evil.com` all return `false`.
///
/// ## What this relies on: an iframe cannot reach the `postMessage` IPC
///
/// Tauri's fallback transport (`window.ipc.postMessage`, used when the
/// `fetch` to the IPC protocol fails) builds the request headers from a
/// JSON field **the page supplies** (`handle_ipc_message`, `tauri-2.10.3` `src/ipc/protocol.rs:185`), so on
/// that path `Origin` is forgeable. What keeps a `browser`-pane iframe off
/// it on Windows is WebView2 itself: wry subscribes only to
/// `ICoreWebView2::add_WebMessageReceived` (`wry-0.54.4`
/// `src/webview2/mod.rs:892`), which fires for the *top-level* document;
/// an iframe's `chrome.webview.postMessage` is delivered to
/// `ICoreWebView2Frame2::WebMessageReceived`, which needs a `FrameCreated`
/// subscription wry never makes. The iframe's only working transport is
/// `fetch`, where the browser sets `Origin`. An `eb-*` child *is* a
/// top-level document and can forge `Origin` — which is why the label check
/// ([`caller_allowed`]) is not optional. If a wry/Tauri upgrade ever starts
/// handling frame web messages, this reasoning must be revisited.
///
/// ## What this cannot see: ymux's own document inside a frame
///
/// If a `browser` pane framed `http://tauri.localhost/` itself, that frame's
/// requests would carry label `main` *and* the local Origin, and nothing in
/// the request tells a subframe from the top-level document (the fetch
/// headers — `Sec-Fetch-*`, `Referer` — are the same for both). That case is
/// stopped before it can issue a request: the CSP's `frame-ancestors 'none'`
/// refuses the load, and `src/bootGuard.ts` refuses to boot when
/// `window.top !== window`.
pub fn origin_is_local<S: AsRef<str>>(origin: Option<&str>, allowed: &[S]) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    allowed.iter().any(|a| same_origin(origin, a.as_ref()))
}

/// Component-wise origin equality for one candidate.
fn same_origin(origin: &str, app_url: &str) -> bool {
    if origin.trim().eq_ignore_ascii_case("null") {
        return false;
    }
    let (Ok(got), Ok(want)) = (url::Url::parse(origin), url::Url::parse(app_url)) else {
        return false;
    };
    let (Some(got_host), Some(want_host)) = (got.host_str(), want.host_str()) else {
        // An origin with no authority (`data:`, `file:`) is never ymux's
        // document, and an app URL without one is a configuration we do not
        // ship — either way, refuse.
        return false;
    };
    got.scheme().eq_ignore_ascii_case(want.scheme())
        && got_host.eq_ignore_ascii_case(want_host)
        && got.port_or_known_default() == want.port_or_known_default()
}

/// The `Origin` header of the IPC request now being served, if any.
///
/// Split out from [`guard_local`] so the header-name lookup is in one place
/// and the decision itself stays in the pure [`origin_is_local`].
#[cfg(feature = "desktop")]
fn request_origin<'a>(request: &'a tauri::ipc::Request<'_>) -> Option<&'a str> {
    request.headers().get("Origin")?.to_str().ok()
}

/// The gate on every ymux command whose only legitimate caller is ymux's
/// own document — which is all of them except the two in
/// [`crate::ipc_guard::EMBEDDED_CHILD_COMMANDS`].
///
/// Called on the first line of every one of those commands, and
/// `ipc_guard::tests::every_registered_command_starts_with_a_guard` fails if
/// one is missing (CLAUDE.md rule 16). It is not
/// defence in depth — it is the *only* defence, because `src-tauri/build.rs`
/// is a bare `tauri_build::build()` with no `AppManifest`, so
/// `RuntimeAuthority::has_app_manifest()` is false and Tauri skips the ACL
/// check entirely for ymux's own commands (`tauri-2.10.3`
/// `src/webview/mod.rs:1802`). The capability files govern `core:` and
/// plugin permissions only; adding ymux's commands to one would require an
/// `AppManifest`, which would switch ACL enforcement on for every
/// command at once. See the spec's §1.5.
///
/// `cmd` only names the caller in the error message.
#[cfg(feature = "desktop")]
pub fn guard_local(
    webview: &tauri::Webview,
    request: &tauri::ipc::Request<'_>,
    cmd: &str,
) -> crate::YmuxResult<()> {
    if !caller_allowed(webview.label()) {
        return Err(crate::YmuxError::Forbidden(format!(
            "{cmd}: only ymux's own webview may call this (label {:?})",
            webview.label()
        )));
    }
    if !origin_is_local(request_origin(request), &allowed_origins(webview)) {
        return Err(crate::YmuxError::Forbidden(format!(
            "{cmd}: only ymux's own document may call this, not embedded web content"
        )));
    }
    Ok(())
}

/// The origins ymux's own document can legitimately have, derived from the
/// running configuration rather than from the webview's current URL.
///
/// Mirrors `AppManager::tauri_protocol_url` (`tauri-2.10.3`
/// `src/manager/mod.rs:331`): Windows and Android serve the app over
/// `http(s)://tauri.localhost`, everything else over `tauri://localhost`.
/// The dev-server origin is added only in a dev build, so a release binary
/// never accepts `http://localhost:1420`.
#[cfg(feature = "desktop")]
fn allowed_origins(webview: &tauri::Webview) -> Vec<String> {
    use tauri::Manager;
    let cfg = webview.config();

    let https = cfg.app.windows.iter().any(|w| w.use_https_scheme);
    let mut out = Vec::with_capacity(2);
    if cfg!(windows) || cfg!(target_os = "android") {
        out.push(if https {
            "https://tauri.localhost".to_string()
        } else {
            "http://tauri.localhost".to_string()
        });
    } else {
        out.push("tauri://localhost".to_string());
    }

    #[cfg(dev)]
    if let Some(dev_url) = &cfg.build.dev_url {
        out.push(dev_url.to_string());
    }

    out
}

/// Longest raw candidate worth looking at. Comfortably past any real path
/// (`MAX_PATH` is 260, and even the extended limit is 32767) while keeping a
/// pathological line from turning into a long syscall.
pub const MAX_RAW_LEN: usize = 4096;

/// Most candidates probed in one request. The frontend sends at most one
/// row's worth (24), so this is a backstop against a caller that does not.
pub const MAX_BATCH: usize = 32;

/// How long a batch of existence checks may take before it is abandoned.
/// A mapped network drive that has gone away blocks `metadata` for tens of
/// seconds; a hover must not be able to wedge anything for that long.
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// A resolved, existing path, as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedPath {
    /// Absolute, with Windows' verbatim `\\?\` prefix stripped so it is both
    /// readable in the tooltip and acceptable to `ShellExecuteW`.
    pub absolute: String,
    /// Directories open in the OS file manager rather than an application.
    pub is_dir: bool,
}

/// Why a raw candidate is not worth touching at all, or `None` if it is.
///
/// Existence is the main filter, but deliberately not the only one: a
/// control character must not reach the opener even if some filesystem
/// somewhere would accept it in a name.
pub fn reject_reason(raw: &str) -> Option<&'static str> {
    if raw.is_empty() {
        return Some("empty path");
    }
    if raw.len() > MAX_RAW_LEN {
        return Some("path too long");
    }
    if raw.chars().any(|c| c.is_control()) {
        return Some("path contains a control character");
    }
    None
}

/// How a path's *syntax* engages Windows' UNC and device namespaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UncKind {
    /// An ordinary path. Nothing special about it.
    Local,
    /// `\\?\C:\…` — the verbatim spelling of a local drive path.
    VerbatimLocal,
    /// `\\.\…`, or a `\\?\…` form that is neither a drive nor a UNC share.
    /// The Win32 device namespace: `\\.\PhysicalDrive0`, `\\.\COM1`. Never
    /// probed and never opened.
    Device,
    /// A network share. The payload is the host.
    Remote(String),
}

/// Classify `raw`'s UNC syntax.
///
/// A leading `\\` is Windows syntax wherever it appears, so it is always
/// classified. A leading `//` is only treated as UNC when this build runs on
/// Windows, because that is where Win32 normalises it into one — on macOS
/// `//Users/me` is a perfectly ordinary path, and the hazard being guarded
/// against (the local SMB client authenticating on a `stat`) is a property
/// of the machine doing the syscall, not of the path's spelling.
pub fn classify_unc(raw: &str) -> UncKind {
    let rest = match raw.strip_prefix(r"\\") {
        Some(r) => r,
        None => match raw.strip_prefix("//").filter(|_| cfg!(windows)) {
            Some(r) => r,
            None => return UncKind::Local,
        },
    };
    let head = rest.split(['\\', '/']).next().unwrap_or("");
    match head {
        "." => UncKind::Device,
        "?" => {
            let tail = &rest[head.len()..];
            let tail = tail.trim_start_matches(['\\', '/']);
            // `\\?\UNC\server\share` is a UNC path in verbatim clothing.
            if let Some(unc) = tail
                .strip_prefix("UNC\\")
                .or_else(|| tail.strip_prefix("UNC/"))
            {
                return match unc.split(['\\', '/']).next().unwrap_or("") {
                    "" => UncKind::Device,
                    host => UncKind::Remote(host.to_string()),
                };
            }
            // `\\?\C:\…` — a drive letter, colon, then a separator or end.
            let mut chars = tail.chars();
            match (chars.next(), chars.next(), chars.next()) {
                (Some(d), Some(':'), sep)
                    if d.is_ascii_alphabetic() && matches!(sep, None | Some('\\') | Some('/')) =>
                {
                    UncKind::VerbatimLocal
                }
                _ => UncKind::Device,
            }
        }
        "" => UncKind::Device,
        host => UncKind::Remote(host.to_string()),
    }
}

/// Hosts whose "network" paths never leave the machine, so probing them
/// cannot leak a credential or block on an unreachable server.
const LOCAL_UNC_HOSTS: &[&str] = &[
    "localhost",
    "127.0.0.1",
    "::1",
    // WSL's two spellings of the distro filesystem.
    "wsl$",
    "wsl.localhost",
];

fn same_host(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// May `raw` be stat'd automatically, on hover, with no click?
///
/// `cwd` is the pane's own working directory: if the user is already working
/// on `\\nas\projects`, the connection is open and authenticated and there
/// is nothing left to leak, so paths on that same host are fair game.
pub fn network_probe_allowed(raw: &str, cwd: Option<&str>) -> Result<(), String> {
    match classify_unc(raw) {
        UncKind::Local | UncKind::VerbatimLocal => Ok(()),
        UncKind::Device => Err("device-namespace paths are not resolved".into()),
        UncKind::Remote(host) => {
            if LOCAL_UNC_HOSTS.iter().any(|h| same_host(h, &host)) {
                return Ok(());
            }
            let cwd_host = cwd.map(classify_unc).and_then(|k| match k {
                UncKind::Remote(h) => Some(h),
                _ => None,
            });
            match cwd_host {
                Some(h) if same_host(&h, &host) => Ok(()),
                _ => Err(format!(
                    "network path on host {host:?} is not resolved automatically"
                )),
            }
        }
    }
}

/// Turn a raw candidate into the absolute path it names, or `None` when it
/// cannot be placed — a relative candidate with no cwd, or a `~` path on a
/// system with no home directory.
///
/// A rooted-but-driveless path on Windows (`/usr/lib`, `\srv\x`) is *not*
/// absolute to Rust, so it takes the `cwd` branch — where `Path::join`
/// keeps the cwd's drive and replaces the rest, giving `D:\usr\lib`. That
/// is Windows' own drive-relative rule, so it is the right answer, but it
/// means a POSIX path pasted into a Windows pane can resolve to a real
/// local file with a different meaning. Harmless in practice: the link only
/// appears when that file exists, the tooltip shows the absolute path that
/// was resolved, and a click opens that same path.
pub fn expand(raw: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let tilde = raw
        .strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\"))
        .map(Some)
        .or_else(|| if raw == "~" { Some(None) } else { None });

    let candidate = match tilde {
        Some(rest) => {
            let home = home?;
            match rest {
                Some(r) => home.join(r),
                None => home.to_path_buf(),
            }
        }
        None => PathBuf::from(raw),
    };

    if candidate.is_absolute() {
        return Some(candidate);
    }
    Some(cwd?.join(candidate))
}

/// Drop Windows' verbatim prefix from a canonicalised path.
///
/// `std::fs::canonicalize` returns `\\?\C:\x` and `\\?\UNC\host\share\x`.
/// Neither belongs in a tooltip, and `ShellExecuteW` does not reliably
/// accept the verbatim form, so both are rewritten to their plain spelling.
/// A no-op on every other platform and on paths that do not carry it.
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        // Only the drive form is safe to unwrap; `\\?\Volume{…}` is not.
        let mut chars = rest.chars();
        if let (Some(d), Some(':')) = (chars.next(), chars.next()) {
            if d.is_ascii_alphabetic() {
                return PathBuf::from(rest);
            }
        }
    }
    path.to_path_buf()
}

/// Extensions that `ShellExecuteW` or `open` would *run* rather than show.
///
/// Opening one of these on a click would turn "the terminal printed a path"
/// into "the terminal got code executed", with the user believing they asked
/// to look at a file. They are revealed in the file manager instead.
const RUNNABLE_EXTENSIONS: &[&str] = &[
    // Windows executables and installers
    "exe",
    "com",
    "scr",
    "pif",
    "msi",
    "msp",
    "msc",
    "cpl",
    "hta",
    "jar", //
    // Windows shells and script hosts
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "ps1xml",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "reg",
    // Shortcuts, which can point at anything
    "lnk",
    "url",
    "scf", //
    // macOS
    "app",
    "command",
    "workflow",
    "scpt",
    "applescript",
    "pkg",
    "mpkg",
    "term",
    // ClickOnce application reference: opening one downloads and runs.
    "appref-ms",
];

/// Should this path be revealed in the file manager instead of opened?
///
/// Files whose extension would execute are revealed; plain directories are
/// opened, because that *is* "show it in the file manager".
///
/// The extension is checked **before** `is_dir`, and that order is
/// load-bearing: a macOS `.app` (and `.pkg`, `.workflow`, `.mpkg`) is a
/// *directory*, so an `is_dir` early return would send it to `opener::open`
/// — which is `open Foo.app`, i.e. launch the application. A directory that
/// merely happens to be named `foo.exe` gets revealed instead of opened,
/// which is harmless.
///
/// This is a denylist and so cannot be complete: a new script host with a
/// new extension, or a file type the user has associated with an
/// interpreter, is not covered. It is the reason nothing on the filesystem
/// command surface calls `opener::open` without going through here.
pub fn should_reveal(path: &Path, _is_dir: bool) -> bool {
    // `_is_dir` is no longer consulted, but stays in the signature so the
    // caller keeps paying for the `metadata` call it needs anyway and so
    // this is a drop-in for the previous behaviour.
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            RUNNABLE_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// Final gate before handing a path to the OS opener.
///
/// The path reaching here came from a probe that already vetted it, but it
/// arrives back over IPC, so it is checked again from scratch. Absoluteness
/// is load-bearing beyond tidiness: macOS's `open` takes flags, and a
/// relative candidate beginning with `-` would become one.
pub fn validate_open(raw: &str) -> Result<(), String> {
    if let Some(why) = reject_reason(raw) {
        return Err(why.to_string());
    }
    if matches!(classify_unc(raw), UncKind::Device) {
        return Err("device-namespace paths are not opened".into());
    }
    let path = Path::new(raw);
    // A UNC path is absolute on Windows but not when this check compiles for
    // a Unix target, so accept it explicitly rather than by platform.
    let unc = matches!(
        classify_unc(raw),
        UncKind::Remote(_) | UncKind::VerbatimLocal
    );
    if !path.is_absolute() && !unc {
        return Err("only absolute paths are opened".into());
    }
    Ok(())
}

/// Resolve one raw candidate against `cwd`, or `None` if it is not a path
/// worth linking. Touches the filesystem exactly once on the happy path
/// (plus a `canonicalize` for the display form).
pub fn probe_one(raw: &str, cwd: Option<&str>) -> Option<ResolvedPath> {
    if reject_reason(raw).is_some() {
        return None;
    }
    if network_probe_allowed(raw, cwd).is_err() {
        return None;
    }
    let expanded = expand(raw, cwd.map(Path::new), dirs::home_dir().as_deref())?;
    let meta = std::fs::metadata(&expanded).ok()?;
    let absolute = std::fs::canonicalize(&expanded)
        .map(|p| strip_verbatim(&p))
        .unwrap_or(expanded);
    Some(ResolvedPath {
        absolute: absolute.to_string_lossy().into_owned(),
        is_dir: meta.is_dir(),
    })
}

/// Resolve a batch, giving up on the whole batch after [`PROBE_TIMEOUT`].
///
/// The timeout runs on a detached worker rather than around each path: a
/// dead network drive blocks in `metadata` with no way to cancel it, so the
/// only thing that can be bounded is how long the *caller* waits. The output
/// always has exactly `raws.len()` entries so the frontend's index mapping
/// holds whatever happened.
pub fn probe_batch(raws: Vec<String>, cwd: Option<String>) -> Vec<Option<ResolvedPath>> {
    let want = raws.len();
    let batch: Vec<String> = raws.into_iter().take(MAX_BATCH).collect();
    let taken = batch.len();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let probed: Vec<Option<ResolvedPath>> = batch
            .iter()
            .map(|raw| probe_one(raw, cwd.as_deref()))
            .collect();
        // The receiver is gone on timeout; dropping the result is the point.
        let _ = tx.send(probed);
    });
    let mut out = rx
        .recv_timeout(PROBE_TIMEOUT)
        .unwrap_or_else(|_| vec![None; taken]);
    out.resize(want, None);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_main_webview_may_ask() {
        assert!(caller_allowed("main"));
        // Tauri injects `invoke` into embedded browser panes, so
        // a page loaded in one can reach `invoke`.
        assert!(!caller_allowed("eb-1234"));
        assert!(!caller_allowed("eb-main"));
        assert!(!caller_allowed(""));
        assert!(!caller_allowed("Main"));
    }

    /// The app origins ymux actually runs under: `tauri://localhost` on
    /// macOS, `http://tauri.localhost` in a Windows release build, and the
    /// Vite dev server under `pnpm tauri dev`.
    const APP_URLS: &[&str] = &[
        "tauri://localhost/",
        "http://tauri.localhost/",
        "http://localhost:1420/",
    ];

    #[test]
    fn origin_accepts_ymuxs_own_document() {
        assert!(origin_is_local(Some("tauri://localhost"), &[APP_URLS[0]]));
        assert!(origin_is_local(
            Some("http://tauri.localhost"),
            &[APP_URLS[1]]
        ));
        assert!(origin_is_local(
            Some("http://localhost:1420"),
            &[APP_URLS[2]]
        ));
        // A scheme is case-insensitive per RFC 3986, and so is a host.
        assert!(origin_is_local(Some("TAURI://LocalHost"), &[APP_URLS[0]]));
        // A dev build allows the app origin *and* the dev server; any one
        // match is enough.
        assert!(origin_is_local(
            Some("http://localhost:1420"),
            &[APP_URLS[1], APP_URLS[2]]
        ));
    }

    /// The list is built from configuration, never from the page currently
    /// loaded. If it were the current URL, a `main` webview that had been
    /// navigated to a remote page would hand that page an origin equal to
    /// the app's and the guard would pass.
    #[test]
    fn a_remote_page_is_refused_even_if_it_is_what_main_is_showing() {
        // "main is showing https://evil.example" is simply not expressible:
        // the allow-list never contains a remote origin.
        let allowed = [APP_URLS[1]];
        assert!(!origin_is_local(Some("https://evil.example"), &allowed));
        // An empty allow-list refuses everything rather than allowing it.
        let none: [&str; 0] = [];
        assert!(!origin_is_local(Some("http://tauri.localhost"), &none));
    }

    /// The case the label check cannot see: a page in a `browser` pane is an
    /// iframe inside the `main` webview, so only its origin gives it away.
    #[test]
    fn origin_rejects_remote_web_content() {
        for app in APP_URLS {
            assert!(!origin_is_local(Some("https://evil.example"), &[*app]));
            assert!(!origin_is_local(Some("http://evil.example"), &[*app]));
        }
        // Not even when every app origin is on the list at once.
        assert!(!origin_is_local(Some("https://evil.example"), APP_URLS));
    }

    /// Fail-closed cases. A missing header is the important one: Tauri's
    /// `postMessage` fallback carries no real headers, and accepting it
    /// would also accept a forged `Origin` on that path.
    #[test]
    fn origin_fails_closed() {
        let app = [APP_URLS[1]];
        assert!(!origin_is_local(None, &app));
        // A sandboxed iframe or a `data:`/`blob:` document.
        assert!(!origin_is_local(Some("null"), &app));
        assert!(!origin_is_local(Some(" NULL "), &app));
        assert!(!origin_is_local(Some(""), &app));
        assert!(!origin_is_local(Some("not a url"), &app));
        // No authority at all.
        assert!(!origin_is_local(Some("data:text/html,x"), &app));
        // An unparseable entry on the allow-list must not degrade into
        // "allow".
        assert!(!origin_is_local(
            Some("http://tauri.localhost"),
            &["nonsense"]
        ));
    }

    /// The near-misses a naive `starts_with` or `contains` would wave
    /// through.
    #[test]
    fn origin_rejects_other_local_looking_origins() {
        let app = ["http://tauri.localhost/"];
        // Tauri's own IPC endpoint and asset protocol are not ymux's
        // document; a page that can name them has not proved anything.
        assert!(!origin_is_local(Some("http://ipc.localhost"), &app));
        assert!(!origin_is_local(Some("http://asset.localhost"), &app));
        assert!(!origin_is_local(Some("http://localhost"), &app));
        assert!(!origin_is_local(Some("http://127.0.0.1"), &app));
        // Userinfo trickery: the host here is `evil.example`.
        assert!(!origin_is_local(
            Some("http://tauri.localhost@evil.example"),
            &app
        ));
        // A non-default port on the right host is a different origin.
        assert!(!origin_is_local(Some("http://tauri.localhost:8080"), &app));
        // macOS: another custom scheme on the same host is not the app.
        assert!(!origin_is_local(
            Some("ipc://localhost"),
            &["tauri://localhost/"]
        ));
        assert!(!origin_is_local(
            Some("https://localhost"),
            &["tauri://localhost/"]
        ));
    }

    /// Every label shape an embedded browser can have is refused, whatever
    /// pane id it carries.
    #[test]
    fn embedded_browser_labels_never_pass_the_label_check() {
        for label in [
            "eb-0f8fad5b-d9cb-469f-a165-70867728950e",
            "browser-0f8fad5b-d9cb-469f-a165-70867728950e",
            "main ",
            " main",
            "main\0",
        ] {
            assert!(!caller_allowed(label), "{label:?}");
        }
    }

    #[test]
    fn origin_rejects_host_and_port_near_misses() {
        let app = ["http://tauri.localhost/"];
        assert!(!origin_is_local(
            Some("http://tauri.localhost.evil.example"),
            &app
        ));
        assert!(!origin_is_local(Some("http://eviltauri.localhost"), &app));
        // Scheme must match: an https page is not the app.
        assert!(!origin_is_local(Some("https://tauri.localhost"), &app));

        // Port must match, and the default must not be confused with a
        // different explicit one.
        let dev = ["http://localhost:1420/"];
        assert!(!origin_is_local(Some("http://localhost:1421"), &dev));
        assert!(!origin_is_local(Some("http://localhost"), &dev));
        // ...but an explicit default port is the same origin.
        assert!(origin_is_local(
            Some("http://localhost:80"),
            &["http://localhost/"]
        ));
    }

    #[test]
    fn reject_reason_flags_control_characters() {
        assert!(reject_reason("src/main.ts").is_none());
        assert!(reject_reason("").is_some());
        assert!(reject_reason("a/\u{7}b").is_some());
        assert!(reject_reason("a/\nb").is_some());
        assert!(reject_reason("a/\u{1b}[0m").is_some());
        assert!(reject_reason(&"a/".repeat(MAX_RAW_LEN)).is_some());
    }

    #[test]
    fn reject_reason_allows_awkward_but_legal_names() {
        // All legal in a Windows or POSIX filename; none may be special to us
        // because nothing is ever handed to a shell.
        assert!(reject_reason(r"C:\a & b\c^d%e.txt").is_none());
        assert!(reject_reason("/srv/한글/문서.txt").is_none());
        assert!(reject_reason(r"/srv/it's a file").is_none());
    }

    #[test]
    fn classify_unc_reads_the_host() {
        assert_eq!(classify_unc(r"C:\Users\me"), UncKind::Local);
        assert_eq!(classify_unc("/usr/lib"), UncKind::Local);
        assert_eq!(classify_unc("src/main.ts"), UncKind::Local);
        assert_eq!(
            classify_unc(r"\\server\share\x"),
            UncKind::Remote("server".into())
        );
        assert_eq!(
            classify_unc(r"\\wsl$\Ubuntu\home\me"),
            UncKind::Remote("wsl$".into())
        );
    }

    #[test]
    fn classify_unc_rejects_the_device_namespace() {
        assert_eq!(classify_unc(r"\\.\PhysicalDrive0"), UncKind::Device);
        assert_eq!(classify_unc(r"\\.\COM1"), UncKind::Device);
        assert_eq!(classify_unc(r"\\"), UncKind::Device);
        assert_eq!(classify_unc(r"\\?\Volume{1234}\x"), UncKind::Device);
        assert_eq!(classify_unc(r"\\?\GLOBALROOT\Device\x"), UncKind::Device);
    }

    #[test]
    fn classify_unc_unwraps_the_verbatim_forms() {
        assert_eq!(classify_unc(r"\\?\C:\Users\me"), UncKind::VerbatimLocal);
        assert_eq!(classify_unc(r"\\?\c:"), UncKind::VerbatimLocal);
        assert_eq!(
            classify_unc(r"\\?\UNC\server\share\x"),
            UncKind::Remote("server".into())
        );
    }

    #[test]
    fn forward_slash_unc_is_windows_syntax_only() {
        // Win32 normalises `//host/share` into a UNC path; POSIX does not,
        // and `//Users/me` on macOS must stay an ordinary path.
        let kind = classify_unc("//server/share/x");
        if cfg!(windows) {
            assert_eq!(kind, UncKind::Remote("server".into()));
        } else {
            assert_eq!(kind, UncKind::Local);
        }
    }

    #[test]
    fn remote_hosts_are_not_probed_on_hover() {
        // Statting this on Windows opens SMB and offers NTLM credentials —
        // a credential leak triggered by a mouse move over terminal output.
        assert!(network_probe_allowed(r"\\evil.example\x\y", None).is_err());
        assert!(network_probe_allowed(r"\\10.0.0.9\share\x", Some(r"C:\repo")).is_err());
    }

    #[test]
    fn local_pseudo_hosts_are_probed() {
        assert!(network_probe_allowed(r"\\wsl$\Ubuntu\home\me", None).is_ok());
        assert!(network_probe_allowed(r"\\WSL.localhost\Ubuntu\etc", None).is_ok());
        assert!(network_probe_allowed(r"\\localhost\c$\Windows", None).is_ok());
    }

    #[test]
    fn the_panes_own_share_is_probed() {
        // Already connected and authenticated: nothing left to leak.
        assert!(network_probe_allowed(r"\\nas\projects\a.txt", Some(r"\\nas\projects")).is_ok());
        assert!(network_probe_allowed(r"\\NAS\projects\a.txt", Some(r"\\nas\other")).is_ok());
        assert!(network_probe_allowed(r"\\other\projects\a.txt", Some(r"\\nas\projects")).is_err());
    }

    #[test]
    fn ordinary_paths_are_always_probed() {
        assert!(network_probe_allowed(r"C:\repo\src\main.rs", None).is_ok());
        assert!(network_probe_allowed("/usr/lib/x", None).is_ok());
        assert!(network_probe_allowed("src/main.ts", Some("/repo")).is_ok());
    }

    #[test]
    fn expand_joins_a_relative_path_onto_the_cwd() {
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\repo" } else { "/repo" });
        let got = expand("src/main.ts", Some(&cwd), None).unwrap();
        assert_eq!(got, cwd.join("src/main.ts"));
    }

    #[test]
    fn expand_refuses_a_relative_path_with_no_cwd() {
        assert!(expand("src/main.ts", None, None).is_none());
    }

    #[test]
    fn expand_leaves_an_absolute_path_alone() {
        let abs = if cfg!(windows) { r"C:\x\y" } else { "/x/y" };
        assert_eq!(expand(abs, None, None).unwrap(), PathBuf::from(abs));
    }

    #[test]
    #[cfg(windows)]
    fn expand_treats_a_driveless_root_as_drive_relative() {
        // Windows' own rule, and `Path::join` implements it: the cwd's drive
        // survives, everything after it is replaced.
        let cwd = PathBuf::from(r"D:\Git\ymux");
        assert_eq!(
            expand("/usr/lib", Some(&cwd), None).unwrap(),
            PathBuf::from("D:/usr/lib")
        );
        assert_eq!(
            expand(r"\srv\x", Some(&cwd), None).unwrap(),
            PathBuf::from(r"D:\srv\x")
        );
    }

    #[test]
    fn expand_substitutes_the_home_directory() {
        let home = PathBuf::from(if cfg!(windows) {
            r"C:\Users\me"
        } else {
            "/home/me"
        });
        assert_eq!(
            expand("~/.claude/settings.json", None, Some(&home)).unwrap(),
            home.join(".claude/settings.json")
        );
        assert_eq!(expand("~", None, Some(&home)).unwrap(), home);
        assert!(expand("~/x", None, None).is_none());
    }

    #[test]
    fn expand_does_not_treat_a_tilde_name_as_home() {
        // `~foo` is shell syntax for another user's home, which this does not
        // implement; it must stay a literal relative name.
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\repo" } else { "/repo" });
        let home = PathBuf::from("/home/me");
        assert_eq!(
            expand("~other/x", Some(&cwd), Some(&home)).unwrap(),
            cwd.join("~other/x")
        );
    }

    #[test]
    fn strip_verbatim_rewrites_both_canonical_forms() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\nas\projects\a.txt")),
            PathBuf::from(r"\\nas\projects\a.txt")
        );
    }

    #[test]
    fn strip_verbatim_leaves_everything_else_alone() {
        assert_eq!(
            strip_verbatim(Path::new("/usr/lib/x")),
            PathBuf::from("/usr/lib/x")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        // Not the drive form, so unwrapping it would change its meaning.
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\Volume{abc}\x")),
            PathBuf::from(r"\\?\Volume{abc}\x")
        );
    }

    #[test]
    fn executables_and_scripts_are_revealed_not_run() {
        for name in [
            "evil.bat",
            "evil.BAT",
            "setup.exe",
            "a.cmd",
            "a.ps1",
            "a.vbs",
            "a.js",
            "a.lnk",
            "a.url",
            "a.reg",
            "a.app",
            "a.command",
            "a.msi",
            "a.appref-ms",
        ] {
            assert!(should_reveal(Path::new(name), false), "{name}");
        }
    }

    /// A macOS `.app` (and `.pkg`, `.workflow`, `.mpkg`) is a **directory**.
    /// An `is_dir` early return therefore sent it to `opener::open`, which
    /// is `open Foo.app` — launching the application. The extension has to
    /// be consulted before `is_dir`, so this pins the order.
    #[test]
    fn a_bundle_is_a_directory_and_must_still_be_revealed() {
        for name in ["Foo.app", "Installer.pkg", "x.mpkg", "y.workflow"] {
            assert!(
                should_reveal(Path::new(name), true),
                "{name} is a directory that would otherwise be launched"
            );
        }
        // An ordinary directory still opens in the file manager.
        assert!(!should_reveal(Path::new("/srv/project"), true));
        assert!(!should_reveal(Path::new("notes.txt"), false));
    }

    #[test]
    fn documents_and_directories_are_opened() {
        for name in [
            "notes.md",
            "a.rs",
            "a.ts",
            "photo.png",
            "report.pdf",
            "Makefile",
        ] {
            assert!(!should_reveal(Path::new(name), false), "{name}");
        }
        // A plain directory *is* the file-manager case, so it opens.
        assert!(!should_reveal(Path::new("scripts"), true));
        // `bundle.app` used to be asserted here as "opens", which was the
        // bug: on macOS a `.app` is a directory and `open Foo.app` launches
        // it. See `a_bundle_is_a_directory_and_must_still_be_revealed`.
    }

    #[test]
    fn validate_open_requires_an_absolute_path() {
        // Relative names beginning with `-` would become flags to macOS `open`.
        assert!(validate_open("-R").is_err());
        assert!(validate_open("src/main.ts").is_err());
        let abs = if cfg!(windows) { r"C:\x\y" } else { "/x/y" };
        assert!(validate_open(abs).is_ok());
    }

    #[test]
    fn validate_open_accepts_a_unc_share_on_any_target() {
        // An explicit click is the consent the hover probe lacks, so a share
        // the user is genuinely working on stays openable.
        assert!(validate_open(r"\\nas\projects\a.txt").is_ok());
        assert!(validate_open(r"\\?\C:\x").is_ok());
    }

    #[test]
    fn validate_open_refuses_the_device_namespace_and_junk() {
        assert!(validate_open(r"\\.\PhysicalDrive0").is_err());
        assert!(validate_open("").is_err());
        assert!(validate_open("/x/\u{7}y").is_err());
    }

    #[test]
    fn probe_one_finds_a_real_file_and_directory() {
        let dir = std::env::temp_dir().join(format!("ymux-fspath-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("a.txt"), b"hi").unwrap();
        let cwd = dir.to_string_lossy().into_owned();

        let file = probe_one("sub/a.txt", Some(&cwd)).expect("relative file resolves");
        assert!(!file.is_dir);
        assert!(file.absolute.ends_with("a.txt"));
        // Absolute, so it can be opened without the cwd.
        assert!(Path::new(&file.absolute).is_absolute());

        let sub = probe_one("sub", Some(&cwd)).expect("relative dir resolves");
        assert!(sub.is_dir);

        assert!(probe_one("sub/missing.txt", Some(&cwd)).is_none());
        // No cwd, so a relative candidate cannot be placed at all.
        assert!(probe_one("sub/a.txt", None).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn probe_batch_answers_every_input_in_order() {
        let got = probe_batch(
            vec!["and/or".into(), "".into(), "n/a".into()],
            Some("/definitely/not/here".into()),
        );
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|r| r.is_none()));
    }

    #[test]
    fn probe_batch_pads_past_the_cap() {
        let raws: Vec<String> = (0..MAX_BATCH + 5).map(|i| format!("d{i}/f")).collect();
        let want = raws.len();
        assert_eq!(probe_batch(raws, None).len(), want);
    }
}
