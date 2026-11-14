//! Two ordering rules of the settings commands and the worker thread, kept
//! pure so they are tested without a window (the runtime that uses them is
//! desktop-only). See CLAUDE.md rule 19.
//!
//! 1. **A stale snapshot is never installed.** The worker reads the device
//!    list and the link keys, and later installs them in the engine. A
//!    settings command (remove a device, lower a capability, pair one, switch
//!    server) can land in between and update the engine; installing the
//!    worker's older list afterwards would revive a removed device or a higher
//!    capability. [`DeviceSet`] is one revision counter that every mutation of
//!    the paired-device set bumps under the same lock the install takes, and a
//!    snapshot records the revision it was read at; installing it is refused
//!    when the revision moved, and the worker reads again.
//! 2. **A new server never sees the old server's setup token.** Switching
//!    server clears the token and the link keys first and only then saves the
//!    new address (and `registered = false`); [`switch_server`] is that order.

use parking_lot::Mutex;

/// Attempts a refresh makes before giving up (a mutation every time in a row
/// is not something a user does).
const MAX_ATTEMPTS: usize = 16;

/// The revision of the paired-device set (devices, their capabilities and
/// their link keys). Lock order: this, then the engine, then (briefly) the
/// config.
#[derive(Debug, Default)]
pub struct DeviceSet {
    rev: Mutex<u64>,
}

impl DeviceSet {
    /// The revision now. A snapshot read after this call is only good while
    /// the revision is still this one.
    pub fn rev(&self) -> u64 {
        *self.rev.lock()
    }

    /// Run a mutation of the device set (config, secrets and engine together)
    /// with the lock held, then bump the revision. Mutations are serialised,
    /// and none can happen between a snapshot's revision check and its
    /// install.
    pub fn mutate<R>(&self, f: impl FnOnce() -> R) -> R {
        let mut g = self.rev.lock();
        let r = f();
        *g = g.wrapping_add(1);
        r
    }

    /// Run `install` only if the revision is still `read_at`; `None` (nothing
    /// installed) otherwise.
    pub fn install_if<R>(&self, read_at: u64, install: impl FnOnce() -> R) -> Option<R> {
        let g = self.rev.lock();
        if *g != read_at {
            return None;
        }
        let r = install();
        drop(g);
        Some(r)
    }

    /// Read a snapshot and install it, reading again whenever a mutation
    /// landed in between. `None` after too many tries in a row.
    pub fn refresh<S, R>(
        &self,
        mut read: impl FnMut() -> S,
        mut install: impl FnMut(S) -> R,
    ) -> Option<R> {
        for _ in 0..MAX_ATTEMPTS {
            let at = self.rev();
            let mut snapshot = Some(read());
            let done = self.install_if(at, || install(snapshot.take().expect("set above")));
            if done.is_some() {
                return done;
            }
        }
        None
    }
}

/// The steps of switching to another server address, in the order they are
/// run by [`switch_server`].
pub trait SwitchSteps {
    /// Forget the stored setup token. `false` when it could not be cleared.
    fn clear_setup_token(&mut self) -> bool;
    /// Forget one device's link key. `false` when it could not be cleared.
    fn clear_link_key(&mut self, device_id: &str) -> bool;
    /// Save the new address, `registered = false` and no devices.
    fn store_config(&mut self) -> bool;
}

/// Why a switch was not done; nothing was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchError {
    Token,
    LinkKey,
    Config,
}

impl SwitchError {
    pub fn message(self) -> &'static str {
        match self {
            SwitchError::Token => {
                "could not clear the stored setup token; the address was not changed"
            }
            SwitchError::LinkKey => {
                "could not remove a paired device's key; the address was not changed"
            }
            SwitchError::Config => "could not save the new address",
        }
    }
}

/// Switch to another server: the old server's setup token first, then the
/// devices' link keys, and only then the config that names the new server and
/// says it has not registered. Any failure stops before the config is saved,
/// so a worker that wakes at any point sees either the old address (and the
/// old registration, which sends no token) or the new one with nothing left to
/// leak.
pub fn switch_server(
    steps: &mut dyn SwitchSteps,
    device_ids: &[String],
) -> Result<(), SwitchError> {
    if !steps.clear_setup_token() {
        return Err(SwitchError::Token);
    }
    for id in device_ids {
        if !steps.clear_link_key(id) {
            return Err(SwitchError::LinkKey);
        }
    }
    if steps.store_config() {
        Ok(())
    } else {
        Err(SwitchError::Config)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::config::model::RemoteDevice;
    use crate::remote::engine::Engine;
    use crate::remote::keys::{Keypair, LinkKey};

    const DESK: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
    const DEV: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";

    fn device(cap: &str) -> (RemoteDevice, LinkKey) {
        (
            RemoteDevice {
                id: DEV.into(),
                name: "Phone".into(),
                pubkey: Keypair::generate().public_b64u(),
                cap: cap.into(),
                added_ms: 1,
            },
            LinkKey::from_bytes([7u8; 32]),
        )
    }

    #[test]
    fn a_snapshot_read_before_a_removal_is_not_installed_after_it() {
        // The shared config the commands edit, and the real engine they feed.
        let config: RefCell<Vec<(RemoteDevice, LinkKey)>> = RefCell::new(vec![device("prompt")]);
        let engine = RefCell::new(Engine::new(DESK, "PC", Keypair::generate()));
        let set = DeviceSet::default();
        let mut reads = 0;
        let mut installed = Vec::new();

        let done = set.refresh(
            || {
                reads += 1;
                let snap = config.borrow().clone();
                if reads == 1 {
                    // The worker has read the list (the device is in it). Now a
                    // settings command removes the device and tells the engine,
                    // in one mutation, before the worker installs.
                    set.mutate(|| {
                        config.borrow_mut().clear();
                        engine.borrow_mut().set_devices(Vec::new());
                    });
                }
                snap
            },
            |snap| {
                installed.push(snap.len());
                engine.borrow_mut().set_devices(snap);
            },
        );

        assert!(done.is_some());
        assert_eq!(reads, 2, "the stale snapshot was refused and read again");
        assert_eq!(
            installed,
            vec![0],
            "only the fresh (empty) list was installed"
        );
        assert_eq!(
            engine.borrow().device_count_for_test(),
            0,
            "no device revived"
        );
    }

    #[test]
    fn a_lowered_capability_is_not_raised_again_by_an_old_snapshot() {
        let config = RefCell::new(vec![device("prompt")]);
        let engine = RefCell::new(Engine::new(DESK, "PC", Keypair::generate()));
        let set = DeviceSet::default();
        let mut first = true;
        set.refresh(
            || {
                let snap = config.borrow().clone();
                if std::mem::take(&mut first) {
                    set.mutate(|| {
                        config.borrow_mut()[0].0.cap = "view".into();
                        engine.borrow_mut().set_devices(config.borrow().clone());
                    });
                }
                snap
            },
            |snap| engine.borrow_mut().set_devices(snap),
        )
        .unwrap();
        assert_eq!(
            engine.borrow().device_cap_for_test(DEV).as_deref(),
            Some("view")
        );
    }

    #[test]
    fn an_unchanged_revision_installs_the_first_read() {
        let set = DeviceSet::default();
        let mut reads = 0;
        let r = set.refresh(
            || {
                reads += 1;
                reads
            },
            |s| s * 10,
        );
        assert_eq!(r, Some(10));
        assert_eq!(reads, 1);
        assert_eq!(set.rev(), 0, "reading does not bump");
        // Every mutation bumps, whatever it returns.
        assert_eq!(set.mutate(|| 5), 5);
        assert_eq!(set.rev(), 1);
        // A snapshot taken at 0 is refused now; one at 1 is installed.
        assert_eq!(set.install_if(0, || ()), None);
        assert_eq!(set.install_if(1, || 3), Some(3));
    }

    #[test]
    fn a_mutation_on_every_attempt_gives_up_instead_of_looping() {
        let set = DeviceSet::default();
        let mut reads = 0;
        let r = set.refresh(
            || {
                reads += 1;
                set.mutate(|| ());
            },
            |()| (),
        );
        assert_eq!(r, None);
        assert_eq!(reads, MAX_ATTEMPTS);
    }

    #[derive(Default)]
    struct Recorder {
        log: Vec<String>,
        fail: Option<&'static str>,
    }

    impl SwitchSteps for Recorder {
        fn clear_setup_token(&mut self) -> bool {
            self.log.push("token".into());
            self.fail != Some("token")
        }
        fn clear_link_key(&mut self, id: &str) -> bool {
            self.log.push(format!("key:{id}"));
            self.fail != Some("key")
        }
        fn store_config(&mut self) -> bool {
            self.log.push("config".into());
            self.fail != Some("config")
        }
    }

    #[test]
    fn the_token_and_keys_are_cleared_before_the_new_address_is_saved() {
        let mut r = Recorder::default();
        let ids = vec!["a".to_string(), "b".to_string()];
        assert_eq!(switch_server(&mut r, &ids), Ok(()));
        assert_eq!(r.log, vec!["token", "key:a", "key:b", "config"]);
        // With no devices it is still token, then config.
        let mut r = Recorder::default();
        switch_server(&mut r, &[]).unwrap();
        assert_eq!(r.log, vec!["token", "config"]);
    }

    #[test]
    fn a_token_that_cannot_be_cleared_keeps_the_old_address() {
        let mut r = Recorder {
            fail: Some("token"),
            ..Recorder::default()
        };
        assert_eq!(
            switch_server(&mut r, &["a".to_string()]),
            Err(SwitchError::Token)
        );
        assert_eq!(
            r.log,
            vec!["token"],
            "nothing else ran, the config was not saved"
        );
        let mut r = Recorder {
            fail: Some("key"),
            ..Recorder::default()
        };
        assert_eq!(
            switch_server(&mut r, &["a".to_string(), "b".to_string()]),
            Err(SwitchError::LinkKey)
        );
        assert!(!r.log.contains(&"config".to_string()));
        let mut r = Recorder {
            fail: Some("config"),
            ..Recorder::default()
        };
        assert_eq!(switch_server(&mut r, &[]), Err(SwitchError::Config));
    }
}
