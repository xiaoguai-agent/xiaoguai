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
//! ## Import guardrails (#288)
//!
//! Both callers (HTTP route + CLI) inherit: a [`MAX_IMPORT_LINES`] cap
//! (fail-fast before any embedding), per-line content byte cap
//! ([`crate::types::MAX_CONTENT_BYTES`]), expired-`ttl_at` lines skipped
//! instead of creating ghost memories, and an early abort after
//! [`MAX_CONSECUTIVE_STORE_FAILURES`] consecutive store failures so an
//! embedder outage is not enumerated line by line.
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

use crate::error::{MemoryError, MemoryResult};
use crate::traits::MemoryStore;
use crate::types::{validate_content, CreateMemoryRequest, Memory, MemoryKind};

/// Prefix marking provenance tags (`source:imported`, `source:im`, ...).
pub const SOURCE_TAG_PREFIX: &str = "source:";

/// Auto-added by the import path when no `source:` tag is present.
pub const SOURCE_IMPORTED_TAG: &str = "source:imported";

/// Hard cap on raw lines (including blank/malformed ones) per import call
/// (#288). Each valid line costs one serial embedder round-trip, so an
/// unbounded document could pin a connection for hours. Larger libraries
/// should be split into multiple `xiaoguai memory import` calls.
pub const MAX_IMPORT_LINES: usize = 10_000;

/// Consecutive `create_memory` failures after which the import aborts
/// (#288). When the embedder (e.g. a remote Ollama) is down, every line
/// fails after a full timeout — without this cut-off, fail-soft would
/// enumerate the whole outage line by line. A single success resets the
/// counter, so scattered bad lines never trip it.
pub const MAX_CONSECUTIVE_STORE_FAILURES: usize = 20;

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
/// were skipped (fail-soft — bad lines never abort the run). `aborted`
/// (#288) is `Some(reason)` when the run stopped early after
/// [`MAX_CONSECUTIVE_STORE_FAILURES`] consecutive store failures; lines
/// after the abort point were never attempted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: Vec<SkippedLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aborted: Option<String>,
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
/// (serde-enforced), blank content and oversized content (#288 — checked
/// here so an oversized line never reaches the embedder).
pub fn parse_import_line(line: &str) -> Result<Option<ImportRecord>, String> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    let record: ImportRecord =
        serde_json::from_str(line).map_err(|e| format!("invalid JSON: {e}"))?;
    if record.content.trim().is_empty() {
        return Err("content must not be blank".to_string());
    }
    validate_content(&record.content).map_err(|e| e.to_string())?;
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
/// Guardrails (#288):
/// * documents over [`MAX_IMPORT_LINES`] raw lines are rejected up front
///   (fail-fast, before any embedder work);
/// * lines whose `ttl_at` is already in the past are skipped (reason:
///   expired ttl) instead of creating ghost memories that are invisible to
///   recall but counted as imported;
/// * after [`MAX_CONSECUTIVE_STORE_FAILURES`] consecutive store failures
///   the run aborts ([`ImportReport::aborted`]) so an embedder outage is
///   not enumerated line by line.
///
/// # Errors
/// Returns `InvalidArgument` when the document exceeds [`MAX_IMPORT_LINES`];
/// otherwise fail-soft (per-line problems go in the report).
pub async fn import_jsonl(store: &dyn MemoryStore, text: &str) -> MemoryResult<ImportReport> {
    // #288: pre-flight line cap, before any parse/embed work.
    let total_lines = text.lines().count();
    if total_lines > MAX_IMPORT_LINES {
        return Err(MemoryError::InvalidArgument(format!(
            "import has {total_lines} lines; the maximum per call is {MAX_IMPORT_LINES} — \
             split the document into smaller batches"
        )));
    }

    let mut report = ImportReport::default();
    let mut consecutive_failures = 0_usize;
    let now = Utc::now();
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
        // #288: a past `ttl_at` would create an already-expired "ghost"
        // memory (filtered out by recall, yet counted as imported) — skip.
        if let Some(ttl) = record.ttl_at {
            if ttl <= now {
                report.skipped.push(SkippedLine {
                    line: line_no,
                    reason: format!("expired ttl_at: {ttl} is in the past"),
                });
                continue;
            }
        }
        let req = CreateMemoryRequest {
            kind: record.kind,
            content: record.content,
            tags: ensure_source_tag(&record.tags),
            ttl_at: record.ttl_at,
        };
        match store.create_memory(req).await {
            Ok(_) => {
                report.imported += 1;
                consecutive_failures = 0;
            }
            Err(e) => {
                report.skipped.push(SkippedLine {
                    line: line_no,
                    reason: format!("store rejected the memory: {e}"),
                });
                consecutive_failures += 1;
                // #288: early stop — when the store/embedder fails this many
                // times in a row it is almost certainly down, not the data.
                if consecutive_failures >= MAX_CONSECUTIVE_STORE_FAILURES {
                    report.aborted = Some(format!(
                        "aborted at line {line_no}: {MAX_CONSECUTIVE_STORE_FAILURES} consecutive \
                         store failures (embedder or store likely unavailable); remaining lines \
                         were not attempted"
                    ));
                    break;
                }
            }
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

    // ─── #288 guardrails ─────────────────────────────────────────────────────

    /// Embedder that always fails — simulates a remote embedder outage.
    struct FailingEmbedder;

    #[async_trait::async_trait]
    impl crate::embedder::EmbeddingProvider for FailingEmbedder {
        async fn embed(&self, _text: &str) -> crate::error::MemoryResult<Vec<f32>> {
            Err(crate::MemoryError::Embedding(
                "embedder unavailable".to_string(),
            ))
        }

        fn dimensions(&self) -> usize {
            384
        }
    }

    #[tokio::test]
    async fn import_rejects_documents_over_the_line_cap() {
        let dst = store();
        // Pre-flight fires on the raw line count — content never parsed.
        let text = "x\n".repeat(MAX_IMPORT_LINES + 1);
        let err = import_jsonl(&dst, &text).await.unwrap_err();
        assert!(matches!(err, crate::MemoryError::InvalidArgument(_)));
        let msg = err.to_string();
        assert!(msg.contains(&MAX_IMPORT_LINES.to_string()), "got: {msg}");
        // Nothing was created.
        assert!(dst
            .list_memories(None, &[], 10, 0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn import_at_exactly_the_line_cap_is_accepted() {
        let dst = store();
        // Blank lines count toward the raw-line cap but import nothing —
        // cheap way to exercise the boundary without 10k embeds.
        let mut text = "\n".repeat(MAX_IMPORT_LINES - 1);
        text.push_str(&line("facts", "boundary ok"));
        text.push('\n');
        let report = import_jsonl(&dst, &text).await.unwrap();
        assert_eq!(report.imported, 1);
        assert!(report.aborted.is_none());
    }

    #[tokio::test]
    async fn import_aborts_after_consecutive_store_failures() {
        let dst = InMemoryMemoryStore::new(Arc::new(FailingEmbedder));
        let valid_line = format!("{}\n", line("facts", "will fail"));
        // More than the cut-off — without early stop all 50 would fail.
        let text = valid_line.repeat(MAX_CONSECUTIVE_STORE_FAILURES + 30);
        let report = import_jsonl(&dst, &text).await.unwrap();
        assert_eq!(report.imported, 0);
        // Exactly the cut-off number of lines was attempted, then abort.
        assert_eq!(report.skipped.len(), MAX_CONSECUTIVE_STORE_FAILURES);
        let aborted = report.aborted.expect("run should have aborted");
        assert!(aborted.contains("consecutive"), "got: {aborted}");
        assert!(
            aborted.contains(&format!("line {MAX_CONSECUTIVE_STORE_FAILURES}")),
            "got: {aborted}"
        );
    }

    /// Embedder that fails only for texts containing "bad" — lets a test
    /// interleave store failures with successes deterministically.
    struct SelectiveEmbedder(InMemoryEmbedder);

    #[async_trait::async_trait]
    impl crate::embedder::EmbeddingProvider for SelectiveEmbedder {
        async fn embed(&self, text: &str) -> crate::error::MemoryResult<Vec<f32>> {
            if text.contains("bad") {
                return Err(crate::MemoryError::Embedding("flaky".to_string()));
            }
            self.0.embed(text).await
        }

        fn dimensions(&self) -> usize {
            self.0.dimensions()
        }
    }

    #[tokio::test]
    async fn a_success_resets_the_consecutive_failure_counter() {
        // More store failures in TOTAL than the cut-off, but never that many
        // in a row — each success resets the counter, so no abort.
        let dst =
            InMemoryMemoryStore::new(Arc::new(SelectiveEmbedder(InMemoryEmbedder::default_dim())));
        let mut text = String::new();
        for i in 0..(MAX_CONSECUTIVE_STORE_FAILURES + 5) {
            text.push_str(&line("facts", &format!("bad {i}")));
            text.push('\n');
            text.push_str(&line("facts", &format!("good {i}")));
            text.push('\n');
        }
        let report = import_jsonl(&dst, &text).await.unwrap();
        assert_eq!(report.imported, MAX_CONSECUTIVE_STORE_FAILURES + 5);
        assert_eq!(report.skipped.len(), MAX_CONSECUTIVE_STORE_FAILURES + 5);
        assert!(report.aborted.is_none());
    }

    #[tokio::test]
    async fn import_skips_oversized_content_before_embedding() {
        let dst = store();
        let big = "x".repeat(crate::types::MAX_CONTENT_BYTES + 1);
        let text = format!("{}\n{}\n", line("facts", &big), line("facts", "small"));
        let report = import_jsonl(&dst, &text).await.unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].line, 1);
        assert!(
            report.skipped[0].reason.contains("bytes"),
            "got: {}",
            report.skipped[0].reason
        );
    }

    #[tokio::test]
    async fn import_skips_lines_with_expired_ttl() {
        let dst = store();
        let past = Utc::now() - chrono::Duration::hours(1);
        let future = Utc::now() + chrono::Duration::hours(1);
        let expired = serde_json::json!({
            "kind": "facts", "content": "ghost", "ttl_at": past.to_rfc3339()
        })
        .to_string();
        let alive = serde_json::json!({
            "kind": "facts", "content": "alive", "ttl_at": future.to_rfc3339()
        })
        .to_string();
        let report = import_jsonl(&dst, &format!("{expired}\n{alive}\n"))
            .await
            .unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].line, 1);
        assert!(
            report.skipped[0].reason.contains("expired ttl_at"),
            "got: {}",
            report.skipped[0].reason
        );
        // The ghost never landed in the store.
        let all = dst.list_memories(None, &[], 10, 0).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].content, "alive");
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
