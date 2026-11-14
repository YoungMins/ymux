//! Reconnect delay (PROTOCOL.md section 7): 1 s doubling to 60 s, with jitter.

use std::time::Duration;

pub const MIN_SECS: f64 = 1.0;
pub const MAX_SECS: f64 = 60.0;

/// Delay before attempt number `attempt` (0 = the first retry). `jitter` is a
/// fraction in `[0, 1)`; the delay lands in `[0.75, 1.25)` of the nominal one
/// and never exceeds [`MAX_SECS`].
pub fn delay(attempt: u32, jitter: f64) -> Duration {
    let nominal = (MIN_SECS * 2f64.powi(attempt.min(10) as i32)).min(MAX_SECS);
    let j = if jitter.is_finite() {
        jitter.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let secs = (nominal * (0.75 + 0.5 * j)).min(MAX_SECS);
    Duration::from_secs_f64(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_then_caps() {
        let nominal = |a| delay(a, 0.5).as_secs_f64();
        assert!((nominal(0) - 1.0).abs() < 1e-9);
        assert!((nominal(1) - 2.0).abs() < 1e-9);
        assert!((nominal(2) - 4.0).abs() < 1e-9);
        assert!((nominal(5) - 32.0).abs() < 1e-9);
        assert!((nominal(6) - 60.0).abs() < 1e-9);
        assert!((nominal(40) - 60.0).abs() < 1e-9);
    }

    #[test]
    fn jitter_stays_inside_the_bounds() {
        for attempt in [0, 1, 3, 6, 9, 30, u32::MAX] {
            for j in [
                0.0,
                0.25,
                0.5,
                0.999,
                1.0,
                -3.0,
                9.0,
                f64::NAN,
                f64::INFINITY,
            ] {
                let d = delay(attempt, j).as_secs_f64();
                assert!(d >= 0.75, "attempt {attempt} jitter {j}: {d}");
                assert!(d <= MAX_SECS, "attempt {attempt} jitter {j}: {d}");
            }
        }
        // The first retry is at least 750 ms and at most 1.25 s.
        assert!(delay(0, 0.0).as_secs_f64() >= 0.75);
        assert!(delay(0, 0.999).as_secs_f64() <= 1.25);
    }
}
