//! Passive circuit breaker per provider.
//!
//! State machine:
//!
//! ```text
//!                          N failures in W
//!   Closed ──────────────────────────────► Open(until = now + cooldown)
//!     ▲                                          │
//!     │                                          │ time ≥ until
//!     │ success                                  ▼
//!     └────────────────────────── HalfOpen ◄────┘
//!                  (success)              │
//!                                         │ failure
//!                                         ▼
//!                            Open(until = now + cooldown)
//! ```
//!
//! Defaults: `N=5`, `W=60s`, `cooldown=30s`. State is per-process and lost
//! on restart — persistence lands in v0.5.5 alongside the API server.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use xiaoguai_types::ProviderId;

#[derive(Debug, Clone, Copy)]
pub struct BreakerConfig {
    pub failure_threshold: usize,
    pub failure_window: Duration,
    pub cooldown: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            failure_window: Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BreakerState {
    Closed,
    Open { until: Instant },
    HalfOpen,
}

/// Clock abstraction so unit tests can drive time deterministically.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug)]
pub struct Breaker {
    state: BreakerState,
    failures: VecDeque<Instant>,
    config: BreakerConfig,
}

impl Breaker {
    #[must_use]
    pub const fn new(config: BreakerConfig) -> Self {
        Self {
            state: BreakerState::Closed,
            failures: VecDeque::new(),
            config,
        }
    }

    /// Inspect the live state, lazily promoting `Open` → `HalfOpen` if the
    /// cooldown has expired.
    pub fn state(&mut self, now: Instant) -> BreakerState {
        if let BreakerState::Open { until } = self.state {
            if now >= until {
                self.state = BreakerState::HalfOpen;
            }
        }
        self.state
    }

    /// Should the next outbound call be attempted? Mutates internal state
    /// when an `Open` breaker has finished cooling down.
    pub fn allows_call(&mut self, now: Instant) -> bool {
        match self.state(now) {
            BreakerState::Closed | BreakerState::HalfOpen => true,
            BreakerState::Open { .. } => false,
        }
    }

    pub fn record_success(&mut self) {
        self.state = BreakerState::Closed;
        self.failures.clear();
    }

    pub fn record_failure(&mut self, now: Instant) {
        match self.state {
            BreakerState::Open { .. } => {
                // Shouldn't normally happen — caller filters with `allows_call`.
                // Refresh the cooldown anyway.
                self.state = BreakerState::Open {
                    until: now + self.config.cooldown,
                };
            }
            BreakerState::HalfOpen => {
                self.state = BreakerState::Open {
                    until: now + self.config.cooldown,
                };
            }
            BreakerState::Closed => {
                self.failures.push_back(now);
                self.evict_old(now);
                if self.failures.len() >= self.config.failure_threshold {
                    self.state = BreakerState::Open {
                        until: now + self.config.cooldown,
                    };
                }
            }
        }
    }

    fn evict_old(&mut self, now: Instant) {
        let cutoff = now.checked_sub(self.config.failure_window);
        if let Some(cutoff) = cutoff {
            while let Some(front) = self.failures.front() {
                if *front < cutoff {
                    self.failures.pop_front();
                } else {
                    break;
                }
            }
        }
    }
}

/// Per-provider breaker pool used by the router. Cheap to clone (Arc inside).
pub struct Breakers {
    inner: Arc<Mutex<HashMap<ProviderId, Breaker>>>,
    config: BreakerConfig,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Breakers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Breakers")
            .field("config", &self.config)
            .field("providers", &self.inner.lock().keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Clone for Breakers {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            config: self.config,
            clock: Arc::clone(&self.clock),
        }
    }
}

impl Breakers {
    #[must_use]
    pub fn new(config: BreakerConfig) -> Self {
        Self::with_clock(config, Arc::new(SystemClock))
    }

    #[must_use]
    pub fn with_clock(config: BreakerConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            config,
            clock,
        }
    }

    #[must_use]
    pub fn allows_call(&self, provider: &ProviderId) -> bool {
        let now = self.clock.now();
        let mut map = self.inner.lock();
        let breaker = map
            .entry(provider.clone())
            .or_insert_with(|| Breaker::new(self.config));
        breaker.allows_call(now)
    }

    pub fn record_success(&self, provider: &ProviderId) {
        let mut map = self.inner.lock();
        let breaker = map
            .entry(provider.clone())
            .or_insert_with(|| Breaker::new(self.config));
        breaker.record_success();
    }

    pub fn record_failure(&self, provider: &ProviderId) {
        let now = self.clock.now();
        let mut map = self.inner.lock();
        let breaker = map
            .entry(provider.clone())
            .or_insert_with(|| Breaker::new(self.config));
        breaker.record_failure(now);
    }

    /// Snapshot the current state of a provider's breaker. Test helper.
    #[must_use]
    pub fn state(&self, provider: &ProviderId) -> BreakerState {
        let now = self.clock.now();
        let mut map = self.inner.lock();
        let breaker = map
            .entry(provider.clone())
            .or_insert_with(|| Breaker::new(self.config));
        breaker.state(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic clock for tests. Advance with `set`.
    #[derive(Debug, Clone)]
    struct FakeClock {
        inner: Arc<Mutex<Instant>>,
    }
    impl FakeClock {
        fn new() -> Self {
            Self {
                inner: Arc::new(Mutex::new(Instant::now())),
            }
        }
        fn advance(&self, d: Duration) {
            let mut g = self.inner.lock();
            *g += d;
        }
    }
    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            *self.inner.lock()
        }
    }

    fn fast_config() -> BreakerConfig {
        BreakerConfig {
            failure_threshold: 5,
            failure_window: Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
        }
    }

    #[test]
    fn five_failures_in_window_opens_breaker() {
        let clock = FakeClock::new();
        let breakers = Breakers::with_clock(fast_config(), Arc::new(clock.clone()));
        let p = ProviderId::from("prov_x".to_string());

        for _ in 0..4 {
            breakers.record_failure(&p);
        }
        assert!(breakers.allows_call(&p), "4 failures should not open");

        breakers.record_failure(&p);
        assert!(!breakers.allows_call(&p), "5th failure must open");
        assert!(matches!(breakers.state(&p), BreakerState::Open { .. }));
    }

    #[test]
    fn failures_spread_beyond_window_do_not_open() {
        let clock = FakeClock::new();
        let breakers = Breakers::with_clock(fast_config(), Arc::new(clock.clone()));
        let p = ProviderId::from("prov_x".to_string());

        // 4 failures, then advance past the window so they're forgotten, then
        // 4 more — should still be closed.
        for _ in 0..4 {
            breakers.record_failure(&p);
        }
        clock.advance(Duration::from_secs(65));
        for _ in 0..4 {
            breakers.record_failure(&p);
        }
        assert!(breakers.allows_call(&p), "stale failures should be evicted");
    }

    #[test]
    fn open_breaker_closes_after_cooldown_via_half_open() {
        let clock = FakeClock::new();
        let breakers = Breakers::with_clock(fast_config(), Arc::new(clock.clone()));
        let p = ProviderId::from("prov_x".to_string());

        for _ in 0..5 {
            breakers.record_failure(&p);
        }
        assert!(!breakers.allows_call(&p));

        clock.advance(Duration::from_secs(31));
        // Cooldown elapsed → HalfOpen → allows one probe call
        assert!(breakers.allows_call(&p));
        assert!(matches!(breakers.state(&p), BreakerState::HalfOpen));

        breakers.record_success(&p);
        assert!(matches!(breakers.state(&p), BreakerState::Closed));
        assert!(breakers.allows_call(&p));
    }

    #[test]
    fn half_open_failure_reopens_with_fresh_cooldown() {
        let clock = FakeClock::new();
        let breakers = Breakers::with_clock(fast_config(), Arc::new(clock.clone()));
        let p = ProviderId::from("prov_x".to_string());

        for _ in 0..5 {
            breakers.record_failure(&p);
        }
        clock.advance(Duration::from_secs(31));
        assert!(matches!(breakers.state(&p), BreakerState::HalfOpen));

        breakers.record_failure(&p);
        assert!(matches!(breakers.state(&p), BreakerState::Open { .. }));
        assert!(!breakers.allows_call(&p));
    }

    #[test]
    fn unknown_provider_defaults_to_closed() {
        let clock = FakeClock::new();
        let breakers = Breakers::with_clock(fast_config(), Arc::new(clock));
        let p = ProviderId::from("prov_new".to_string());
        assert!(breakers.allows_call(&p));
        assert!(matches!(breakers.state(&p), BreakerState::Closed));
    }
}
