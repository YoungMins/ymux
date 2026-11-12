//! Keeping the index current without any window open.
//!
//! `collect` otherwise runs only when something asks for usage (the Usage
//! view, the office hero, the quota poll, the digest). Claude Code deletes
//! transcripts after `cleanupPeriodDays` (14 by default), so a transcript no
//! refresh has seen by then is lost for good. This worker refreshes once
//! shortly after startup and then every half hour, and keeps going every few
//! seconds while a first-time index is still being built in budgeted steps. It
//! runs the very same `collect` (same mutex, same budget) and reads nothing
//! when nothing changed.
//!
//! Two yMux processes may both run it (a debug build next to a release one,
//! the startup race of the single-instance plugin): that is safe because the
//! retired store is guarded by a lock and a content check (see `index.rs`).
use super::collect::{collect, lock_recovering};
use crate::config::model::AccountProfile;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, Once};
use std::time::{Duration, Instant};

/// Env var that lets a debug build (`tauri dev`) run the worker; otherwise it
/// would fight a release build over the same index directory.
pub const DEV_ENV: &str = "YMUX_DEV_TOKEN_INDEX";

/// Delay before the first refresh: never part of startup.
pub const FIRST_DELAY: Duration = Duration::from_secs(25);
/// Between refreshes once everything is indexed.
pub const IDLE_EVERY: Duration = Duration::from_secs(30 * 60);
/// Between refreshes while a first-time index is still being built.
pub const INDEXING_EVERY: Duration = Duration::from_secs(5);
/// How long exit waits for a refresh that is under way.
const EXIT_GRACE: Duration = Duration::from_secs(3);

/// May this build run the worker? Release builds always; a debug build only
/// when `YMUX_DEV_TOKEN_INDEX=1` asks for it.
pub fn allowed(debug_build: bool, env_value: Option<&str>) -> bool {
    !debug_build || env_value == Some("1")
}

/// How long to wait before the next refresh. `first`: none has run yet.
/// `indexing`: the latest one left readable files unfinished.
pub fn next_delay(first: bool, indexing: bool) -> Duration {
    if first {
        FIRST_DELAY
    } else if indexing {
        INDEXING_EVERY
    } else {
        IDLE_EVERY
    }
}

/// A flag the worker sleeps on, so exit does not wait out a 30 minute sleep.
#[derive(Default)]
pub(super) struct Stopper {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl Stopper {
    pub fn stop(&self) {
        *lock_recovering(&self.stopped) = true;
        self.wake.notify_all();
    }

    /// Sleep up to `delay`; `true` when stopped (before or during the wait).
    pub fn wait(&self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        let mut stopped = lock_recovering(&self.stopped);
        while !*stopped {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            stopped = self
                .wake
                .wait_timeout(stopped, left)
                .map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
        }
        *stopped
    }
}

/// Run `step` (a refresh; returns whether indexing is still going) at the
/// delays `delay` gives until `stopper` stops.
pub(super) fn run(
    stopper: &Stopper,
    delay: impl Fn(bool, bool) -> Duration,
    mut step: impl FnMut() -> bool,
) {
    let mut first = true;
    let mut indexing = false;
    while !stopper.wait(delay(first, indexing)) {
        first = false;
        indexing = step();
    }
}

static STOPPER: Stopper = Stopper {
    stopped: Mutex::new(false),
    wake: Condvar::new(),
};
static STARTED: Once = Once::new();
static BUSY: AtomicBool = AtomicBool::new(false);

/// Start the worker (once). `accounts` is read afresh for every refresh, so a
/// change of accounts is picked up.
pub fn start(accounts: impl Fn() -> Vec<AccountProfile> + Send + 'static) {
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("ymux-token-index".into())
            .spawn(move || {
                run(&STOPPER, next_delay, || {
                    BUSY.store(true, Ordering::SeqCst);
                    let indexing = collect(&accounts()).indexing;
                    BUSY.store(false, Ordering::SeqCst);
                    indexing
                });
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "cannot start the background token index refresh");
        }
    });
}

/// Stop the worker and give a refresh that is under way a moment to finish
/// (its writes are atomic, so a process that exits anyway loses nothing).
pub fn shutdown() {
    STOPPER.stop();
    let until = Instant::now() + EXIT_GRACE;
    while BUSY.load(Ordering::SeqCst) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    #[test]
    fn the_first_refresh_waits_then_it_is_fast_while_indexing_and_slow_when_idle() {
        assert_eq!(next_delay(true, false), FIRST_DELAY);
        assert_eq!(next_delay(true, true), FIRST_DELAY);
        assert_eq!(next_delay(false, true), INDEXING_EVERY);
        assert_eq!(next_delay(false, false), IDLE_EVERY);
        assert!(FIRST_DELAY >= Duration::from_secs(20) && FIRST_DELAY <= Duration::from_secs(30));
        assert!(INDEXING_EVERY < IDLE_EVERY && IDLE_EVERY == Duration::from_secs(1800));
    }

    #[test]
    fn debug_builds_need_the_opt_in() {
        assert!(allowed(false, None));
        assert!(allowed(false, Some("0")));
        assert!(!allowed(true, None));
        assert!(!allowed(true, Some("0")));
        assert!(!allowed(true, Some("true")));
        assert!(allowed(true, Some("1")));
    }

    #[test]
    fn the_loop_follows_the_schedule_and_stops() {
        let stopper = Arc::new(Stopper::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let runs = Arc::new(AtomicUsize::new(0));
        let (s, log, n) = (Arc::clone(&stopper), Arc::clone(&seen), Arc::clone(&runs));
        run(
            &stopper,
            |first, indexing| {
                lock_recovering(&log).push((first, indexing));
                Duration::from_millis(1)
            },
            || {
                let count = n.fetch_add(1, Ordering::SeqCst) + 1;
                if count == 4 {
                    s.stop();
                }
                count < 3
            },
        );
        assert_eq!(runs.load(Ordering::SeqCst), 4);
        // The delay is asked before every run, with what the previous one said.
        assert_eq!(
            *lock_recovering(&seen),
            vec![
                (true, false),
                (false, true),
                (false, true),
                (false, false),
                (false, false)
            ]
        );
    }

    #[test]
    fn stopping_ends_a_long_sleep_at_once() {
        let stopper = Arc::new(Stopper::default());
        let waiter = Arc::clone(&stopper);
        let handle = std::thread::spawn(move || waiter.wait(Duration::from_secs(3600)));
        std::thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        stopper.stop();
        assert!(handle.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(5));
        // Stopped stays stopped.
        assert!(stopper.wait(Duration::from_secs(3600)));
    }
}
