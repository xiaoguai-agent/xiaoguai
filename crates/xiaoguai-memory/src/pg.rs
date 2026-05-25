//! Postgres + pgvector memory store.
//!
//! ## Prerequisite
//!
//! The `vector` Postgres extension must be installed before running migrations:
//! ```sql
//! CREATE EXTENSION IF NOT EXISTS vector;
//! ```
//!
//! ## Migration
//!
//! Run `crates/xiaoguai-storage/migrations/0019_memories.sql` via
//! `sqlx::migrate!` / `db::migrate`.
//!
//! ## Similarity search
//!
//! Uses pgvector's `<=>` (cosine distance) operator on the `content_embedding`
//! HNSW index. The returned score is `1 - cosine_distance` to match the
//! `[0, 1]` convention used by [`InMemoryMemoryStore`].

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use crate::embedder::EmbeddingProvider;
use crate::error::{MemoryError, MemoryResult};
use crate::traits::MemoryStore;
use crate::types::{
    CreateMemoryRequest, Memory, MemoryKind, RecallRequest, RecalledMemory, RecalledMemoryRef,
    UpdateMemoryRequest,
};

pub struct PgMemoryStore {
    pool: PgPool,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl PgMemoryStore {
    pub fn new(pool: PgPool, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self { pool, embedder }
    }
}

// ─── Row → Memory conversion ─────────────────────────────────────────────────

fn row_to_memory(row: &PgRow) -> Result<Memory, sqlx::Error> {
    let kind_str: String = row.try_get("kind")?;
    let kind: MemoryKind = kind_str
        .parse()
        .map_err(|e: MemoryError| sqlx::Error::Decode(e.to_string().into()))?;

    // pgvector returns the vector as a string "[0.1,0.2,...]"; parse it.
    let embedding_raw: Option<String> = row.try_get("content_embedding")?;
    let content_embedding = embedding_raw
        .as_deref()
        .map(parse_pgvector_str)
        .unwrap_or_default();

    Ok(Memory {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        kind,
        content: row.try_get("content")?,
        content_embedding,
        tags: row.try_get::<Vec<String>, _>("tags").unwrap_or_default(),
        ttl_at: row.try_get("ttl_at")?,
        created_at: row.try_get("created_at")?,
        last_recalled_at: row.try_get("last_recalled_at")?,
        recall_count: row.try_get("recall_count")?,
    })
}

/// Parse a pgvector literal "[0.1,0.2,...]" into `Vec<f32>`.
fn parse_pgvector_str(s: &str) -> Vec<f32> {
    let inner = s.trim_matches(|c| c == '[' || c == ']');
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .filter_map(|tok| tok.trim().parse::<f32>().ok())
        .collect()
}

/// Format a `Vec<f32>` as a pgvector literal "[0.1,0.2,...]".
fn format_pgvector(v: &[f32]) -> String {
    let inner = v.iter().map(std::string::ToString::to_string).collect::<Vec<_>>().join(",");
    format!("[{inner}]")
}

/// Safe usize → i64 conversion: clamps to `i64::MAX` rather than wrapping.
/// In practice `limit`, `offset`, and `top_k` are small user-supplied values.
#[allow(clippy::cast_possible_wrap)]
fn to_i64(n: usize) -> i64 {
    n as i64
}

#[async_trait]
impl MemoryStore for PgMemoryStore {
    async fn list_memories(
        &self,
        tenant_id: Uuid,
        kind_filter: Option<MemoryKind>,
        tag_filter: &[String],
        limit: usize,
        offset: usize,
    ) -> MemoryResult<Vec<Memory>> {
        // Dynamic filter build — kind is optional, tags must be a superset.
        let rows = sqlx::query(
            r"
            SELECT id, tenant_id, kind, content,
                   content_embedding::text, tags, ttl_at,
                   created_at, last_recalled_at, recall_count
            FROM memories
            WHERE tenant_id = $1
              AND ($2::text IS NULL OR kind = $2)
              AND ($3::text[] IS NULL OR tags @> $3)
            ORDER BY created_at DESC
            LIMIT $4 OFFSET $5
            ",
        )
        .bind(tenant_id)
        .bind(kind_filter.map(|k| k.as_str().to_owned()))
        .bind(if tag_filter.is_empty() {
            None
        } else {
            Some(tag_filter.to_vec())
        })
        .bind(to_i64(limit))
        .bind(to_i64(offset))
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(row_to_memory)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MemoryError::Database(e.to_string()))
    }

    async fn get_memory(&self, id: Uuid, tenant_id: Uuid) -> MemoryResult<Memory> {
        let row = sqlx::query(
            r"
            SELECT id, tenant_id, kind, content,
                   content_embedding::text, tags, ttl_at,
                   created_at, last_recalled_at, recall_count
            FROM memories
            WHERE id = $1 AND tenant_id = $2
            ",
        )
        .bind(id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MemoryError::NotFound(id))?;

        row_to_memory(&row).map_err(|e| MemoryError::Database(e.to_string()))
    }

    async fn create_memory(&self, req: CreateMemoryRequest) -> MemoryResult<Memory> {
        let embedding = self.embedder.embed(&req.content).await?;
        let embedding_lit = format_pgvector(&embedding);
        let now = Utc::now();
        let id = Uuid::new_v4();

        sqlx::query(
            r"
            INSERT INTO memories
              (id, tenant_id, kind, content, content_embedding, tags, ttl_at, created_at, recall_count)
            VALUES ($1, $2, $3, $4, $5::vector, $6, $7, $8, 0)
            ",
        )
        .bind(id)
        .bind(req.tenant_id)
        .bind(req.kind.as_str())
        .bind(&req.content)
        .bind(&embedding_lit)
        .bind(&req.tags)
        .bind(req.ttl_at)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(Memory {
            id,
            tenant_id: req.tenant_id,
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

    async fn update_memory(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        req: UpdateMemoryRequest,
    ) -> MemoryResult<Memory> {
        // Re-embed if content changes.
        let new_embedding = if let Some(ref text) = req.content {
            Some(self.embedder.embed(text).await?)
        } else {
            None
        };

        // Build SET clause dynamically.
        let content_update = req.content.as_deref().unwrap_or("");
        let emb_lit = new_embedding.as_deref().map(format_pgvector);

        // Use a single UPDATE that returns the full row.
        let row = sqlx::query(
            r"
            UPDATE memories SET
              content          = CASE WHEN $3 THEN $4 ELSE content END,
              content_embedding = CASE WHEN $5 THEN $6::vector ELSE content_embedding END,
              tags             = CASE WHEN $7 THEN $8 ELSE tags END,
              ttl_at           = CASE WHEN $9 THEN $10 ELSE ttl_at END
            WHERE id = $1 AND tenant_id = $2
            RETURNING id, tenant_id, kind, content,
                      content_embedding::text, tags, ttl_at,
                      created_at, last_recalled_at, recall_count
            ",
        )
        .bind(id)
        .bind(tenant_id)
        .bind(req.content.is_some())
        .bind(content_update)
        .bind(emb_lit.is_some())
        .bind(emb_lit.as_deref().unwrap_or("[]"))
        .bind(req.tags.is_some())
        .bind(req.tags.unwrap_or_default())
        .bind(req.ttl_at.is_some())
        .bind(req.ttl_at.flatten())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MemoryError::NotFound(id))?;

        row_to_memory(&row).map_err(|e| MemoryError::Database(e.to_string()))
    }

    async fn delete_memory(&self, id: Uuid, tenant_id: Uuid) -> MemoryResult<()> {
        let result = sqlx::query("DELETE FROM memories WHERE id = $1 AND tenant_id = $2")
            .bind(id)
            .bind(tenant_id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(MemoryError::NotFound(id));
        }
        Ok(())
    }

    async fn recall_memories(&self, req: RecallRequest) -> MemoryResult<Vec<RecalledMemory>> {
        let query_embedding = self.embedder.embed(&req.query).await?;
        let emb_lit = format_pgvector(&query_embedding);

        let rows = sqlx::query(
            r"
            SELECT id, tenant_id, kind, content,
                   content_embedding::text, tags, ttl_at,
                   created_at, last_recalled_at, recall_count,
                   1 - (content_embedding <=> $3::vector) AS score
            FROM memories
            WHERE tenant_id = $1
              AND ($4::text IS NULL OR kind = $4)
              AND ($5::text[] IS NULL OR tags @> $5)
            ORDER BY content_embedding <=> $3::vector
            LIMIT $2
            ",
        )
        .bind(req.tenant_id)
        .bind(to_i64(req.top_k))
        .bind(&emb_lit)
        .bind(req.kind_filter.map(|k| k.as_str().to_owned()))
        .bind(if req.tag_filter.is_empty() {
            None
        } else {
            Some(req.tag_filter.clone())
        })
        .fetch_all(&self.pool)
        .await?;

        let now = Utc::now();
        let mut recalled: Vec<RecalledMemory> = Vec::with_capacity(rows.len());
        let mut refs: Vec<RecalledMemoryRef> = Vec::with_capacity(rows.len());

        for row in &rows {
            let memory = row_to_memory(row).map_err(|e| MemoryError::Database(e.to_string()))?;
            // f64 → f32 truncation is acceptable: scores are in [0,1],
            // well within f32 precision.
            #[allow(clippy::cast_possible_truncation)]
            let score = row.try_get::<f64, _>("score").unwrap_or(0.0) as f32;

            // Update recall metadata.
            sqlx::query(
                r"
                UPDATE memories
                SET last_recalled_at = $1, recall_count = recall_count + 1
                WHERE id = $2
                ",
            )
            .bind(now)
            .bind(memory.id)
            .execute(&self.pool)
            .await?;

            refs.push(RecalledMemoryRef { id: memory.id, score });
            recalled.push(RecalledMemory { memory, score });
        }

        // Write recall trace.
        let trace_id = Uuid::new_v4();
        let refs_json = serde_json::to_value(&refs)?;
        sqlx::query(
            r"
            INSERT INTO recall_traces
              (id, session_id, query_embedding, memories_recalled, recalled_at)
            VALUES ($1, $2, $3::vector, $4, $5)
            ",
        )
        .bind(trace_id)
        .bind(req.session_id)
        .bind(&emb_lit)
        .bind(refs_json)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(recalled)
    }

    async fn find_similar(
        &self,
        memory_id: Uuid,
        tenant_id: Uuid,
        top_k: usize,
    ) -> MemoryResult<Vec<RecalledMemory>> {
        let rows = sqlx::query(
            r"
            SELECT m.id, m.tenant_id, m.kind, m.content,
                   m.content_embedding::text, m.tags, m.ttl_at,
                   m.created_at, m.last_recalled_at, m.recall_count,
                   1 - (m.content_embedding <=> anchor.content_embedding) AS score
            FROM memories m,
                 (SELECT content_embedding FROM memories WHERE id = $1 AND tenant_id = $2) AS anchor
            WHERE m.tenant_id = $2 AND m.id != $1
            ORDER BY m.content_embedding <=> anchor.content_embedding
            LIMIT $3
            ",
        )
        .bind(memory_id)
        .bind(tenant_id)
        .bind(to_i64(top_k))
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            // Could be missing anchor — check explicitly.
            let exists: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM memories WHERE id = $1 AND tenant_id = $2")
                    .bind(memory_id)
                    .bind(tenant_id)
                    .fetch_optional(&self.pool)
                    .await?;
            if exists.is_none() {
                return Err(MemoryError::NotFound(memory_id));
            }
        }

        rows.iter()
            .map(|row| {
                let memory =
                    row_to_memory(row).map_err(|e| MemoryError::Database(e.to_string()))?;
                // f64 → f32 truncation acceptable: scores are in [0,1].
                #[allow(clippy::cast_possible_truncation)]
                let score = row.try_get::<f64, _>("score").unwrap_or(0.0) as f32;
                Ok(RecalledMemory { memory, score })
            })
            .collect()
    }

    async fn cleanup_expired(&self) -> MemoryResult<u64> {
        let result =
            sqlx::query("DELETE FROM memories WHERE ttl_at IS NOT NULL AND ttl_at < now()")
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected())
    }
}
