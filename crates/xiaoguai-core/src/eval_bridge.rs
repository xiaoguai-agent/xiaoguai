//! v0.11.2 — PG-backed `CaseFromSessionSource`.
//!
//! Same layering choice as `audit_bridge.rs` / `today_bridge.rs`: the
//! api crate stays sqlx-free, the SQL lives here. The projection pulls
//! the session's message history + the matching `tool.invoke` rows from
//! `audit_log` (per the canonical pattern documented in
//! `xiaoguai-eval/tests/regression_from_audit.rs`) and hands the
//! [`EvalService`](xiaoguai_api::EvalService) a [`SessionForCase`]
//! ready for projection into YAML.
//!
//! We deliberately don't go through `MessageRepository` — that trait
//! takes a `tenant: Option<&str>` and runs inside an RLS-scoped
//! transaction. The admin path bypasses RLS (same as
//! `PgTodayReader`), so we read directly.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{types::Json, PgPool};
use xiaoguai_api::eval::{
    CaseFromSessionSource, EvalServiceError, SessionForCase, ToolInvocationRecord,
};
use xiaoguai_llm::{Message as LlmMessage, Role as LlmRole};
use xiaoguai_types::ContentBlock;

pub struct PgCaseFromSessionSource {
    pool: PgPool,
}

impl PgCaseFromSessionSource {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_err(e: sqlx::Error) -> EvalServiceError {
    EvalServiceError::Backend(e.to_string())
}

#[async_trait]
impl CaseFromSessionSource for PgCaseFromSessionSource {
    async fn load_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionForCase>, EvalServiceError> {
        let Some(tenant_id): Option<String> =
            sqlx::query_scalar("SELECT tenant_id FROM sessions WHERE id = $1")
                .bind(session_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(map_err)?
        else {
            return Ok(None);
        };

        let message_rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT role, content, created_at
             FROM messages
             WHERE session_id = $1
             ORDER BY created_at ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        let mut input_messages: Vec<LlmMessage> = Vec::new();
        let mut final_assistant_text: Option<String> = None;
        for row in &message_rows {
            let text = flatten_text(&row.content.0);
            let llm_role = map_role(&row.role);
            // Inputs feed the agent loop; assistant rows track the
            // canonical "final reply" but aren't replayed (the mock
            // script does that).
            if matches!(llm_role, LlmRole::Assistant) {
                if !text.is_empty() {
                    final_assistant_text = Some(text);
                }
            } else {
                input_messages.push(LlmMessage {
                    role: llm_role,
                    content: text,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                });
            }
        }

        // Pull tool.invoke audit rows scoped to this session via
        // `details.session_id`. We deliberately don't filter by
        // `tenant_id` in the query: the admin path is cross-tenant, and
        // a tool.invoke row that doesn't carry session_id can't be tied
        // to this session anyway.
        let tool_rows: Vec<ToolInvokeRow> = sqlx::query_as(
            "SELECT details
             FROM audit_log
             WHERE action = 'tool.invoke'
               AND details->>'session_id' = $1
             ORDER BY ts ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        let mut tool_invocations: Vec<ToolInvocationRecord> = Vec::new();
        for row in tool_rows {
            let details = row.details.0;
            let Some(tool_name) = details
                .get("tool_name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
            else {
                continue;
            };
            let arguments_json = details.get("arguments").map_or_else(
                || "{}".into(),
                |v| serde_json::to_string(v).unwrap_or_else(|_| "{}".into()),
            );
            tool_invocations.push(ToolInvocationRecord {
                tool_name,
                arguments_json,
            });
        }

        Ok(Some(SessionForCase {
            session_id: session_id.to_string(),
            tenant_id: Some(tenant_id),
            input_messages,
            tool_invocations,
            final_assistant_text,
        }))
    }
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    role: String,
    content: Json<Vec<ContentBlock>>,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ToolInvokeRow {
    details: Json<serde_json::Value>,
}

fn map_role(s: &str) -> LlmRole {
    match s {
        "system" => LlmRole::System,
        "assistant" => LlmRole::Assistant,
        "tool" => LlmRole::Tool,
        _ => LlmRole::User,
    }
}

fn flatten_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for b in blocks {
        if let ContentBlock::Text { text } = b {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_text_joins_text_blocks_and_skips_others() {
        let blocks = vec![
            ContentBlock::Text {
                text: "hello".into(),
            },
            ContentBlock::ToolCall {
                tool_call_id: "c1".into(),
                name: "search".into(),
                arguments: serde_json::json!({}),
            },
            ContentBlock::Text {
                text: "world".into(),
            },
        ];
        assert_eq!(flatten_text(&blocks), "hello\nworld");
    }

    #[test]
    fn map_role_falls_back_to_user_for_unknown() {
        assert!(matches!(map_role("system"), LlmRole::System));
        assert!(matches!(map_role("garbage"), LlmRole::User));
    }
}
