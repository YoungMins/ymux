//! Close-to-tray and the quit handshake, as a pure state machine.
//!
//! The main window's close button (Windows ×, macOS red button, Alt+F4)
//! hides ymux to its tray icon instead of quitting, so the PTYs and agents
//! keep running. A *real* quit — tray Quit, macOS Cmd+Q / the app menu's
//! Quit, the palette's "Quit ymux" — must still pass the frontend's
//! unsaved-editors prompt (`main.ts`'s quit listener, `confirmCloseAll`)
//! before `app.exit(0)` runs `final_flush`:
//!
//! 1. Quit asks [`QuitGate::request_quit`]. With the frontend armed that
//!    moves Idle → Asking and the caller emits `ymux://quit-requested`.
//! 2. The frontend acknowledges at once ([`QuitGate::ack`]), before any
//!    prompt, then runs its prompt and answers with [`QuitGate::answer`]:
//!    proceed → Exiting (the caller exits), cancel → Idle.
//!
//! ymux must always be quittable, so:
//!
//! - The frontend arms the handshake ([`QuitGate::arm`]) only once its quit
//!   listener is subscribed. Until then Quit exits at once — a page that
//!   never finished booting (no shells detected, `main()` threw).
//! - Every main-page load disarms it ([`QuitGate::disarm`], from
//!   `on_page_load`), and drops a pending Asking: a reload or renderer
//!   crash takes the listener with it.
//! - A request nobody acknowledged within [`ACK_TIMEOUT`] is presumed lost
//!   (a hung or dead page): the next Quit — or close — exits.
//!
//! Not desktop-gated: the decisions run under
//! `cargo test --no-default-features --lib -p ymux`. The wiring lives in
//! `tray.rs` (desktop) and `main.rs`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a quit request may go unacknowledged before the frontend is
/// presumed gone and the next Quit exits without it.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// What to do with a close request on the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Prevent the close and hide the window to the tray.
    HideToTray,
    /// Prevent the close and do nothing: the quit prompt is up, and
    /// hiding it would leave the prompt unanswerable out of sight.
    Ignore,
    /// Prevent the close and run the real quit flow instead: there is no
    /// tray icon to come back from (hiding would strand a headless process
    /// with live PTYs), or a quit request went unanswered.
    Quit,
    /// Let the window close: the app is already exiting.
    Allow,
}

/// What the caller of [`QuitGate::request_quit`] must do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitStart {
    /// Emit `ymux://quit-requested` and wait for the frontend's answer.
    AskFrontend,
    /// A prompt is already up (a second Cmd+Q / tray Quit): swallow it.
    AlreadyAsking,
    /// Exit now — no frontend to ask, it stopped answering, or already
    /// exiting.
    ExitNow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// A request is out; `acked` once the frontend received it.
    Asking {
        since: Instant,
        acked: bool,
    },
    Exiting,
}

#[derive(Debug)]
struct Inner {
    phase: Phase,
    armed: bool,
    tray: bool,
}

/// The shared quit state. One per app, managed in Tauri state.
#[derive(Debug)]
pub struct QuitGate {
    inner: Mutex<Inner>,
}

impl Default for QuitGate {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                phase: Phase::Idle,
                armed: false,
                tray: false,
            }),
        }
    }
}

impl QuitGate {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // Plain data with no invariants a panic could break mid-update.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The frontend's quit listener is subscribed.
    pub fn arm(&self) {
        self.lock().armed = true;
    }

    /// The main page is (re)loading: its listener is gone, and any request
    /// it was holding will never be answered.
    pub fn disarm(&self) {
        let mut g = self.lock();
        g.armed = false;
        if matches!(g.phase, Phase::Asking { .. }) {
            g.phase = Phase::Idle;
        }
    }

    /// The frontend received the quit request (sent before its prompt).
    pub fn ack(&self) {
        if let Phase::Asking { acked, .. } = &mut self.lock().phase {
            *acked = true;
        }
    }

    /// Record whether the tray icon was actually built.
    pub fn set_tray_present(&self, present: bool) {
        self.lock().tray = present;
    }

    /// True once the quit has been confirmed (or bypassed).
    pub fn is_exiting(&self) -> bool {
        self.lock().phase == Phase::Exiting
    }

    /// The decision for a close request on the main window.
    pub fn on_main_close(&self, now: Instant) -> CloseAction {
        let g = self.lock();
        match g.phase {
            Phase::Exiting => CloseAction::Allow,
            Phase::Asking { since, acked } => {
                if !acked && now.saturating_duration_since(since) >= ACK_TIMEOUT {
                    CloseAction::Quit
                } else {
                    CloseAction::Ignore
                }
            }
            Phase::Idle if !g.tray => CloseAction::Quit,
            Phase::Idle => CloseAction::HideToTray,
        }
    }

    /// A real quit was asked for (tray Quit, Cmd+Q, the palette, or a close
    /// with no tray). See [`QuitStart`].
    pub fn request_quit(&self, now: Instant) -> QuitStart {
        let mut g = self.lock();
        if !g.armed {
            g.phase = Phase::Exiting;
            return QuitStart::ExitNow;
        }
        match g.phase {
            Phase::Idle => {
                g.phase = Phase::Asking {
                    since: now,
                    acked: false,
                };
                QuitStart::AskFrontend
            }
            Phase::Asking { since, acked } => {
                if !acked && now.saturating_duration_since(since) >= ACK_TIMEOUT {
                    g.phase = Phase::Exiting;
                    QuitStart::ExitNow
                } else {
                    QuitStart::AlreadyAsking
                }
            }
            Phase::Exiting => QuitStart::ExitNow,
        }
    }

    /// The frontend's answer to a quit request. Returns true when the
    /// caller must exit now. An answer with no quit pending is ignored.
    pub fn answer(&self, proceed: bool) -> bool {
        let mut g = self.lock();
        if !matches!(g.phase, Phase::Asking { .. }) {
            return false;
        }
        g.phase = if proceed { Phase::Exiting } else { Phase::Idle };
        proceed
    }
}

/// Longest tray label accepted from the frontend, in chars.
const MAX_LABEL_CHARS: usize = 64;

/// A tray label from the frontend (`set_tray_labels`): control characters
/// dropped, capped, trimmed. `None` means "keep the current one".
pub fn clean_label(s: &str) -> Option<String> {
    let s: String = s
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect();
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// A menu item's text for the platform: Windows menus read `&` as the
/// mnemonic marker, so a literal one is written `&&` there. (Not for the
/// tooltip, which shows `&` as is.)
pub fn menu_text(label: &str, windows: bool) -> String {
    if windows {
        label.replace('&', "&&")
    } else {
        label.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> QuitGate {
        let g = QuitGate::default();
        g.set_tray_present(true);
        g.arm();
        g
    }

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn close_hides_to_tray_while_idle() {
        assert_eq!(ready().on_main_close(t0()), CloseAction::HideToTray);
    }

    #[test]
    fn close_without_a_tray_quits_instead_of_hiding() {
        let g = QuitGate::default();
        g.arm();
        assert_eq!(g.on_main_close(t0()), CloseAction::Quit);
    }

    #[test]
    fn quit_asks_the_frontend_then_exits_on_proceed() {
        let g = ready();
        let now = t0();
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
        g.ack();
        assert_eq!(g.on_main_close(now), CloseAction::Ignore);
        assert!(g.answer(true));
        assert!(g.is_exiting());
        assert_eq!(g.on_main_close(now), CloseAction::Allow);
    }

    #[test]
    fn cancelled_quit_returns_to_idle_and_can_be_asked_again() {
        let g = ready();
        let now = t0();
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
        g.ack();
        assert!(!g.answer(false));
        assert!(!g.is_exiting());
        assert_eq!(g.on_main_close(now), CloseAction::HideToTray);
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
    }

    #[test]
    fn second_quit_while_acked_prompt_is_up_is_swallowed() {
        let g = ready();
        let now = t0();
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
        g.ack();
        // Even long after: a user can sit on the prompt as long as they like.
        let later = now + ACK_TIMEOUT * 10;
        assert_eq!(g.request_quit(later), QuitStart::AlreadyAsking);
        assert_eq!(g.on_main_close(later), CloseAction::Ignore);
    }

    #[test]
    fn unacked_second_quit_inside_the_timeout_is_swallowed() {
        let g = ready();
        let now = t0();
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
        assert_eq!(
            g.request_quit(now + Duration::from_millis(500)),
            QuitStart::AlreadyAsking
        );
    }

    #[test]
    fn unacked_request_past_the_timeout_lets_the_next_quit_exit() {
        let g = ready();
        let now = t0();
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
        let later = now + ACK_TIMEOUT;
        assert_eq!(g.on_main_close(later), CloseAction::Quit);
        assert_eq!(g.request_quit(later), QuitStart::ExitNow);
        assert!(g.is_exiting());
    }

    #[test]
    fn unarmed_frontend_means_quit_exits_at_once() {
        let g = QuitGate::default();
        g.set_tray_present(true);
        assert_eq!(g.request_quit(t0()), QuitStart::ExitNow);
        assert!(g.is_exiting());
    }

    #[test]
    fn page_reload_disarms_so_quit_exits_at_once() {
        let g = ready();
        g.disarm(); // main page reloaded / crashed
        assert_eq!(g.request_quit(t0()), QuitStart::ExitNow);
    }

    #[test]
    fn reload_mid_prompt_drops_it_and_rearming_asks_again() {
        let g = ready();
        let now = t0();
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
        g.ack();
        g.disarm();
        assert!(!g.answer(true), "the old page's answer is void");
        assert_eq!(g.on_main_close(now), CloseAction::HideToTray);
        g.arm();
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
    }

    #[test]
    fn stray_answer_or_ack_without_a_pending_quit_is_ignored() {
        let g = ready();
        g.ack();
        assert!(!g.answer(true));
        assert!(!g.is_exiting());
        let now = t0();
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
        // The stray ack above didn't pre-acknowledge this request.
        assert_eq!(g.request_quit(now + ACK_TIMEOUT), QuitStart::ExitNow);
    }

    #[test]
    fn quit_while_exiting_exits() {
        let g = ready();
        let now = t0();
        assert_eq!(g.request_quit(now), QuitStart::AskFrontend);
        assert!(g.answer(true));
        assert_eq!(g.request_quit(now), QuitStart::ExitNow);
        g.disarm();
        g.arm();
        assert!(g.is_exiting(), "a reload never resurrects an exiting app");
    }

    #[test]
    fn labels_are_cleaned_and_capped() {
        assert_eq!(clean_label("  ymux 열기 \n").as_deref(), Some("ymux 열기"));
        assert_eq!(clean_label(" \t\r "), None);
        assert_eq!(clean_label("a\u{7}b").as_deref(), Some("ab"));
        assert_eq!(
            clean_label(&"é".repeat(100)).map(|s| s.chars().count()),
            Some(64)
        );
    }

    #[test]
    fn ampersands_are_doubled_only_for_windows_menus() {
        assert_eq!(menu_text("Save & Quit", true), "Save && Quit");
        assert_eq!(menu_text("Save & Quit", false), "Save & Quit");
        assert_eq!(menu_text("Quit", true), "Quit");
    }
}
