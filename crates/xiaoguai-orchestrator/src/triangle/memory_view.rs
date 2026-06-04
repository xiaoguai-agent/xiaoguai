//! `MemoryView` trait + `MemorySnapshot` value type. DEC-021 §4.5
//! invariant: **the same snapshot is read by all three roles for the
//! lifetime of one plan→execute round**. The orchestrator captures
//! it once at Planner-spawn time and passes it by reference to
//! Worker(s) and Critic. Cross-round invalidation is the only path
//! to fresher state.
//!
//! Production impl will be in `xiaoguai-core::orchestrator_bridge`
//! against the real `xiaoguai-memory::SqliteMemoryStore`. Tests use the
//! `InMemoryMemoryView` provided below.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFact {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySnapshot {
    pub round: u32,
    pub facts: Vec<MemoryFact>,
    pub captured_at: DateTime<Utc>,
}

#[async_trait]
pub trait MemoryView: Send + Sync {
    /// Materialise the snapshot for `round`. The implementation MUST
    /// return facts that are stable for the duration of that round —
    /// the orchestrator calls this once per round, never mid-round.
    async fn snapshot(&self, round: u32) -> MemorySnapshot;
}

/// In-memory test fixture — operators inject facts ahead of a test
/// run; snapshots always return everything that was inserted. Not
/// safe for production use because there's no per-tenant isolation.
#[derive(Debug, Default)]
pub struct InMemoryMemoryView {
    facts: parking_lot::Mutex<Vec<MemoryFact>>,
}

impl InMemoryMemoryView {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn insert(&self, key: impl Into<String>, value: impl Into<String>) {
        self.facts.lock().push(MemoryFact {
            key: key.into(),
            value: value.into(),
        });
    }
}

#[async_trait]
impl MemoryView for InMemoryMemoryView {
    async fn snapshot(&self, round: u32) -> MemorySnapshot {
        MemorySnapshot {
            round,
            facts: self.facts.lock().clone(),
            captured_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_view_returns_inserted_facts() {
        let v = InMemoryMemoryView::new();
        v.insert("region", "us-east-1");
        v.insert("model", "claude-sonnet-4-6");

        let snap = v.snapshot(0).await;
        assert_eq!(snap.facts.len(), 2);
        assert_eq!(snap.facts[0].key, "region");
        assert_eq!(snap.facts[1].key, "model");
        assert_eq!(snap.round, 0);
    }

    #[tokio::test]
    async fn snapshot_round_is_preserved() {
        let v = InMemoryMemoryView::new();
        let snap = v.snapshot(7).await;
        assert_eq!(snap.round, 7);
    }
}
