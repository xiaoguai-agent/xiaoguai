//! Embedded `SQLite` memory store (single-user, DEC-033).
//!
//! ## Migration
//!
//! Run `crates/xiaoguai-storage/migrations/0019_memories.sql` via
//! `sqlx::migrate!` / `db::migrate`.
//!
//! ## Similarity search (no pgvector)
//!
//! pgvector is gone. `content_embedding` / `query_embedding` are stored as a
//! `BLOB` of 384 little-endian `f32` values. Recall applies the SQL-expressible
//! filters (kind, tags, ttl), then decodes each embedding BLOB and computes
//! cosine similarity in Rust, sorts descending, and takes the top-k. A full
//! scan is acceptable at single-user scale (this is also Phase 3's approach).
//! The returned score is cosine similarity in `[0, 1]` to match the convention
//! used by [`InMemoryMemoryStore`].

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sqlx::{sqlite::SqliteRow, Row, SqlitePool};
use uuid::Uuid;

use crate::embedder::EmbeddingProvider;
use crate::error::{MemoryError, MemoryResult};
use crate::traits::MemoryStore;
use crate::types::{
    CreateMemoryRequest, Memory, MemoryKind, RecallRequest, RecalledMemory, RecalledMemoryRef,
    UpdateMemoryRequest,
};

pub struct SqliteMemoryStore {
    pool: SqlitePool,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl SqliteMemoryStore {
    pub fn new(pool: SqlitePool, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self { pool, embedder }
    }
}

// ─── Embedding BLOB (de)serialization ────────────────────────────────────────

/// Serialize an embedding `&[f32]` into a `Vec<u8>` of little-endian f32 bytes.
fn embedding_to_blob(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// Decode a `BLOB` of little-endian f32 bytes back into a `Vec<f32>`.
/// Trailing bytes that don't form a complete f32 are ignored.
fn blob_to_embedding(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Cosine similarity in `[0, 1]` between two equal-length vectors. Returns `0.0`
/// when either vector is empty, dimensions mismatch, or a magnitude is zero.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())).clamp(0.0, 1.0)
}

// ─── tags JSON (de)serialization ─────────────────────────────────────────────

/// Serialize tags into a JSON array string for the `tags TEXT` column.
fn tags_to_json(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_owned())
}

/// Parse a `tags TEXT` JSON array column into `Vec<String>`.
fn json_to_tags(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

// ─── Row → Memory conversion ─────────────────────────────────────────────────

fn row_to_memory(row: &SqliteRow) -> Result<Memory, sqlx::Error> {
    let kind_str: String = row.try_get("kind")?;
    let kind: MemoryKind = kind_str
        .parse()
        .map_err(|e: MemoryError| sqlx::Error::Decode(e.to_string().into()))?;

    let embedding_blob: Vec<u8> = row.try_get("content_embedding")?;
    let content_embedding = blob_to_embedding(&embedding_blob);

    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str)
        .map_err(|e| sqlx::Error::Decode(format!("invalid memory id {id_str:?}: {e}").into()))?;

    let tags_str: String = row.try_get("tags").unwrap_or_else(|_| "[]".to_owned());

    Ok(Memory {
        id,
        kind,
        content: row.try_get("content")?,
        content_embedding,
        tags: json_to_tags(&tags_str),
        ttl_at: row.try_get("ttl_at")?,
        created_at: row.try_get("created_at")?,
        last_recalled_at: row.try_get("last_recalled_at")?,
        recall_count: row.try_get("recall_count")?,
    })
}

#[async_trait]
impl MemoryStore for SqliteMemoryStore {
    async fn list_memories(
        &self,
        kind_filter: Option<MemoryKind>,
        tag_filter: &[String],
        limit: usize,
        offset: usize,
    ) -> MemoryResult<Vec<Memory>> {
        // Apply kind + tag filters in SQL, then
        // enforce tag-superset semantics in Rust (one EXISTS per tag is awkward
        // with a variable tag count; filtering the small result set is simpler).
        let rows = sqlx::query(
            r"
            SELECT id, kind, content, content_embedding, tags, ttl_at,
                   created_at, last_recalled_at, recall_count
            FROM memories
            WHERE (?1 IS NULL OR kind = ?1)
            ORDER BY created_at DESC
            ",
        )
        .bind(kind_filter.map(|k| k.as_str().to_owned()))
        .fetch_all(&self.pool)
        .await?;

        let memories = rows
            .iter()
            .map(row_to_memory)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MemoryError::Database(e.to_string()))?;

        // tags @> tag_filter  ==>  memory.tags is a superset of tag_filter.
        let filtered = memories
            .into_iter()
            .filter(|m| {
                tag_filter
                    .iter()
                    .all(|want| m.tags.iter().any(|have| have == want))
            })
            .skip(offset)
            .take(limit)
            .collect();

        Ok(filtered)
    }

    async fn get_memory(&self, id: Uuid) -> MemoryResult<Memory> {
        let row = sqlx::query(
            r"
            SELECT id, kind, content, content_embedding, tags, ttl_at,
                   created_at, last_recalled_at, recall_count
            FROM memories
            WHERE id = ?1
            ",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MemoryError::NotFound(id))?;

        row_to_memory(&row).map_err(|e| MemoryError::Database(e.to_string()))
    }

    async fn create_memory(&self, req: CreateMemoryRequest) -> MemoryResult<Memory> {
        let embedding = self.embedder.embed(&req.content).await?;
        let embedding_blob = embedding_to_blob(&embedding);
        let now = Utc::now();
        let id = Uuid::new_v4();

        sqlx::query(
            r"
            INSERT INTO memories
              (id, kind, content, content_embedding, tags, ttl_at, created_at, recall_count)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
            ",
        )
        .bind(id.to_string())
        .bind(req.kind.as_str())
        .bind(&req.content)
        .bind(&embedding_blob)
        .bind(tags_to_json(&req.tags))
        .bind(req.ttl_at)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(Memory {
            id,
            kind: req.kind,
            content: req.content,
            content_embedding: embedding,
            tags: req.tags,
            ttl_at: req.ttl_at,
            created_at: now,
            last_recalled_at: None,
            recall_count: 0,
        })
    }

    async fn update_memory(&self, id: Uuid, req: UpdateMemoryRequest) -> MemoryResult<Memory> {
        // Re-embed if content changes.
        let new_embedding = if let Some(ref text) = req.content {
            Some(self.embedder.embed(text).await?)
        } else {
            None
        };

        let content_update = req.content.as_deref().unwrap_or("");
        let emb_blob = new_embedding.as_deref().map(embedding_to_blob);

        // Single UPDATE that returns the full row. SQLite supports RETURNING.
        let row = sqlx::query(
            r"
            UPDATE memories SET
              content           = CASE WHEN ?2 THEN ?3 ELSE content END,
              content_embedding = CASE WHEN ?4 THEN ?5 ELSE content_embedding END,
              tags              = CASE WHEN ?6 THEN ?7 ELSE tags END,
              ttl_at            = CASE WHEN ?8 THEN ?9 ELSE ttl_at END
            WHERE id = ?1
            RETURNING id, kind, content, content_embedding, tags, ttl_at,
                      created_at, last_recalled_at, recall_count
            ",
        )
        .bind(id.to_string())
        .bind(req.content.is_some())
        .bind(content_update)
        .bind(emb_blob.is_some())
        .bind(emb_blob.unwrap_or_default())
        .bind(req.tags.is_some())
        .bind(tags_to_json(req.tags.as_deref().unwrap_or(&[])))
        .bind(req.ttl_at.is_some())
        .bind(req.ttl_at.flatten())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MemoryError::NotFound(id))?;

        row_to_memory(&row).map_err(|e| MemoryError::Database(e.to_string()))
    }

    async fn delete_memory(&self, id: Uuid) -> MemoryResult<()> {
        let result = sqlx::query("DELETE FROM memories WHERE id = ?1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(MemoryError::NotFound(id));
        }
        Ok(())
    }

    async fn recall_memories(&self, req: RecallRequest) -> MemoryResult<Vec<RecalledMemory>> {
        let query_embedding = self.embedder.embed(&req.query).await?;

        // SQL-expressible filters only: kind + non-expired ttl. The vector
        // similarity is computed in Rust below (brute-force cosine scan).
        let rows = sqlx::query(
            r"
            SELECT id, kind, content, content_embedding, tags, ttl_at,
                   created_at, last_recalled_at, recall_count
            FROM memories
            WHERE (?1 IS NULL OR kind = ?1)
              AND (ttl_at IS NULL OR ttl_at >= strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            ",
        )
        .bind(req.kind_filter.map(|k| k.as_str().to_owned()))
        .fetch_all(&self.pool)
        .await?;

        let candidates = rows
            .iter()
            .map(row_to_memory)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MemoryError::Database(e.to_string()))?;

        // Tag-superset filter + cosine scoring in Rust.
        let mut scored: Vec<(Memory, f32)> = candidates
            .into_iter()
            .filter(|m| {
                req.tag_filter
                    .iter()
                    .all(|want| m.tags.iter().any(|have| have == want))
            })
            .map(|m| {
                let score = cosine_similarity(&query_embedding, &m.content_embedding);
                (m, score)
            })
            .collect();

        // Sort by similarity descending; take top-k.
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(req.top_k);

        let now = Utc::now();
        let mut recalled: Vec<RecalledMemory> = Vec::with_capacity(scored.len());
        let mut refs: Vec<RecalledMemoryRef> = Vec::with_capacity(scored.len());

        for (memory, score) in scored {
            // Update recall metadata.
            sqlx::query(
                r"
                UPDATE memories
                SET last_recalled_at = ?1, recall_count = recall_count + 1
                WHERE id = ?2
                ",
            )
            .bind(now)
            .bind(memory.id.to_string())
            .execute(&self.pool)
            .await?;

            refs.push(RecalledMemoryRef {
                id: memory.id,
                score,
            });
            recalled.push(RecalledMemory { memory, score });
        }

        // Write recall trace (query embedding stored as BLOB too).
        let trace_id = Uuid::new_v4();
        let refs_json = serde_json::to_string(&refs)?;
        sqlx::query(
            r"
            INSERT INTO recall_traces
              (id, session_id, query_embedding, memories_recalled, recalled_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ",
        )
        .bind(trace_id.to_string())
        .bind(req.session_id.map(|s| s.to_string()))
        .bind(embedding_to_blob(&query_embedding))
        .bind(refs_json)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(recalled)
    }

    async fn find_similar(
        &self,
        memory_id: Uuid,
        top_k: usize,
    ) -> MemoryResult<Vec<RecalledMemory>> {
        // Fetch the anchor's embedding first; missing anchor => NotFound.
        let anchor = self.get_memory(memory_id).await.map_err(|e| match e {
            MemoryError::NotFound(_) => MemoryError::NotFound(memory_id),
            other => other,
        })?;

        let rows = sqlx::query(
            r"
            SELECT id, kind, content, content_embedding, tags, ttl_at,
                   created_at, last_recalled_at, recall_count
            FROM memories
            WHERE id != ?1
            ",
        )
        .bind(memory_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let mut scored: Vec<(Memory, f32)> = rows
            .iter()
            .map(|row| {
                let memory =
                    row_to_memory(row).map_err(|e| MemoryError::Database(e.to_string()))?;
                let score = cosine_similarity(&anchor.content_embedding, &memory.content_embedding);
                Ok::<_, MemoryError>((memory, score))
            })
            .collect::<Result<Vec<_>, _>>()?;

        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(top_k);

        Ok(scored
            .into_iter()
            .map(|(memory, score)| RecalledMemory { memory, score })
            .collect())
    }

    async fn cleanup_expired(&self) -> MemoryResult<u64> {
        let result = sqlx::query(
            "DELETE FROM memories WHERE ttl_at IS NOT NULL AND ttl_at < strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}
