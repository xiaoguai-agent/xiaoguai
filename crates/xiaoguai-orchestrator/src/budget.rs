//! Budget enforcement: step cap, token cap, wall-time cap.
//!
//! All fields are optional; absent limits are treated as unlimited.
//! Call `Budget::check` after each step to get an early-stop signal.

use std::time::{Duration, Instant};

/// Resource budget for a supervisor run.
#[derive(Debug, Clone)]
pub struct Budget {
    /// Maximum number of plan steps that may be dispatched.  `None` = unlimited.
    pub max_steps: Option<u32>,
    /// Maximum total tokens across all worker runs.  `None` = unlimited.
    pub max_tokens: Option<u64>,
    /// Maximum wall-clock time for the entire run.  `None` = unlimited.
    pub max_wall_time: Option<Duration>,
    /// Accumulated token usage (updated by the supervisor).
    pub(crate) tokens_used: u64,
    /// Wall-clock start (set when the supervisor starts running).
    pub(crate) started_at: Option<Instant>,
}

impl Budget {
    /// Create a budget with all limits unset (unlimited).
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_steps: None,
            max_tokens: None,
            max_wall_time: None,
            tokens_used: 0,
            started_at: None,
        }
    }

    /// Set a maximum step count.
    #[must_use]
    pub fn with_max_steps(mut self, n: u32) -> Self {
        self.max_steps = Some(n);
        self
    }

    /// Set a maximum token budget.
    #[must_use]
    pub fn with_max_tokens(mut self, n: u64) -> Self {
        self.max_tokens = Some(n);
        self
    }

    /// Set a maximum wall-time duration.
    #[must_use]
    pub fn with_max_wall_time(mut self, d: Duration) -> Self {
        self.max_wall_time = Some(d);
        self
    }

    /// Called at the start of a run to begin wall-time tracking.
    pub(crate) fn start(&mut self) {
        self.started_at = Some(Instant::now());
    }

    /// Accumulate token usage from a completed worker step.
    /// Reserved for v1.2 token-budget enforcement; unused in v1.1.5b.
    #[allow(dead_code)]
    pub(crate) fn add_tokens(&mut self, tokens: u64) {
        self.tokens_used = self.tokens_used.saturating_add(tokens);
    }

    /// Returns `Some(reason)` if any budget limit is exceeded, `None` otherwise.
    ///
    /// `steps_taken` is the number of steps *already dispatched* (before the
    /// next one would start).
    pub(crate) fn check(&self, steps_taken: u32) -> Option<String> {
        if let Some(max) = self.max_steps {
            if steps_taken >= max {
                return Some(format!("max_steps={max} reached"));
            }
        }
        if let Some(max) = self.max_tokens {
            if self.tokens_used >= max {
                return Some(format!(
                    "max_tokens={max} reached (used={})",
                    self.tokens_used
                ));
            }
        }
        if let (Some(max), Some(started)) = (self.max_wall_time, self.started_at) {
            if started.elapsed() >= max {
                return Some(format!("max_wall_time={max:?} exceeded"));
            }
        }
        None
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_budget_never_fires() {
        let mut b = Budget::new();
        b.start();
        assert!(b.check(0).is_none());
        assert!(b.check(1_000_000).is_none());
    }

    #[test]
    fn step_budget_fires_at_limit() {
        let mut b = Budget::new().with_max_steps(3);
        b.start();
        assert!(b.check(2).is_none());
        assert!(b.check(3).is_some());
        assert!(b.check(4).is_some());
    }

    #[test]
    fn token_budget_fires_at_limit() {
        let mut b = Budget::new().with_max_tokens(100);
        b.start();
        b.add_tokens(99);
        assert!(b.check(0).is_none());
        b.add_tokens(1);
        assert!(b.check(0).is_some());
    }

    #[test]
    fn wall_time_budget_fires_after_duration() {
        // 1ms limit — we sleep briefly to exceed it.
        let mut b = Budget::new().with_max_wall_time(Duration::from_millis(1));
        b.start();
        // Spin until elapsed. In practice 1ms fires quickly.
        let deadline = Instant::now() + Duration::from_millis(20);
        while Instant::now() < deadline {
            if b.check(0).is_some() {
                return; // test passed
            }
            std::hint::spin_loop();
        }
        panic!("wall-time budget should have fired within 20ms");
    }
}
