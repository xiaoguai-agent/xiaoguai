//! [`CardStore`] trait + [`InMemoryCardStore`].
//!
//! The in-memory store is the production-shaped contract. When the parallel
//! branch `feat/kanban-backend-tasks` lands, a `PgCardStore` will implement
//! the same trait — the dispatcher uses `Arc<dyn CardStore>` and doesn't care
//! which backend is wired.
//!
//! ## SKIP LOCKED semantics
//!
//! In a real PostgreSQL backend the claim query is:
//!
//! ```sql
//! UPDATE kanban_cards
//! SET    column = 'RUNNING', claimed_at = now(), attempt = attempt + 1
//! WHERE  id IN (
//!     SELECT id FROM kanban_cards
//!     WHERE  column = 'READY'
//!     ORDER  BY created_at
//!     LIMIT  $1
//!     FOR UPDATE SKIP LOCKED
//! )
//! RETURNING *;
//! ```
//!
//! `SKIP LOCKED` ensures that two concurrent workers never claim the same card;
//! rows locked by a peer are invisibly skipped, so the second worker moves on
//! to the next available READY card without blocking.
//!
//! The [`InMemoryCardStore`] emulates this with a `tokio::sync::Mutex` — the
//! critical section is the whole claim loop, so only one worker enters at a
//! time. The semantics are identical; the implementation is simpler because we
//! don't need row-level locking in-process.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use thiserror::Error;
use tokio::sync::Mutex;

use crate::card::{CardColumn, CardId, KanbanCard, Outcome};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("card not found: {0}")]
    NotFound(CardId),
    #[error("store error: {0}")]
    Internal(String),
}

/// The minimal interface the dispatcher needs from its backing store.
#[async_trait::async_trait]
pub trait CardStore: Send + Sync {
    /// Atomically claim up to `limit` READY cards, moving them to RUNNING.
    ///
    /// This is where SKIP LOCKED semantics live. Implementations must be
    /// concurrent-safe: calling this from N workers simultaneously must never
    /// return the same card to two different workers.
    async fn claim_ready(&self, limit: usize) -> Result<Vec<KanbanCard>, StoreError>;

    /// Move a card to DONE and attach the execution outcome.
    async fn mark_done(&self, id: CardId, outcome: Outcome) -> Result<(), StoreError>;

    /// Move a card to BLOCKED with a human-readable reason.
    async fn mark_blocked(&self, id: CardId, reason: String) -> Result<(), StoreError>;

    /// Move a card back to READY so it can be re-claimed on the next poll.
    /// Used when a worker crashes mid-execution and the card needs retry.
    async fn requeue(&self, id: CardId) -> Result<(), StoreError>;

    /// Read a snapshot of the store for testing / metrics.
    async fn snapshot(&self) -> Vec<KanbanCard>;
}

// ─── In-memory implementation ─────────────────────────────────────────────────

/// Thread-safe in-memory card store (production seam for the PG impl).
///
/// The `Mutex` here is `tokio::sync::Mutex` (async-friendly). The claim
/// critical section holds the lock for the duration of the claim scan so that
/// concurrent workers see a consistent view — this is the SKIP LOCKED analogue.
#[derive(Debug, Default)]
pub struct InMemoryCardStore {
    cards: Mutex<HashMap<CardId, KanbanCard>>,
}

impl InMemoryCardStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a card (for tests / pre-population).
    pub async fn insert(&self, card: KanbanCard) {
        self.cards.lock().await.insert(card.id, card);
    }

    /// Shared handle (test helper).
    #[must_use]
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl CardStore for InMemoryCardStore {
    /// Claim up to `limit` READY cards atomically.
    ///
    /// The Mutex is held for the whole scan + mutate — same semantics as
    /// `SELECT … FOR UPDATE SKIP LOCKED` on Postgres: no other concurrent
    /// claim can interleave and pick the same card.
    async fn claim_ready(&self, limit: usize) -> Result<Vec<KanbanCard>, StoreError> {
        let mut map = self.cards.lock().await;
        let now = Utc::now();

        // Collect READY cards ordered by creation time (oldest first).
        let mut ready_ids: Vec<CardId> = map
            .values()
            .filter(|c| c.column == CardColumn::Ready)
            .map(|c| c.id)
            .collect();
        ready_ids.sort_by_key(|id| map[id].created_at);
        ready_ids.truncate(limit);

        let mut claimed = Vec::with_capacity(ready_ids.len());
        for id in ready_ids {
            let card = map.get_mut(&id).expect("just found above");
            card.column = CardColumn::Running;
            card.claimed_at = Some(now);
            card.attempt += 1;
            claimed.push(card.clone());
        }
        Ok(claimed)
    }

    async fn mark_done(&self, id: CardId, outcome: Outcome) -> Result<(), StoreError> {
        let mut map = self.cards.lock().await;
        let card = map.get_mut(&id).ok_or(StoreError::NotFound(id))?;
        card.column = CardColumn::Done;
        card.outcome = Some(outcome);
        card.completed_at = Some(Utc::now());
        Ok(())
    }

    async fn mark_blocked(&self, id: CardId, reason: String) -> Result<(), StoreError> {
        let mut map = self.cards.lock().await;
        let card = map.get_mut(&id).ok_or(StoreError::NotFound(id))?;
        card.column = CardColumn::Blocked;
        card.blocked_reason = Some(reason);
        card.completed_at = Some(Utc::now());
        Ok(())
    }

    async fn requeue(&self, id: CardId) -> Result<(), StoreError> {
        let mut map = self.cards.lock().await;
        let card = map.get_mut(&id).ok_or(StoreError::NotFound(id))?;
        card.column = CardColumn::Ready;
        card.claimed_at = None;
        Ok(())
    }

    async fn snapshot(&self) -> Vec<KanbanCard> {
        self.cards.lock().await.values().cloned().collect()
    }
}
