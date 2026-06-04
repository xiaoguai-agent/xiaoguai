//! v0.11.1 — PG-backed adapter for `xiaoguai-api::today::TodayReader`.
//!
//! Same layering choice as `audit_bridge.rs`: the api crate doesn't
//! depend on sqlx, so the merge query lives in the core binary and the
//! api side stays storage-agnostic. The query strategy is intentionally
//! conservative — three independent SELECTs against the existing
//! migrations (no schema changes for v0.11.1), then merge + sort + cap
//! in Rust. Each branch caps at `query.limit` rows so the worst-case
//! merge size is `3 * limit`.
//!
//! Why not push the union+sort into one SQL query?
//! 1. `sessions` is RLS-scoped; cross-tenant reads under the admin path
//!    would have to either disable RLS or `SET LOCAL` per tenant in
//!    rotation — both ugly. We bypass RLS by running as the superuser
//!    role used for the bootstrap pool.
//! 2. The three sources have very different column shapes; SQL `UNION`
//!    here would mean a lot of `NULL AS ...` casts and a tag column
//!    that Rust has to deserialize back into the enum anyway.
//! 3. Three independent indexes (`ix_sessions_user_updated` for
//!    sessions, `ix_im_conversations_session` join for IM lookup,
//!    `ix_scheduled_job_runs_*_created_at DESC` for runs) each do their
//!    own `LIMIT`. Faster than a single ordered union over the
//!    full Cartesian product.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use xiaoguai_api::today::{TodayError, TodayItem, TodayKind, TodayQuery, TodayReader};
use xiaoguai_storage::ReadWritePool;

pub struct PgTodayReader {
    /// All three fetch methods are pure reads — route to replica.
    pool: ReadWritePool,
}

impl PgTodayReader {
    #[must_use]
    pub fn new(pool: ReadWritePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: ReadWritePool) -> Arc<dyn TodayReader> {
        Arc::new(Self::new(pool))
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "used as `.map_err(map_err)` — changing to `&e` would require closure wrappers at every call site"
)]
fn map_err(e: sqlx::Error) -> TodayError {
    TodayError::Backend(e.to_string())
}

#[async_trait]
impl TodayReader for PgTodayReader {
    async fn list(&self, query: TodayQuery) -> Result<Vec<TodayItem>, TodayError> {
        if query.limit < 0 {
            return Err(TodayError::InvalidArgument("limit must be >= 0".into()));
        }

        let want_chat = matches!(query.kind, None | Some(TodayKind::Chat));
        let want_im = matches!(query.kind, None | Some(TodayKind::Im));
        let want_sched = matches!(query.kind, None | Some(TodayKind::Scheduled));

        let mut merged: Vec<TodayItem> =
            Vec::with_capacity(usize::try_from(query.limit * 3).unwrap_or(150));

        if want_chat {
            merged.extend(self.fetch_chat(&query).await?);
        }
        if want_im {
            merged.extend(self.fetch_im(&query).await?);
        }
        if want_sched {
            merged.extend(self.fetch_scheduled(&query).await?);
        }

        merged.sort_by_key(|r| std::cmp::Reverse(r.ts()));
        let take = usize::try_from(query.limit).unwrap_or(usize::MAX);
        merged.truncate(take);
        Ok(merged)
    }
}

impl PgTodayReader {
    async fn fetch_chat(&self, q: &TodayQuery) -> Result<Vec<TodayItem>, TodayError> {
        // Chat sessions = sessions not present in im_conversations.
        // Preview/count/tool_count come from a single grouped scan of
        // `messages` over the candidate session ids.
        // DEC-033: sessions/im_conversations have no tenant_id. SQLite has
        // no LATERAL — the per-session aggregates become correlated scalar
        // subqueries. `since`/`limit` referenced once each → anonymous `?`.
        // The preview extracts the first text ContentBlock from the JSON
        // array stored in `messages.content` via json_each.
        let rows: Vec<ChatRow> = sqlx::query_as::<_, ChatRow>(
            "WITH candidate AS (
                 SELECT s.id, s.user_id, s.created_at, s.updated_at
                 FROM sessions s
                 LEFT JOIN im_conversations c ON c.session_id = s.id
                 WHERE c.session_id IS NULL
                   AND (? IS NULL OR s.updated_at >= ?)
                 ORDER BY s.updated_at DESC
                 LIMIT ?
             )
             SELECT
                 c.id,
                 c.user_id,
                 c.created_at,
                 c.updated_at,
                 (SELECT COUNT(*) FROM messages WHERE session_id = c.id)
                     AS message_count,
                 (SELECT COUNT(*) FROM messages
                  WHERE session_id = c.id AND role = 'tool')
                     AS tool_count,
                 (
                     SELECT substr(
                         COALESCE(
                             (SELECT json_extract(je.value, '$.text')
                              FROM json_each(m2.content) je
                              WHERE json_extract(je.value, '$.type') = 'text'
                              LIMIT 1),
                             ''
                         ),
                         1, 200
                     )
                     FROM messages m2
                     WHERE m2.session_id = c.id
                     ORDER BY m2.created_at DESC
                     LIMIT 1
                 ) AS last_preview
             FROM candidate c
             ORDER BY c.updated_at DESC",
        )
        .bind(q.since)
        .bind(q.since)
        .bind(q.limit)
        .fetch_all(self.pool.reader())
        .await
        .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|r| TodayItem::Chat {
                ts: r.updated_at,
                session_id: r.id,
                user_id: r.user_id,
                started_at: r.created_at,
                last_message_preview: clean_preview(r.last_preview),
                message_count: r.message_count,
                tool_count: r.tool_count,
            })
            .collect())
    }

    async fn fetch_im(&self, q: &TodayQuery) -> Result<Vec<TodayItem>, TodayError> {
        // DEC-033: no tenant_id. SQLite: correlated subqueries, json_each
        // preview, anonymous binds (since used twice, limit once).
        let rows: Vec<ImRow> = sqlx::query_as::<_, ImRow>(
            "WITH candidate AS (
                 SELECT s.id, s.created_at, s.updated_at,
                        c.provider, c.conversation_id
                 FROM sessions s
                 JOIN im_conversations c ON c.session_id = s.id
                 WHERE (? IS NULL OR s.updated_at >= ?)
                 ORDER BY s.updated_at DESC
                 LIMIT ?
             )
             SELECT
                 c.id, c.created_at, c.updated_at,
                 c.provider, c.conversation_id,
                 (SELECT COUNT(*) FROM messages WHERE session_id = c.id)
                     AS message_count,
                 (
                     SELECT substr(
                         COALESCE(
                             (SELECT json_extract(je.value, '$.text')
                              FROM json_each(m2.content) je
                              WHERE json_extract(je.value, '$.type') = 'text'
                              LIMIT 1),
                             ''
                         ),
                         1, 200
                     )
                     FROM messages m2
                     WHERE m2.session_id = c.id
                     ORDER BY m2.created_at DESC
                     LIMIT 1
                 ) AS last_preview
             FROM candidate c
             ORDER BY c.updated_at DESC",
        )
        .bind(q.since)
        .bind(q.since)
        .bind(q.limit)
        .fetch_all(self.pool.reader())
        .await
        .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|r| TodayItem::Im {
                ts: r.updated_at,
                session_id: r.id,
                provider: r.provider,
                chat_id: r.conversation_id,
                started_at: r.created_at,
                last_message_preview: clean_preview(r.last_preview),
                message_count: r.message_count,
            })
            .collect())
    }

    async fn fetch_scheduled(&self, q: &TodayQuery) -> Result<Vec<TodayItem>, TodayError> {
        // For proactive reason: look at the audit row written by the
        // scheduler (actor = 'scheduler:<job_id>', details->>'run_id' =
        // <run.id>, details->'trigger'->>'type' = 'proactive' →
        // details->'trigger'->>'reason'). We LEFT JOIN on the actor /
        // run_id pair; non-proactive runs simply produce NULL.
        // DEC-033: scheduled_job_runs has no tenant_id. JSON ops use
        // json_extract; audit `details` is TEXT. since used twice, limit once.
        let rows: Vec<SchedRow> = sqlx::query_as::<_, SchedRow>(
            "SELECT
                 r.id, r.job_id, r.status, r.attempt,
                 r.started_at, r.finished_at, r.created_at,
                 r.error_message, r.output_preview,
                 (
                     SELECT json_extract(a.details, '$.trigger.reason')
                     FROM audit_log a
                     WHERE a.actor = ('scheduler:' || r.job_id)
                       AND CAST(json_extract(a.details, '$.run_id') AS INTEGER) = r.id
                       AND json_extract(a.details, '$.trigger.type') = 'proactive'
                     ORDER BY a.id DESC
                     LIMIT 1
                 ) AS reason
             FROM scheduled_job_runs r
             WHERE (? IS NULL OR r.created_at >= ?)
             ORDER BY r.created_at DESC
             LIMIT ?",
        )
        .bind(q.since)
        .bind(q.since)
        .bind(q.limit)
        .fetch_all(self.pool.reader())
        .await
        .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|r| TodayItem::Scheduled {
                ts: r.created_at,
                job_id: r.job_id,
                run_id: r.id,
                attempt: r.attempt,
                status: r.status,
                fired_at: r.started_at.unwrap_or(r.created_at),
                output_preview: r.output_preview,
                error_message: r.error_message,
                reason: r.reason,
            })
            .collect())
    }
}

#[derive(sqlx::FromRow)]
struct ChatRow {
    id: String,
    user_id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    message_count: i64,
    tool_count: i64,
    last_preview: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ImRow {
    id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    provider: String,
    conversation_id: String,
    message_count: i64,
    last_preview: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SchedRow {
    id: i64,
    job_id: String,
    status: String,
    attempt: i32,
    started_at: Option<DateTime<Utc>>,
    #[allow(dead_code)]
    finished_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    error_message: Option<String>,
    output_preview: Option<String>,
    reason: Option<String>,
}

/// `jsonb_path_query_first(...).text` wraps results in double-quotes. Strip
/// them so the preview shows as plain text. Empty strings → `None`.
fn clean_preview(raw: Option<String>) -> Option<String> {
    let s = raw?;
    let trimmed = s.trim();
    let stripped = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed);
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_preview_strips_quotes() {
        assert_eq!(
            clean_preview(Some("\"hello\"".into())),
            Some("hello".into())
        );
        assert_eq!(clean_preview(Some(String::new())), None);
        assert_eq!(clean_preview(None), None);
        // Already unquoted falls through untouched.
        assert_eq!(clean_preview(Some("raw".into())), Some("raw".into()));
    }
}
