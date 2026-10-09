use std::{
    io::Read,
    time::{Duration, Instant},
};

use super::model::Quota;

const MAX_BYTES: u64 = 1024 * 1024;
pub(super) const REFRESH: Duration = Duration::from_secs(30);
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

    pub(super) fn refreshed(
        previous: Option<Self>,
        identity: [u8; 32],
        result: Result<Vec<Quota>, Unavailable>,
    ) -> Self {
        let previous = previous.filter(|c| c.identity == identity);
        let (quotas, unavailable) = match result {
            Ok(quotas) => (quotas, false),
            Err(Unavailable) => (previous.map_or_else(Vec::new, |c| c.quotas), true),
        };
        Self {
            identity,
            attempted_at: Instant::now(),
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
