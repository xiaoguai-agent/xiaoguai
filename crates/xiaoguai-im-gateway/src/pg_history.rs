//! Postgres-backed conversation history for IM webhooks.
//!
//! Resolves `(provider, tenant_ext, user_ext, conversation_id)` to internal
//! `(tenant_id, user_id, session_id)` via the `im_identities` /
//! `im_conversations` mapping tables (see `xiaoguai-storage` migration
//! `0006_im_identity.sql`), then reads / writes the conversation's
//! turn history through the same `MessageRepository` the REST API uses.
//!
//! Parity with [`crate::ConversationHistory`] (the in-process default):
//!
//! * Snapshot returns the trailing `max_turns` text messages of the
//!   underlying session, oldest-first.
//! * Extend appends the supplied `LlmMessage`s in order.
//! * Tool-call / tool-result LLM messages are flattened to a text
//!   `ContentBlock::Text { text: "" }` if they carry no `content` — the
//!   IM gateway only renders text replies, so non-text turns aren't
//!   replayed (consistent with v0.7.2 semantics).

use std::sync::Arc;

use async_trait::async_trait;
use xiaoguai_llm::{Message as LlmMessage, Role as LlmRole};
use xiaoguai_storage::repositories::{
    ExternalConversation, ExternalIdentity, ImIdentityRepository, MessageRepository,
    SessionRepository,
};
use xiaoguai_types::{
    ids::SessionId, ContentBlock, Message as DomainMessage, MessageId, MessageRole,
};

use crate::history::{ConversationIdent, HistoryError, ImHistoryStore};

/// PG-backed conversation history. Hold an `Arc<PgImHistoryStore>` in
/// `GatewayState` exactly like the in-memory `ConversationHistory`.
pub struct PgImHistoryStore {
    identities: Arc<dyn ImIdentityRepository>,
    sessions: Arc<dyn SessionRepository>,
    messages: Arc<dyn MessageRepository>,
    /// Default `agent_defaults.model` propagated onto auto-created sessions
    /// so the row has a meaningful value. Not used for the actual LLM
    /// dispatch (that comes off `AppState.agent_defaults` at handler time).
    default_model: String,
    /// Trailing window of messages returned by `snapshot`. Matches the
    /// in-memory default of 20.
    max_turns: i64,
}

impl PgImHistoryStore {
    #[must_use]
    pub fn new(
        identities: Arc<dyn ImIdentityRepository>,
        sessions: Arc<dyn SessionRepository>,
        messages: Arc<dyn MessageRepository>,
        default_model: impl Into<String>,
        max_turns: usize,
    ) -> Self {
        Self {
            identities,
            sessions,
            messages,
            default_model: default_model.into(),
            max_turns: i64::try_from(max_turns.max(1)).unwrap_or(i64::MAX),
        }
    }
}

impl std::fmt::Debug for PgImHistoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgImHistoryStore")
            .field("default_model", &self.default_model)
            .field("max_turns", &self.max_turns)
            .finish_non_exhaustive()
    }
}

fn backend_err<E: std::fmt::Display>(e: E) -> HistoryError {
    HistoryError::Backend(e.to_string())
}

/// Map an `LlmMessage` to the persisted `DomainMessage` envelope. Text-only
/// for now (matches the v0.7.2 history semantics); `tool_call` payloads on
/// the assistant turn are flattened to an empty text content. Tool-result
/// turns (`Role::Tool`) are skipped by the caller — they belong to the
/// agent's internal loop, not the IM conversation transcript.
fn llm_to_domain(session_id: &str, msg: &LlmMessage) -> Option<DomainMessage> {
    let role = match msg.role {
        LlmRole::User => MessageRole::User,
        LlmRole::Assistant => MessageRole::Assistant,
        LlmRole::System => MessageRole::System,
        // Tool turns don't appear in the IM transcript.
        LlmRole::Tool => return None,
    };
    Some(DomainMessage {
        id: MessageId::new(),
        session_id: SessionId::from(session_id.to_string()),
        role,
        content: vec![ContentBlock::Text {
            text: msg.content.clone(),
        }],
        created_at: chrono::Utc::now(),
    })
}

/// Map a persisted `DomainMessage` back into the `LlmMessage` shape the
/// agent loop consumes. Text-only: `tool_calls` / `tool_results` saved in
/// the DB are ignored when replaying history.
fn domain_to_llm(msg: &DomainMessage) -> Option<LlmMessage> {
    let text = msg
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    match msg.role {
        MessageRole::User => Some(LlmMessage::user(text)),
        MessageRole::Assistant => Some(LlmMessage::assistant(text)),
        MessageRole::System => Some(LlmMessage::system(text)),
        // Tool replies don't roundtrip into IM context.
        MessageRole::Tool => None,
    }
}

#[async_trait]
impl ImHistoryStore for PgImHistoryStore {
    async fn snapshot(&self, ident: &ConversationIdent) -> Result<Vec<LlmMessage>, HistoryError> {
        let identity = self
            .identities
            .resolve_or_create_identity(
                ExternalIdentity {
                    provider: &ident.provider,
                    tenant_external_id: &ident.tenant_external_id,
                    user_external_id: &ident.user_external_id,
                },
                None,
            )
            .await
            .map_err(backend_err)?;

        let conv = self
            .identities
            .resolve_or_create_conversation(
                ExternalConversation {
                    provider: &ident.provider,
                    tenant_external_id: &ident.tenant_external_id,
                    conversation_id: &ident.conversation_id,
                },
                &identity,
                Some(&self.default_model),
            )
            .await
            .map_err(backend_err)?;

        // We want the trailing `max_turns` messages in oldest-first order.
        // Easiest portable read: count + offset + ASC.
        let total = self
            .messages
            .count_by_session(Some(&identity.tenant_id), &conv.session_id)
            .await
            .map_err(backend_err)?;
        let offset = (total - self.max_turns).max(0);
        let rows = self
            .messages
            .list_by_session(
                Some(&identity.tenant_id),
                &conv.session_id,
                self.max_turns,
                offset,
            )
            .await
            .map_err(backend_err)?;
        Ok(rows.iter().filter_map(domain_to_llm).collect())
    }

    async fn resolve_tenant(
        &self,
        ident: &ConversationIdent,
    ) -> Result<Option<String>, HistoryError> {
        let identity = self
            .identities
            .resolve_or_create_identity(
                ExternalIdentity {
                    provider: &ident.provider,
                    tenant_external_id: &ident.tenant_external_id,
                    user_external_id: &ident.user_external_id,
                },
                None,
            )
            .await
            .map_err(backend_err)?;
        Ok(Some(identity.tenant_id))
    }

    async fn extend(
        &self,
        ident: &ConversationIdent,
        msgs: Vec<LlmMessage>,
    ) -> Result<(), HistoryError> {
        let identity = self
            .identities
            .resolve_or_create_identity(
                ExternalIdentity {
                    provider: &ident.provider,
                    tenant_external_id: &ident.tenant_external_id,
                    user_external_id: &ident.user_external_id,
                },
                None,
            )
            .await
            .map_err(backend_err)?;
        let conv = self
            .identities
            .resolve_or_create_conversation(
                ExternalConversation {
                    provider: &ident.provider,
                    tenant_external_id: &ident.tenant_external_id,
                    conversation_id: &ident.conversation_id,
                },
                &identity,
                Some(&self.default_model),
            )
            .await
            .map_err(backend_err)?;

        for msg in &msgs {
            let Some(domain) = llm_to_domain(&conv.session_id, msg) else {
                continue;
            };
            self.messages
                .append(Some(&identity.tenant_id), &domain)
                .await
                .map_err(backend_err)?;
        }
        // Touch the session's updated_at so chat-ui sorts by recency.
        // Ignore NotFound — the session was just resolved/created above.
        let _ = self
            .sessions
            .touch(Some(&identity.tenant_id), &conv.session_id)
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xiaoguai_llm::Message as Lm;

    #[test]
    fn llm_to_domain_skips_tool_role() {
        assert!(llm_to_domain("s", &Lm::tool("call_1", "out")).is_none());
    }

    #[test]
    fn domain_to_llm_joins_text_blocks() {
        let msg = DomainMessage {
            id: MessageId::new(),
            session_id: SessionId::from("s".to_string()),
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "hello ".into(),
                },
                ContentBlock::Text {
                    text: "world".into(),
                },
            ],
            created_at: chrono::Utc::now(),
        };
        let lm = domain_to_llm(&msg).unwrap();
        assert_eq!(lm.content, "hello world");
        assert!(matches!(lm.role, LlmRole::Assistant));
    }
}
