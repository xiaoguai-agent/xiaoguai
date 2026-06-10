//! SEC-13: in-process inbound event de-duplication.
//!
//! IM platforms re-deliver webhooks on timeout/retry, and a captured
//! request replayed inside an adapter's signature-freshness window would
//! otherwise spawn a duplicate agent run. [`EventDeduper`] remembers
//! `(provider, event_id)` pairs for a bounded TTL so the router can drop
//! repeats before `spawn_agent_reply`.
//!
//! Trade-off (deliberate, fits DEC-033 single-binary / no external
//! queue): the set is **per-process memory** — it does not survive
//! restarts and is not shared across replicas. The default TTL
//! (10 minutes) comfortably covers the platforms' typical retry/replay
//! windows (seconds to a few minutes, and the adapters' ±300 s timestamp
//! tolerance) without growing unbounded; expired entries are pruned
//! opportunistically on every insert.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Default retention for seen event ids. Longer than every adapter's
/// SEC-05 replay window (±300 s) so a replayed-but-fresh request still
/// hits the dedup set.
pub const DEFAULT_DEDUP_TTL: Duration = Duration::from_secs(10 * 60);

/// Time-bounded set of `(provider, event_id)` pairs already handled.
pub struct EventDeduper {
    ttl: Duration,
    /// Interior mutability is required here: the cache is shared across
    /// concurrent webhook handlers. All mutation stays behind this lock;
    /// the critical section is a map lookup/insert (no awaits).
    seen: Mutex<HashMap<(String, String), Instant>>,
}

impl Default for EventDeduper {
    fn default() -> Self {
        Self::new(DEFAULT_DEDUP_TTL)
    }
}

impl EventDeduper {
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `true` when `(provider, event_id)` was already recorded
    /// within the TTL — the caller should acknowledge and drop the event.
    /// Otherwise records the pair and returns `false`.
    ///
    /// Empty `event_id`s are never de-duplicated: some providers omit ids
    /// and collapsing unrelated messages onto one key would drop real
    /// traffic.
    pub fn check_and_record(&self, provider: &str, event_id: &str) -> bool {
        if event_id.is_empty() {
            return false;
        }
        let now = Instant::now();
        let mut seen = self.seen.lock();
        // Opportunistic prune keeps the map bounded without a sweeper task.
        seen.retain(|_, inserted| now.duration_since(*inserted) < self.ttl);
        match seen.entry((provider.to_string(), event_id.to_string())) {
            Entry::Occupied(_) => true,
            Entry::Vacant(slot) => {
                slot.insert(now);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SEC-13: the second delivery of the same `(provider, event_id)` is
    /// flagged as a duplicate.
    #[test]
    fn second_delivery_of_same_event_is_deduplicated() {
        let dedup = EventDeduper::default();
        assert!(
            !dedup.check_and_record("feishu", "evt-1"),
            "first delivery passes"
        );
        assert!(
            dedup.check_and_record("feishu", "evt-1"),
            "second delivery is a duplicate"
        );
    }

    #[test]
    fn distinct_events_and_providers_are_not_deduplicated() {
        let dedup = EventDeduper::default();
        assert!(!dedup.check_and_record("feishu", "evt-1"));
        assert!(!dedup.check_and_record("feishu", "evt-2"));
        assert!(
            !dedup.check_and_record("dingtalk", "evt-1"),
            "same id under another provider is a distinct key"
        );
    }

    #[test]
    fn empty_event_id_is_never_deduplicated() {
        let dedup = EventDeduper::default();
        assert!(!dedup.check_and_record("telegram", ""));
        assert!(!dedup.check_and_record("telegram", ""));
    }

    /// With a zero TTL every entry expires immediately, so the prune on
    /// the next call removes it and the event is processed again.
    #[test]
    fn expired_entries_are_pruned_and_reprocessed() {
        let dedup = EventDeduper::new(Duration::ZERO);
        assert!(!dedup.check_and_record("slack", "evt-1"));
        assert!(!dedup.check_and_record("slack", "evt-1"));
    }
}
