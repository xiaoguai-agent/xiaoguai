//! Retry policy — how to escalate after a job execution fails.
//!
//! Simple exponential backoff with an attempt cap. The runner consults
//! [`RetryPolicy::delay_before_attempt`] *before* attempt `n` (1-indexed);
//! `delay_before_attempt(1)` is always zero (the first attempt never
//! waits). Returning `None` means "stop trying" — the runner then
//! marks the `JobRun` as `Failed`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Retry strategy.
///
/// Defaults give 3 attempts at 30s → 60s → 120s. The `max_attempts`
/// includes the first attempt, so `max_attempts = 1` means "no retry".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff_secs: u64,
    pub multiplier: f64,
    /// Cap on a single backoff to avoid runaway long sleeps.
    pub max_backoff_secs: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_secs: 30,
            multiplier: 2.0,
            max_backoff_secs: 3600,
        }
    }
}

impl RetryPolicy {
    /// "Never retry" preset.
    #[must_use]
    pub fn no_retry() -> Self {
        Self {
            max_attempts: 1,
            initial_backoff_secs: 0,
            multiplier: 1.0,
            max_backoff_secs: 0,
        }
    }

    /// Backoff before `attempt` (1-indexed). Returns `None` once
    /// `attempt > max_attempts`.
    ///
    /// `attempt = 1` always returns `Some(Duration::ZERO)`.
    #[must_use]
    pub fn delay_before_attempt(&self, attempt: u32) -> Option<Duration> {
        if attempt == 0 || attempt > self.max_attempts {
            return None;
        }
        if attempt == 1 {
            return Some(Duration::ZERO);
        }
        // Exponential: initial * multiplier ^ (attempt - 2).
        // attempt=2 → initial; attempt=3 → initial * multiplier; ...
        let exp = f64::from(attempt - 2);
        #[allow(clippy::cast_precision_loss)]
        let base = self.initial_backoff_secs as f64;
        let raw = base * self.multiplier.powf(exp);
        // Clamp to max_backoff_secs.
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let secs = raw.min(self.max_backoff_secs as f64).max(0.0) as u64;
        Some(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_schedule_is_30_60_120() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_before_attempt(1), Some(Duration::ZERO));
        assert_eq!(p.delay_before_attempt(2), Some(Duration::from_secs(30)));
        assert_eq!(p.delay_before_attempt(3), Some(Duration::from_secs(60)));
        assert_eq!(p.delay_before_attempt(4), None);
    }

    #[test]
    fn no_retry_stops_after_first_attempt() {
        let p = RetryPolicy::no_retry();
        assert_eq!(p.delay_before_attempt(1), Some(Duration::ZERO));
        assert_eq!(p.delay_before_attempt(2), None);
    }

    #[test]
    fn max_backoff_clamps_long_sleeps() {
        let p = RetryPolicy {
            max_attempts: 10,
            initial_backoff_secs: 60,
            multiplier: 3.0,
            max_backoff_secs: 600,
        };
        // attempt=2 → 60, attempt=3 → 180, attempt=4 → 540, attempt=5 → would be 1620, clamped to 600.
        assert_eq!(p.delay_before_attempt(2), Some(Duration::from_secs(60)));
        assert_eq!(p.delay_before_attempt(5), Some(Duration::from_secs(600)));
        assert_eq!(p.delay_before_attempt(10), Some(Duration::from_secs(600)));
        assert_eq!(p.delay_before_attempt(11), None);
    }

    #[test]
    fn attempt_zero_rejected() {
        assert_eq!(RetryPolicy::default().delay_before_attempt(0), None);
    }
}
