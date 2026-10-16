//! Retry policy: 5 s * 2^n, capped at 30 min, 8 attempts, `Retry-After` honored.

use std::time::Duration;

pub const MAX_ATTEMPTS: u32 = 8;
const BASE_SECS: u64 = 5;
const CAP_SECS: u64 = 30 * 60;

/// What to do with an HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Retry,
    Drop,
}

/// 2xx ok; 408/429/5xx retry; everything else (other 4xx, 3xx since redirects
/// are not followed, 1xx) is dropped.
pub fn classify(status: u16) -> Outcome {
    match status {
        200..=299 => Outcome::Ok,
        408 | 429 | 500..=599 => Outcome::Retry,
        _ => Outcome::Drop,
    }
}

/// Delay before the next try after `attempt` failed tries (1 = the first try
/// just failed). `None` once `MAX_ATTEMPTS` tries are spent. `jitter` in
/// [0, 1] scales the delay into [50 %, 100 %]; non-finite input counts as 1.
/// `Retry-After` raises the delay (never lowers it), still capped at 30 min.
pub fn next_delay(attempt: u32, retry_after: Option<Duration>, jitter: f64) -> Option<Duration> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    let n = attempt.saturating_sub(1).min(20);
    let base = BASE_SECS.saturating_mul(1u64 << n).min(CAP_SECS) as f64;
    let j = if jitter.is_finite() {
        jitter.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let jittered = Duration::from_secs_f64(base * (0.5 + 0.5 * j));
    let d = match retry_after {
        Some(ra) => jittered.max(ra),
        None => jittered,
    };
    Some(d.min(Duration::from_secs(CAP_SECS)))
}

/// Parses a `Retry-After` header given in delta-seconds. The HTTP-date form is
/// ignored (None).
pub fn parse_retry_after(v: &str) -> Option<Duration> {
    v.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_table() {
        assert_eq!(classify(200), Outcome::Ok);
        assert_eq!(classify(204), Outcome::Ok);
        assert_eq!(classify(408), Outcome::Retry);
        assert_eq!(classify(429), Outcome::Retry);
        assert_eq!(classify(500), Outcome::Retry);
        assert_eq!(classify(503), Outcome::Retry);
        assert_eq!(classify(400), Outcome::Drop);
        assert_eq!(classify(404), Outcome::Drop);
        assert_eq!(classify(302), Outcome::Drop);
        assert_eq!(classify(0), Outcome::Drop);
    }

    #[test]
    fn exponential_with_full_jitter_upper_bound() {
        let secs = |a| next_delay(a, None, 1.0).unwrap().as_secs();
        assert_eq!(secs(1), 5);
        assert_eq!(secs(2), 10);
        assert_eq!(secs(3), 20);
        assert_eq!(secs(7), 320);
    }

    #[test]
    fn jitter_scales_between_half_and_full() {
        assert_eq!(next_delay(1, None, 0.0).unwrap().as_secs_f64(), 2.5);
        assert_eq!(next_delay(1, None, f64::NAN).unwrap().as_secs(), 5);
        assert_eq!(next_delay(1, None, 9.0).unwrap().as_secs(), 5);
        assert_eq!(next_delay(1, None, -3.0).unwrap().as_secs_f64(), 2.5);
    }

    #[test]
    fn gives_up_after_max_attempts() {
        assert!(next_delay(MAX_ATTEMPTS - 1, None, 0.5).is_some());
        assert!(next_delay(MAX_ATTEMPTS, None, 0.5).is_none());
        assert!(next_delay(u32::MAX, None, 0.5).is_none());
    }

    #[test]
    fn retry_after_raises_but_is_capped() {
        let d = next_delay(1, Some(Duration::from_secs(120)), 1.0).unwrap();
        assert_eq!(d.as_secs(), 120);
        let d = next_delay(3, Some(Duration::from_secs(1)), 1.0).unwrap();
        assert_eq!(d.as_secs(), 20);
        let d = next_delay(1, Some(Duration::from_secs(99_999)), 1.0).unwrap();
        assert_eq!(d.as_secs(), 1800);
    }

    #[test]
    fn attempt_zero_is_safe() {
        assert_eq!(next_delay(0, None, 1.0).unwrap().as_secs(), 5);
    }

    #[test]
    fn retry_after_parse() {
        assert_eq!(parse_retry_after(" 30 "), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("-1"), None);
    }
}
