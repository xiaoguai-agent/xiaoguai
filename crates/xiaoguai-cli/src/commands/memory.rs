//! `xiaoguai memory {export,import}` — bulk JSONL memory transfer against
//! the **local** `SQLite` store (T7.2,
//! `docs/plans/2026-06-10-memory-multisource.md` §1.3).
//!
//! Like the `provider` commands, these run directly over the local DB (no
//! running server needed); `main.rs` builds the store via
//! `xiaoguai_core::memory_bridge::build_memory_store` (migrate-on-connect,
//! same embedder selection as `serve`), so imports re-embed with the
//! deployment's configured embedder.
//!
//! The JSONL line codec + fail-soft import loop live in
//! [`xiaoguai_memory::jsonl`] — shared with `GET/POST /v1/memories/
//! {export,import}` so the wire format has one source of truth. These
//! functions take `&dyn MemoryStore` so tests run on the in-memory store.

use anyhow::{anyhow, Result};
use xiaoguai_memory::jsonl::{self, ImportReport};
use xiaoguai_memory::{MemoryKind, MemoryStore};

/// Parse an optional `--kind` filter (`facts|episodes|preferences`).
///
/// # Errors
/// Returns a user-facing error naming the valid kinds on bad input.
pub fn parse_kind_filter(kind: Option<&str>) -> Result<Option<MemoryKind>> {
    match kind {
        None => Ok(None),
        Some(k) => k.parse::<MemoryKind>().map(Some).map_err(|_| {
            anyhow!("unknown kind '{k}': expected one of 'facts', 'episodes', 'preferences'")
        }),
    }
}

/// Export all memories (optionally one `--kind`) as a JSONL document.
///
/// # Errors
/// Returns kind-parse and store errors.
pub async fn export(store: &dyn MemoryStore, kind: Option<&str>) -> Result<String> {
    let kind_filter = parse_kind_filter(kind)?;
    jsonl::export_jsonl_from_store(store, kind_filter)
        .await
        .map_err(|e| anyhow!("export memories: {e}"))
}

/// Import a JSONL document, fail-soft per line (blank lines silently
/// skipped, malformed lines reported). Adds `source:imported` unless a line
/// already carries a `source:` tag; content is re-embedded by the store.
/// Shares the #288 guardrails with the HTTP route (line cap, expired-ttl
/// skip, early abort on consecutive store failures — see
/// `xiaoguai_memory::jsonl`).
///
/// # Errors
/// Returns the line-cap rejection (#288) and store-level failures
/// (per-line problems are reported, not errors).
pub async fn import(store: &dyn MemoryStore, content: &str) -> Result<ImportReport> {
    jsonl::import_jsonl(store, content)
        .await
        .map_err(|e| anyhow!("import memories: {e}"))
}

/// Human-readable import summary for stdout. Pure.
#[must_use]
pub fn format_import_report(report: &ImportReport) -> String {
    let mut out = format!(
        "imported {} memorie(s), skipped {} line(s)\n",
        report.imported,
        report.skipped.len()
    );
    for s in &report.skipped {
        out.push_str(&format!("  line {}: {}\n", s.line, s.reason));
    }
    // #288: surface an early abort (consecutive store failures) prominently
    // so the owner knows the remaining lines were never attempted.
    if let Some(reason) = &report.aborted {
        out.push_str(&format!("ABORTED: {reason}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use xiaoguai_memory::{InMemoryEmbedder, InMemoryMemoryStore};

    fn store() -> InMemoryMemoryStore {
        InMemoryMemoryStore::new(Arc::new(InMemoryEmbedder::default_dim()))
    }

    #[tokio::test]
    async fn import_then_export_round_trip() {
        let s = store();
        let text = concat!(
            r#"{"kind":"facts","content":"deploy window is Friday"}"#,
            "\n",
            r#"{"kind":"preferences","content":"terse answers","tags":["source:manual"]}"#,
            "\n",
        );
        let report = import(&s, text).await.unwrap();
        assert_eq!(report.imported, 2);
        assert!(report.skipped.is_empty());

        let all = export(&s, None).await.unwrap();
        assert_eq!(all.lines().count(), 2);
        assert!(all.contains("deploy window is Friday"));
        // Auto-tag on the untagged line; explicit source tag respected.
        assert!(all.contains("source:imported"));
        assert!(all.contains("source:manual"));

        let facts_only = export(&s, Some("facts")).await.unwrap();
        assert_eq!(facts_only.lines().count(), 1);
    }

    #[tokio::test]
    async fn import_reports_skipped_lines() {
        let s = store();
        let text = "garbage\n\n{\"kind\":\"facts\",\"content\":\"ok\"}\n";
        let report = import(&s, text).await.unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].line, 1);

        let rendered = format_import_report(&report);
        assert!(rendered.contains("imported 1"));
        assert!(rendered.contains("line 1:"));
    }

    #[test]
    fn format_import_report_renders_an_abort_reason() {
        // #288: early-stop reason must reach stdout.
        let report = ImportReport {
            imported: 3,
            skipped: vec![],
            aborted: Some("aborted at line 23: 20 consecutive store failures".to_string()),
        };
        let rendered = format_import_report(&report);
        assert!(rendered.contains("imported 3"));
        assert!(rendered.contains("ABORTED: aborted at line 23"));
    }

    #[tokio::test]
    async fn export_rejects_unknown_kind() {
        let s = store();
        let err = export(&s, Some("bogus")).await.unwrap_err();
        assert!(err.to_string().contains("unknown kind 'bogus'"));
    }
}
