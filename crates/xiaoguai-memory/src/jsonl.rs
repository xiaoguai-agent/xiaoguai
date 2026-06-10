//! JSONL line codec + bulk import/export over a [`MemoryStore`] (T7.2,
//! `docs/plans/2026-06-10-memory-multisource.md` §1.3).
//!
//! One memory per line: `{kind, content, tags, ttl_at, created_at}`.
//! Embeddings are deliberately NOT exported — they are re-computed on import
//! via [`MemoryStore::create_memory`] (the store's embedder), so exports stay
//! portable across embedding backends.
//!
//! The codec lives here (not in `xiaoguai-api`) so the CLI shares it without
//! depending on the HTTP crate. The routes own the HTTP shapes; the line
//! format and the fail-soft import loop are one source of truth.
//!
//! ## Source-tag convention (§1.2)
//!
//! Tags prefixed [`SOURCE_TAG_PREFIX`] (`source:`) record where a memory
//! came from: `source:imported`, `source:im`, `source:rag`. Pure convention
//! over the existing `tags` column — recall/list tag filtering already
//! works on them. The import path auto-adds [`SOURCE_IMPORTED_TAG`] unless
//! the line already carries some `source:` tag.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::MemoryResult;
use crate::traits::MemoryStore;
use crate::types::{CreateMemoryRequest, Memory, MemoryKind};

/// Prefix marking provenance tags (`source:imported`, `source:im`, ...).
pub const SOURCE_TAG_PREFIX: &str = "source:";

/// Auto-added by the import path when no `source:` tag is present.
pub const SOURCE_IMPORTED_TAG: &str = "source:imported";

/// One exported memory line. `created_at` is informational on export and
/// ignored on import (the store stamps its own).
#[derive(Debug, Clone, Serialize)]
pub struct ExportRecord {
    pub kind: MemoryKind,
    pub content: String,
    pub tags: Vec<String>,
    pub ttl_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<&Memory> for ExportRecord {
    fn from(m: &Memory) -> Self {
        Self {
            kind: m.kind,
            content: m.content.clone(),
            tags: m.tags.clone(),
            ttl_at: m.ttl_at,
            created_at: m.created_at,
        }
    }
}

/// One parsed import line. `created_at` (if present in the JSON) is ignored
/// — serde skips unknown fields — so round-tripping an export works as-is.
#[derive(Debug, Clone, Deserialize)]
pub struct ImportRecord {
    pub kind: MemoryKind,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub ttl_at: Option<DateTime<Utc>>,
}

/// A line the import skipped, with its 1-based line number and the reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedLine {
    pub line: usize,
    pub reason: String,
}

/// Outcome of a bulk import: how many memories were created and which lines
/// were skipped (fail-soft — bad lines never abort the run).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: Vec<SkippedLine>,
}

/// Serialize one memory as a JSONL line (no trailing newline).
///
/// # Errors
/// Returns `Serialization` if serde fails (practically unreachable for
/// these field types, but never swallowed).
pub fn export_line(memory: &Memory) -> MemoryResult<String> {
    Ok(serde_json::to_string(&ExportRecord::from(memory))?)
}

/// Serialize memories as a JSONL document (one line each, trailing newline
/// when non-empty).
///
/// # Errors
/// Propagates the first serde failure.
pub fn export_jsonl(memories: &[Memory]) -> MemoryResult<String> {
    let mut out = String::new();
    for m in memories {
        out.push_str(&export_line(m)?);
        out.push('\n');
    }
    Ok(out)
}

/// List + serialize every memory (optionally one kind) as JSONL. Collects —
/// memory counts are small by design (semantic store, not a log).
///
/// # Errors
/// Propagates store/serde failures.
pub async fn export_jsonl_from_store(
    store: &dyn MemoryStore,
    kind: Option<MemoryKind>,
) -> MemoryResult<String> {
    let memories = store.list_memories(kind, &[], usize::MAX, 0).await?;
    export_jsonl(&memories)
}

/// Parse one import line. `Ok(None)` = blank line (skipped silently);
/// `Err(reason)` = malformed (reported in [`ImportReport::skipped`]).
///
/// # Errors
/// Returns a human-readable reason for malformed JSON, unknown kinds
/// (serde-enforced) and blank content.
pub fn parse_import_line(line: &str) -> Result<Option<ImportRecord>, String> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    let record: ImportRecord =
        serde_json::from_str(line).map_err(|e| format!("invalid JSON: {e}"))?;
    if record.content.trim().is_empty() {
        return Err("content must not be blank".to_string());
    }
    Ok(Some(record))
}

/// Return a new tag list carrying a `source:` tag: the input unchanged when
/// one is already present, otherwise with [`SOURCE_IMPORTED_TAG`] appended.
/// Pure — never mutates the input.
#[must_use]
pub fn ensure_source_tag(tags: &[String]) -> Vec<String> {
    if tags.iter().any(|t| t.starts_with(SOURCE_TAG_PREFIX)) {
        return tags.to_vec();
    }
    let mut out = tags.to_vec();
    out.push(SOURCE_IMPORTED_TAG.to_string());
    out
}

/// Import a JSONL document into `store`, fail-soft per line: blank lines are
/// skipped silently; malformed lines and per-line store failures are
/// reported in [`ImportReport::skipped`] with their 1-based line number.
/// Each valid line goes through [`MemoryStore::create_memory`], which
/// re-embeds the content with the store's embedder. No dedup in v1 —
/// re-importing an export creates twins (plan §4.2).
///
/// # Errors
/// Infallible per the fail-soft contract today; kept as `MemoryResult` so a
/// future pre-flight (e.g. embedder health check) can fail fast.
pub async fn import_jsonl(store: &dyn MemoryStore, text: &str) -> MemoryResult<ImportReport> {
    let mut report = ImportReport::default();
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let record = match parse_import_line(line) {
            Ok(None) => continue,
            Ok(Some(r)) => r,
            Err(reason) => {
                report.skipped.push(SkippedLine {
                    line: line_no,
                    reason,
                });
                continue;
            }
        };
        let req = CreateMemoryRequest {
            kind: record.kind,
            content: record.content,
            tags: ensure_source_tag(&record.tags),
            ttl_at: record.ttl_at,
        };
        match store.create_memory(req).await {
            Ok(_) => report.imported += 1,
            Err(e) => report.skipped.push(SkippedLine {
                line: line_no,
                reason: format!("store rejected the memory: {e}"),
            }),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::InMemoryEmbedder;
    use crate::store::InMemoryMemoryStore;
    use crate::types::RecallRequest;
    use std::sync::Arc;

    fn store() -> InMemoryMemoryStore {
        InMemoryMemoryStore::new(Arc::new(InMemoryEmbedder::default_dim()))
    }

    fn line(kind: &str, content: &str) -> String {
        serde_json::json!({"kind": kind, "content": content}).to_string()
    }

    #[tokio::test]
    async fn export_import_round_trip_re_embeds_and_recall_works() {
        let src = store();
        src.create_memory(CreateMemoryRequest {
            kind: MemoryKind::Facts,
            content: "the deploy window is Friday 02:00 UTC".to_string(),
            tags: vec!["ops".to_string()],
            ttl_at: None,
        })
        .await
        .unwrap();
        src.create_memory(CreateMemoryRequest {
            kind: MemoryKind::Preferences,
            content: "owner prefers terse answers".to_string(),
            tags: vec![],
            ttl_at: None,
        })
        .await
        .unwrap();

        let jsonl = export_jsonl_from_store(&src, None).await.unwrap();
        assert_eq!(jsonl.lines().count(), 2);
        // Embeddings never leak into the export.
        assert!(!jsonl.contains("content_embedding"));

        let dst = store();
        let report = import_jsonl(&dst, &jsonl).await.unwrap();
        assert_eq!(report.imported, 2);
        assert!(report.skipped.is_empty());

        // Re-embedded on import: recall in the destination store works.
        let recalled = dst
            .recall_memories(RecallRequest {
                query: "the deploy window is Friday 02:00 UTC".to_string(),
                top_k: 1,
                kind_filter: None,
                tag_filter: vec![],
                session_id: None,
            })
            .await
            .unwrap();
        assert_eq!(recalled.len(), 1);
        assert!(!recalled[0].memory.content_embedding.is_empty());
    }

    #[tokio::test]
    async fn export_filters_by_kind() {
        let src = store();
        for (kind, content) in [
            (MemoryKind::Facts, "a fact"),
            (MemoryKind::Episodes, "an ep"),
        ] {
            src.create_memory(CreateMemoryRequest {
                kind,
                content: content.to_string(),
                tags: vec![],
                ttl_at: None,
            })
            .await
            .unwrap();
        }
        let jsonl = export_jsonl_from_store(&src, Some(MemoryKind::Facts))
            .await
            .unwrap();
        assert_eq!(jsonl.lines().count(), 1);
        assert!(jsonl.contains("a fact"));
    }

    #[tokio::test]
    async fn import_is_fail_soft_on_mixed_good_and_bad_lines() {
        let dst = store();
        let text = format!(
            "{good}\nnot json at all\n\n{unknown_kind}\n{blank_content}\n{good2}\n",
            good = line("facts", "good one"),
            unknown_kind = line("nonsense", "x"),
            blank_content = line("facts", "   "),
            good2 = line("episodes", "good two"),
        );
        let report = import_jsonl(&dst, &text).await.unwrap();
        assert_eq!(report.imported, 2);
        // Blank line (3) skipped silently; lines 2, 4, 5 reported.
        let lines: Vec<usize> = report.skipped.iter().map(|s| s.line).collect();
        assert_eq!(lines, vec![2, 4, 5]);
        assert!(report.skipped[0].reason.contains("invalid JSON"));
        assert!(report.skipped[1].reason.contains("invalid JSON")); // unknown kind = serde error
        assert!(report.skipped[2].reason.contains("blank"));
    }

    #[tokio::test]
    async fn import_auto_adds_source_imported_tag() {
        let dst = store();
        import_jsonl(&dst, &line("facts", "tag me")).await.unwrap();
        let all = dst.list_memories(None, &[], 10, 0).await.unwrap();
        assert_eq!(all[0].tags, vec![SOURCE_IMPORTED_TAG.to_string()]);
    }

    #[tokio::test]
    async fn import_respects_explicit_source_tag() {
        let dst = store();
        let text =
            serde_json::json!({"kind": "facts", "content": "from im", "tags": ["source:im"]})
                .to_string();
        import_jsonl(&dst, &text).await.unwrap();
        let all = dst.list_memories(None, &[], 10, 0).await.unwrap();
        assert_eq!(all[0].tags, vec!["source:im".to_string()]);
    }

    #[test]
    fn ensure_source_tag_is_pure_and_idempotent() {
        let tags = vec!["ops".to_string()];
        let out = ensure_source_tag(&tags);
        assert_eq!(tags, vec!["ops".to_string()]); // input untouched
        assert_eq!(
            out,
            vec!["ops".to_string(), SOURCE_IMPORTED_TAG.to_string()]
        );
        assert_eq!(ensure_source_tag(&out), out); // already sourced → unchanged
    }

    #[test]
    fn parse_blank_line_is_silent_none() {
        assert!(parse_import_line("").unwrap().is_none());
        assert!(parse_import_line("   \t").unwrap().is_none());
    }

    #[test]
    fn import_record_ignores_created_at_and_unknown_fields() {
        let l = serde_json::json!({
            "kind": "facts", "content": "x",
            "created_at": "2026-01-01T00:00:00Z", "extra": 1
        })
        .to_string();
        let r = parse_import_line(&l).unwrap().unwrap();
        assert_eq!(r.content, "x");
    }
}
