use std::{
    io::Read,
    time::{Duration, Instant},
};

use super::model::Quota;

const MAX_BYTES: u64 = 1024 * 1024;
pub(super) const REFRESH: Duration = Duration::from_secs(10);
const MANUAL_REFRESH: Duration = Duration::from_secs(10);
const FAILURE_BACKOFF: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error)]
#[error("quota unavailable")]
pub(super) struct Unavailable;

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
            FAILURE_BACKOFF
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

    #[test]
    fn successful_quota_refreshes_at_ten_seconds() {
        let mut cache = Cached::refreshed(None, [1; 32], Ok(Vec::new()));
        cache.attempted_at = Instant::now() - Duration::from_millis(9_500);
        assert!(!cache.needs_refresh([1; 32], false));
        cache.attempted_at = Instant::now() - Duration::from_secs(10);
        assert!(cache.needs_refresh([1; 32], false));
    }

    #[test]
    fn quota_refresh_interval_starts_before_network_latency() {
        let started = Instant::now() - Duration::from_secs(10);
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
}
