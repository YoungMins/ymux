use std::{
    collections::HashMap,
    io::Read,
    time::{Duration, Instant},
};

use parking_lot::Mutex;

use super::model::Quota;

const MAX_BYTES: u64 = 1024 * 1024;
pub(super) const REFRESH: Duration = Duration::from_secs(60);
const MANUAL_REFRESH: Duration = Duration::from_secs(30);
const FAILURE_BACKOFF: Duration = Duration::from_secs(300);
const EMPTY_RETRY: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
#[error("quota unavailable")]
pub(super) struct Unavailable;

pub(super) type Identity = [u8; 32];

/// Quota state per login. One slot would be overwritten by every other
/// identity's refresh, so two accounts polled in turn would each look "new"
/// every time and defeat the backoff (a 429 storm).
pub(super) type KeyedCache = Mutex<HashMap<Identity, Cached>>;

pub(super) struct Cached {
    identity: [u8; 32],
    pub(super) attempted_at: Instant,
    pub(super) quotas: Vec<Quota>,
    pub(super) unavailable: bool,
}

impl Cached {
    pub(super) fn needs_refresh(&self, identity: [u8; 32], manual: bool) -> bool {
        if self.identity != identity {
            return true;
        }
        let interval = if self.unavailable {
            // A first-ever failure (typically a transient 429) leaves nothing to show,
            // so retry soon; once stale data exists, back off longer.
            if self.quotas.is_empty() {
                EMPTY_RETRY
            } else {
                FAILURE_BACKOFF
            }
        } else if manual {
            MANUAL_REFRESH
        } else {
            REFRESH
        };
        self.attempted_at.elapsed() >= interval
    }

    #[cfg(test)]
    pub(super) fn refreshed(
        previous: Option<Self>,
        identity: [u8; 32],
        result: Result<Vec<Quota>, Unavailable>,
    ) -> Self {
        Self::refreshed_at(previous, identity, result, Instant::now())
    }

    pub(super) fn refreshed_at(
        previous: Option<Self>,
        identity: [u8; 32],
        result: Result<Vec<Quota>, Unavailable>,
        attempted_at: Instant,
    ) -> Self {
        let previous = previous.filter(|c| c.identity == identity);
        let (quotas, unavailable) = match result {
            Ok(quotas) => (quotas, false),
            Err(Unavailable) => (previous.map_or_else(Vec::new, |c| c.quotas), true),
        };
        Self {
            identity,
            attempted_at,
            quotas,
            unavailable,
        }
    }
}

/// One refresh round over every identity seen now. Identities that need a
/// refresh are fetched in parallel while the lock is held (so concurrent
/// panel requests cannot bypass the backoff), results are stored per
/// identity, and identities not seen this round are dropped. Returns each
/// seen identity's `(quotas, unavailable)`.
pub(super) fn round<C: Sync>(
    cache: &KeyedCache,
    jobs: Vec<(Identity, C)>,
    manual: bool,
    fetch: impl Fn(&C) -> Result<Vec<Quota>, Unavailable> + Sync,
) -> HashMap<Identity, (Vec<Quota>, bool)> {
    let mut cache = cache.lock();
    round_in(&mut cache, jobs, manual, fetch)
}

fn round_in<C: Sync>(
    cache: &mut HashMap<Identity, Cached>,
    jobs: Vec<(Identity, C)>,
    manual: bool,
    fetch: impl Fn(&C) -> Result<Vec<Quota>, Unavailable> + Sync,
) -> HashMap<Identity, (Vec<Quota>, bool)> {
    let mut unique: Vec<(Identity, C)> = Vec::new();
    for (identity, credential) in jobs {
        if !unique.iter().any(|(i, _)| *i == identity) {
            unique.push((identity, credential));
        }
    }
    cache.retain(|identity, _| unique.iter().any(|(i, _)| i == identity));
    let due: Vec<&(Identity, C)> = unique
        .iter()
        .filter(|(identity, _)| {
            cache
                .get(identity)
                .map_or(true, |c| c.needs_refresh(*identity, manual))
        })
        .collect();
    let fetched: Vec<(Identity, Result<Vec<Quota>, Unavailable>, Instant)> =
        std::thread::scope(|scope| {
            let handles: Vec<_> = due
                .iter()
                .map(|(identity, credential)| {
                    let fetch = &fetch;
                    scope.spawn(move || {
                        let started = Instant::now();
                        (*identity, fetch(credential), started)
                    })
                })
                .collect();
            handles
                .into_iter()
                .zip(&due)
                .map(|(handle, (identity, _))| {
                    handle
                        .join()
                        .unwrap_or_else(|_| (*identity, Err(Unavailable), Instant::now()))
                })
                .collect()
        });
    for (identity, result, started) in fetched {
        let previous = cache.remove(&identity);
        cache.insert(
            identity,
            Cached::refreshed_at(previous, identity, result, started),
        );
    }
    unique
        .iter()
        .filter_map(|(identity, _)| {
            cache
                .get(identity)
                .map(|c| (*identity, (c.quotas.clone(), c.unavailable)))
        })
        .collect()
}

pub(super) fn read_bounded(reader: impl Read) -> Result<Vec<u8>, Unavailable> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Unavailable)?;
    if u64::try_from(bytes.len()).map_err(|_| Unavailable)? > MAX_BYTES {
        return Err(Unavailable);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(observed_at: u64) -> Vec<Quota> {
        vec![Quota {
            window_minutes: 300,
            used_percent: 20.0,
            resets_at: None,
            observed_at,
        }]
    }

    #[test]
    fn keyed_cache_keeps_each_identitys_backoff() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut cache = HashMap::new();
        let calls = AtomicUsize::new(0);
        // A is rate limited, B is healthy; both are polled in turn.
        let fetch = |who: &u8| {
            calls.fetch_add(1, Ordering::SeqCst);
            if *who == 1 {
                Err(Unavailable)
            } else {
                Ok(quota(7))
            }
        };
        let jobs = || vec![([1; 32], 1u8), ([2; 32], 2u8)];
        let first = round_in(&mut cache, jobs(), false, fetch);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(first[&[1; 32]].1 && first[&[1; 32]].0.is_empty());
        assert!(!first[&[2; 32]].1 && first[&[2; 32]].0.len() == 1);
        // Neither is due again yet: a single slot would have re-fetched
        // whichever identity it did not hold.
        let again = round_in(&mut cache, jobs(), false, fetch);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(again[&[2; 32]].0.len(), 1);
        // Only A's retry interval elapses: only A is fetched.
        cache.get_mut(&[1; 32]).unwrap().attempted_at = Instant::now() - EMPTY_RETRY;
        round_in(&mut cache, jobs(), false, fetch);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn unseen_identities_pruned() {
        let mut cache = HashMap::new();
        let fetch = |_: &u8| Ok(quota(7));
        round_in(
            &mut cache,
            vec![([1; 32], 1u8), ([2; 32], 2u8)],
            false,
            fetch,
        );
        assert_eq!(cache.len(), 2);
        round_in(&mut cache, vec![([2; 32], 2u8)], false, fetch);
        assert!(!cache.contains_key(&[1; 32]) && cache.contains_key(&[2; 32]));
        round_in::<u8>(&mut cache, Vec::new(), false, fetch);
        assert!(cache.is_empty());
    }

    #[test]
    fn duplicate_identities_fetch_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut cache = HashMap::new();
        let calls = AtomicUsize::new(0);
        let out = round_in(
            &mut cache,
            vec![([1; 32], 1u8), ([1; 32], 1u8)],
            false,
            |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(quota(7))
            },
        );
        assert_eq!((calls.load(Ordering::SeqCst), out.len()), (1, 1));
    }

    #[test]
    fn successful_quota_refreshes_at_sixty_seconds() {
        let mut cache = Cached::refreshed(None, [1; 32], Ok(Vec::new()));
        cache.attempted_at = Instant::now() - Duration::from_millis(59_500);
        assert!(!cache.needs_refresh([1; 32], false));
        cache.attempted_at = Instant::now() - Duration::from_secs(60);
        assert!(cache.needs_refresh([1; 32], false));
    }

    #[test]
    fn quota_refresh_interval_starts_before_network_latency() {
        let started = Instant::now() - Duration::from_secs(60);
        let cache = Cached::refreshed_at(None, [1; 32], Ok(Vec::new()), started);
        assert!(cache.needs_refresh([1; 32], false));
    }

    #[test]
    fn failed_quota_preserves_five_minute_backoff_and_identity_change() {
        let mut cache = Cached::refreshed(None, [1; 32], Err(Unavailable));
        cache.attempted_at = Instant::now() - Duration::from_secs(20);
        assert!(!cache.needs_refresh([1; 32], false));
        assert!(!cache.needs_refresh([1; 32], true));
        assert!(cache.needs_refresh([2; 32], false));
        cache.attempted_at = Instant::now() - Duration::from_secs(300);
        assert!(cache.needs_refresh([1; 32], false));
    }

    #[test]
    fn first_failure_retries_soon_but_stale_data_backs_off() {
        let mut empty = Cached::refreshed(None, [1; 32], Err(Unavailable));
        empty.attempted_at = Instant::now() - Duration::from_secs(31);
        assert!(empty.needs_refresh([1; 32], false));
        let good = Ok(vec![Quota {
            window_minutes: 300,
            used_percent: 20.0,
            resets_at: None,
            observed_at: 42,
        }]);
        let first = Cached::refreshed(None, [1; 32], good);
        let mut stale = Cached::refreshed(Some(first), [1; 32], Err(Unavailable));
        stale.attempted_at = Instant::now() - Duration::from_secs(31);
        assert!(!stale.needs_refresh([1; 32], false));
    }
}
