//! In-memory memory store — deterministic backend for unit tests.
//!
//! Thread-safe via `parking_lot::RwLock`. All vector comparisons use
//! [`cosine_similarity`] on L2-normalised embeddings produced by the
//! caller-supplied [`EmbeddingProvider`] (typically [`InMemoryEmbedder`]).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use uuid::Uuid;

use crate::embedder::{cosine_similarity, EmbeddingProvider};
use crate::error::{MemoryError, MemoryResult};
use crate::traits::MemoryStore;
use crate::types::{
    CreateMemoryRequest, Memory, MemoryKind, RecallRequest, RecallTrace, RecalledMemory,
    RecalledMemoryRef, UpdateMemoryRequest,
};

/// In-memory implementation of [`MemoryStore`].
///
/// `embed` is called on write (create / update) and on query
/// (`recall_memories`, `find_similar`). The same embedder must be used for
/// both operations to get meaningful cosine similarity results.
pub struct InMemoryMemoryStore {
    embedder: Arc<dyn EmbeddingProvider>,
    memories: RwLock<Vec<Memory>>,
    traces: RwLock<Vec<RecallTrace>>,
}

impl InMemoryMemoryStore {
    pub fn new(embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            embedder,
            memories: RwLock::new(Vec::new()),
            traces: RwLock::new(Vec::new()),
        }
    }
}

#[async_trait]
impl MemoryStore for InMemoryMemoryStore {
    async fn list_memories(
        &self,
        tenant_id: Uuid,
        kind_filter: Option<MemoryKind>,
        tag_filter: &[String],
        limit: usize,
        offset: usize,
    ) -> MemoryResult<Vec<Memory>> {
        let guard = self.memories.read();
        let results: Vec<Memory> = guard
            .iter()
            .filter(|m| m.tenant_id == tenant_id)
            .filter(|m| kind_filter.is_none_or(|k| m.kind == k))
            .filter(|m| tag_filter.iter().all(|tag| m.tags.iter().any(|t| t == tag)))
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        Ok(results)
    }

    async fn get_memory(&self, id: Uuid, tenant_id: Uuid) -> MemoryResult<Memory> {
        let guard = self.memories.read();
        guard
            .iter()
            .find(|m| m.id == id && m.tenant_id == tenant_id)
            .cloned()
            .ok_or(MemoryError::NotFound(id))
    }

    async fn create_memory(&self, req: CreateMemoryRequest) -> MemoryResult<Memory> {
        let embedding = self.embedder.embed(&req.content).await?;
        let now = Utc::now();
        let memory = Memory {
            id: Uuid::new_v4(),
            tenant_id: req.tenant_id,
            kind: req.kind,
            content: req.content,
            content_embedding: embedding,
            tags: req.tags,
            ttl_at: req.ttl_at,
            created_at: now,
            last_recalled_at: None,
            recall_count: 0,
        };
        self.memories.write().push(memory.clone());
        Ok(memory)
    }

    async fn update_memory(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        req: UpdateMemoryRequest,
    ) -> MemoryResult<Memory> {
        // Re-embed when content changes.
        let new_embedding = if let Some(ref text) = req.content {
            Some(self.embedder.embed(text).await?)
        } else {
            None
        };

        let mut guard = self.memories.write();
        let memory = guard
            .iter_mut()
            .find(|m| m.id == id && m.tenant_id == tenant_id)
            .ok_or(MemoryError::NotFound(id))?;

        if let Some(text) = req.content {
            memory.content = text;
        }
        if let Some(emb) = new_embedding {
            memory.content_embedding = emb;
        }
        if let Some(tags) = req.tags {
            memory.tags = tags;
        }
        if let Some(ttl) = req.ttl_at {
            memory.ttl_at = ttl;
        }
        Ok(memory.clone())
    }

    async fn delete_memory(&self, id: Uuid, tenant_id: Uuid) -> MemoryResult<()> {
        let mut guard = self.memories.write();
        let before = guard.len();
        guard.retain(|m| !(m.id == id && m.tenant_id == tenant_id));
        if guard.len() == before {
            return Err(MemoryError::NotFound(id));
        }
        Ok(())
    }

    async fn recall_memories(&self, req: RecallRequest) -> MemoryResult<Vec<RecalledMemory>> {
        let query_embedding = self.embedder.embed(&req.query).await?;

        let mut scored: Vec<(f32, Memory)> = {
            let guard = self.memories.read();
            guard
                .iter()
                .filter(|m| m.tenant_id == req.tenant_id)
                .filter(|m| req.kind_filter.is_none_or(|k| m.kind == k))
                .filter(|m| {
                    req.tag_filter
                        .iter()
                        .all(|tag| m.tags.iter().any(|t| t == tag))
                })
                .map(|m| {
                    let score = cosine_similarity(&query_embedding, &m.content_embedding);
                    (score, m.clone())
                })
                .collect()
        };

        // Sort descending by score.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(req.top_k);

        // Update recall metadata and record trace.
        let now = Utc::now();
        let refs: Vec<RecalledMemoryRef> = scored
            .iter()
            .map(|(score, m)| RecalledMemoryRef {
                id: m.id,
                score: *score,
            })
            .collect();

        {
            let mut guard = self.memories.write();
            for (_, mem) in &scored {
                if let Some(m) = guard.iter_mut().find(|m| m.id == mem.id) {
                    m.last_recalled_at = Some(now);
                    m.recall_count += 1;
                }
            }
        }

        let trace = RecallTrace {
            id: Uuid::new_v4(),
            session_id: req.session_id,
            query_embedding: query_embedding.clone(),
            memories_recalled: refs,
            recalled_at: now,
        };
        self.traces.write().push(trace);

        Ok(scored
            .into_iter()
            .map(|(score, memory)| RecalledMemory { memory, score })
            .collect())
    }

    async fn find_similar(
        &self,
        memory_id: Uuid,
        tenant_id: Uuid,
        top_k: usize,
    ) -> MemoryResult<Vec<RecalledMemory>> {
        let anchor_embedding = {
            let guard = self.memories.read();
            guard
                .iter()
                .find(|m| m.id == memory_id && m.tenant_id == tenant_id)
                .map(|m| m.content_embedding.clone())
                .ok_or(MemoryError::NotFound(memory_id))?
        };

        let mut scored: Vec<(f32, Memory)> = {
            let guard = self.memories.read();
            guard
                .iter()
                .filter(|m| m.tenant_id == tenant_id && m.id != memory_id)
                .map(|m| {
                    let score = cosine_similarity(&anchor_embedding, &m.content_embedding);
                    (score, m.clone())
                })
                .collect()
        };

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        Ok(scored
            .into_iter()
            .map(|(score, memory)| RecalledMemory { memory, score })
            .collect())
    }

    async fn cleanup_expired(&self) -> MemoryResult<u64> {
        let now = Utc::now();
        let mut guard = self.memories.write();
        let before = guard.len();
        guard.retain(|m| m.ttl_at.is_none_or(|ttl| ttl > now));
        Ok((before - guard.len()) as u64)
    }
}
