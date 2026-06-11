//! Core domain types for the memory subsystem.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Classification of a stored memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Stable factual knowledge about the user or their environment.
    Facts,
    /// Episodic summary of a past session or event.
    Episodes,
    /// Explicit user preferences or soft constraints.
    Preferences,
}

impl MemoryKind {
    /// Return the canonical string representation stored in the DB.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Facts => "facts",
            Self::Episodes => "episodes",
            Self::Preferences => "preferences",
        }
    }
}

impl std::fmt::Display for MemoryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryKind {
    type Err = crate::MemoryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "facts" => Ok(Self::Facts),
            "episodes" => Ok(Self::Episodes),
            "preferences" => Ok(Self::Preferences),
            other => Err(crate::MemoryError::UnknownKind(other.to_owned())),
        }
    }
}

/// A single long-term memory record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: Uuid,
    pub kind: MemoryKind,
    /// Natural-language content of the memory.
    pub content: String,
    /// Embedding vector (dimension = 384 for production; variable for tests).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub content_embedding: Vec<f32>,
    /// Optional topic tags for B-tree–filtered recall.
    pub tags: Vec<String>,
    /// Wall-clock expiry. `None` means the memory never expires.
    pub ttl_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_recalled_at: Option<DateTime<Utc>>,
    pub recall_count: i32,
}

/// Hard cap on a single memory's `content` in bytes (#288): same 16 KiB
/// budget as the team glossary cap (`xiaoguai-personas/src/teams/model.rs`,
/// `MAX_GLOSSARY_BYTES`). Oversized values are rejected at the write
/// boundary with `InvalidArgument` — truncation is NOT allowed (silently
/// dropping memory tail would be a correctness hazard).
pub const MAX_CONTENT_BYTES: usize = 16_384;

/// Validate memory `content` against [`MAX_CONTENT_BYTES`] (#288). Shared
/// by both store implementations so the API boundary surfaces it as a 400
/// and the JSONL import path reports it as a skipped line.
///
/// # Errors
/// Returns `InvalidArgument` when the content exceeds the byte cap.
pub fn validate_content(content: &str) -> crate::error::MemoryResult<()> {
    if content.len() > MAX_CONTENT_BYTES {
        return Err(crate::MemoryError::InvalidArgument(format!(
            "content is {} bytes; the maximum is {MAX_CONTENT_BYTES} bytes (16 KiB)",
            content.len()
        )));
    }
    Ok(())
}

/// Request to create a new memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMemoryRequest {
    pub kind: MemoryKind,
    pub content: String,
    pub tags: Vec<String>,
    pub ttl_at: Option<DateTime<Utc>>,
}

/// Request to update an existing memory's mutable fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMemoryRequest {
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    pub ttl_at: Option<Option<DateTime<Utc>>>,
}

/// Parameters for semantic recall.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallRequest {
    pub query: String,
    pub top_k: usize,
    /// Optional filter: only recall memories of this kind.
    pub kind_filter: Option<MemoryKind>,
    /// Optional tag filter: only recall memories that contain ALL tags.
    pub tag_filter: Vec<String>,
    /// Session id for the recall trace (observability).
    pub session_id: Option<Uuid>,
}

/// A memory returned from recall, including its similarity score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecalledMemory {
    pub memory: Memory,
    /// Cosine similarity in [0, 1]. Higher is more similar.
    pub score: f32,
}

/// An audit record of one recall invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallTrace {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub query_embedding: Vec<f32>,
    /// Ids and scores of memories returned.
    pub memories_recalled: Vec<RecalledMemoryRef>,
    pub recalled_at: DateTime<Utc>,
}

/// Lightweight reference stored inside `RecallTrace.memories_recalled` (JSONB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecalledMemoryRef {
    pub id: Uuid,
    pub score: f32,
}
