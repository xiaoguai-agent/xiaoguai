//! Core trait for memory storage backends.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::MemoryResult;
use crate::types::{
    CreateMemoryRequest, Memory, RecallRequest, RecalledMemory, UpdateMemoryRequest,
};

/// Unified interface for long-term memory storage and retrieval.
///
/// Both [`PgMemoryStore`] and [`InMemoryMemoryStore`] implement this trait so
/// production and test code share the same call-sites.
#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    // ─── CRUD ───────────────────────────────────────────────────────────────

    /// Return all memories for `tenant_id`, optionally filtered by kind and/or tags.
    async fn list_memories(
        &self,
        tenant_id: Uuid,
        kind_filter: Option<crate::types::MemoryKind>,
        tag_filter: &[String],
        limit: usize,
        offset: usize,
    ) -> MemoryResult<Vec<Memory>>;

    /// Fetch a single memory by id. Returns `Err(NotFound)` when missing.
    async fn get_memory(&self, id: Uuid, tenant_id: Uuid) -> MemoryResult<Memory>;

    /// Persist a new memory. The backend embeds `req.content` automatically.
    async fn create_memory(&self, req: CreateMemoryRequest) -> MemoryResult<Memory>;

    /// Update mutable fields on an existing memory. Re-embeds content when it changes.
    async fn update_memory(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        req: UpdateMemoryRequest,
    ) -> MemoryResult<Memory>;

    /// Hard-delete a memory by id.
    async fn delete_memory(&self, id: Uuid, tenant_id: Uuid) -> MemoryResult<()>;

    // ─── Semantic retrieval ──────────────────────────────────────────────────

    /// Embed `req.query` and return the `top_k` most similar memories by
    /// cosine distance. Writes a [`RecallTrace`] for observability.
    async fn recall_memories(&self, req: RecallRequest) -> MemoryResult<Vec<RecalledMemory>>;

    /// Return the `top_k` memories most similar to `memory_id`'s embedding.
    async fn find_similar(
        &self,
        memory_id: Uuid,
        tenant_id: Uuid,
        top_k: usize,
    ) -> MemoryResult<Vec<RecalledMemory>>;

    // ─── Maintenance ─────────────────────────────────────────────────────────

    /// Delete all memories whose `ttl_at` is in the past. Returns the count
    /// of rows removed. Safe to call repeatedly; idempotent.
    async fn cleanup_expired(&self) -> MemoryResult<u64>;
}
