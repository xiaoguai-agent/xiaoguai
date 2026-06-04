//! Conversation history for IM webhooks.
//!
//! Keyed by `conversation_id` (Feishu `chat_id`, DingTalk channel id, …).
//! Each chat thread gets a sliding window of the last `max_turns`
//! `LlmMessage`s so subsequent webhook deliveries pick up where the
//! last one left off.
//!
//! v0.7.3 introduces the [`ImHistoryStore`] trait. Two impls ship in this
//! crate:
//!
//! * [`ConversationHistory`] — original in-process `HashMap` store. Cheap,
//!   single-replica only. Default for tests and dev.
//! * [`crate::sqlite_history::SqliteImHistoryStore`] — durable, multi-replica safe.
//!   Maps `(provider, tenant_ext, user_ext, conversation_id)` to the
//!   internal tenant/user/session model via the `im_identities` /
//!   `im_conversations` tables and persists each turn to the `messages`
//!   table. Required for HA deployments.
//!
//! The two impls share the same trait surface so production code can
//! flip stores via configuration without touching the webhook handler.

use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::Mutex;
use thiserror::Error;
use xiaoguai_llm::Message as LlmMessage;

/// Identity of an IM conversation, carrying every external key the
/// gateway received from the provider plus the chat ID. Passing the
/// whole struct (rather than just `conversation_id`) lets the PG store
/// resolve / create the tenant + user on demand without re-parsing the
/// webhook payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationIdent {
    pub provider: String,
    pub tenant_external_id: String,
    pub user_external_id: String,
    pub conversation_id: String,
}

impl ConversationIdent {
    #[must_use]
    pub fn new(
        provider: impl Into<String>,
        tenant_external_id: impl Into<String>,
        user_external_id: impl Into<String>,
        conversation_id: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            tenant_external_id: tenant_external_id.into(),
            user_external_id: user_external_id.into(),
            conversation_id: conversation_id.into(),
        }
    }
}

/// Errors a history store can surface. Kept narrow on purpose — the IM
/// webhook handler treats every variant as a transient transport failure
/// and returns HTTP 500 from the provider's perspective; the conversation
/// is dropped for that turn.
#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("history backend error: {0}")]
    Backend(String),
}

/// Abstract conversation history. The router only holds an
/// `Arc<dyn ImHistoryStore>`, so different deployments can pick in-process
/// (default) or PG-backed (HA) without touching the webhook code.
#[async_trait]
pub trait ImHistoryStore: Send + Sync {
    /// Return the trailing window of messages for this conversation. The
    /// PG impl auto-creates the underlying tenant/user/session on first
    /// sight; the in-memory impl just reads its `HashMap`.
    async fn snapshot(&self, ident: &ConversationIdent) -> Result<Vec<LlmMessage>, HistoryError>;

    /// Append the user turn + assistant reply (or any other ordered
    /// sequence) to the conversation. Existing messages stay; the store
    /// may evict to a per-impl maximum window.
    async fn extend(
        &self,
        ident: &ConversationIdent,
        msgs: Vec<LlmMessage>,
    ) -> Result<(), HistoryError>;
}

/// Per-process conversation memory. Cheap to clone (`Arc`-ish via the
/// surrounding `Arc<ConversationHistory>`).
pub struct ConversationHistory {
    inner: Mutex<HashMap<String, Vec<LlmMessage>>>,
    /// How many trailing messages to retain per conversation. Older
    /// turns are dropped at the head when this is exceeded.
    max_turns: usize,
}

impl std::fmt::Debug for ConversationHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationHistory")
            .field("max_turns", &self.max_turns)
            .field("active_conversations", &self.inner.lock().len())
            .finish()
    }
}

impl ConversationHistory {
    /// Build a history store with the given sliding-window cap. v0.7.2
    /// defaults pick `20` — large enough to hold a real-world Q-and-A
    /// chain without ballooning per-process memory.
    #[must_use]
    pub fn new(max_turns: usize) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            max_turns: max_turns.max(1),
        }
    }

    /// Return a clone of the current message list for this conversation.
    /// Empty when the conversation hasn't been seen before.
    #[must_use]
    pub fn snapshot(&self, conversation_id: &str) -> Vec<LlmMessage> {
        self.inner
            .lock()
            .get(conversation_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Append `msg` to `conversation_id`'s window, evicting the oldest
    /// messages when the window exceeds `max_turns`.
    pub fn append(&self, conversation_id: &str, msg: LlmMessage) {
        let mut g = self.inner.lock();
        let entry = g.entry(conversation_id.to_string()).or_default();
        entry.push(msg);
        if entry.len() > self.max_turns {
            let drop = entry.len() - self.max_turns;
            entry.drain(0..drop);
        }
    }

    /// Append several messages at once. Slightly cheaper than calling
    /// `append` N times because it acquires the mutex once.
    pub fn extend(&self, conversation_id: &str, msgs: impl IntoIterator<Item = LlmMessage>) {
        let mut g = self.inner.lock();
        let entry = g.entry(conversation_id.to_string()).or_default();
        entry.extend(msgs);
        if entry.len() > self.max_turns {
            let drop = entry.len() - self.max_turns;
            entry.drain(0..drop);
        }
    }

    /// Drop the history for a conversation. Useful for tests + for a
    /// future `/reset` command.
    pub fn clear(&self, conversation_id: &str) {
        self.inner.lock().remove(conversation_id);
    }
}

#[async_trait]
impl ImHistoryStore for ConversationHistory {
    async fn snapshot(&self, ident: &ConversationIdent) -> Result<Vec<LlmMessage>, HistoryError> {
        // In-memory store: only the conversation_id matters; tenant/user
        // external IDs are ignored. The trait still requires the full
        // struct so the PG impl has what it needs.
        Ok(Self::snapshot(self, &ident.conversation_id))
    }

    async fn extend(
        &self,
        ident: &ConversationIdent,
        msgs: Vec<LlmMessage>,
    ) -> Result<(), HistoryError> {
        Self::extend(self, &ident.conversation_id, msgs);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(conv: &str) -> ConversationIdent {
        ConversationIdent::new("feishu", "ten_x", "ou_alice", conv)
    }

    #[test]
    fn empty_snapshot_for_unknown_conversation() {
        let h = ConversationHistory::new(20);
        assert!(h.snapshot("oc_nope").is_empty());
    }

    #[test]
    fn append_then_snapshot_round_trips() {
        let h = ConversationHistory::new(20);
        h.append("oc_a", LlmMessage::user("hi"));
        h.append("oc_a", LlmMessage::assistant("hello"));
        let s = h.snapshot("oc_a");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].content, "hi");
        assert_eq!(s[1].content, "hello");
    }

    #[test]
    fn sliding_window_drops_oldest() {
        let h = ConversationHistory::new(3);
        for i in 0..5 {
            h.append("oc_a", LlmMessage::user(format!("msg-{i}")));
        }
        let s = h.snapshot("oc_a");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].content, "msg-2");
        assert_eq!(s[1].content, "msg-3");
        assert_eq!(s[2].content, "msg-4");
    }

    #[test]
    fn conversations_do_not_share_state() {
        let h = ConversationHistory::new(20);
        h.append("oc_a", LlmMessage::user("a-msg"));
        h.append("oc_b", LlmMessage::user("b-msg"));
        assert_eq!(h.snapshot("oc_a").len(), 1);
        assert_eq!(h.snapshot("oc_b").len(), 1);
        assert_eq!(h.snapshot("oc_a")[0].content, "a-msg");
    }

    #[test]
    fn clear_removes_the_conversation() {
        let h = ConversationHistory::new(20);
        h.append("oc_a", LlmMessage::user("x"));
        h.clear("oc_a");
        assert!(h.snapshot("oc_a").is_empty());
    }

    #[test]
    fn extend_accepts_multiple_messages() {
        let h = ConversationHistory::new(20);
        h.extend(
            "oc_a",
            vec![LlmMessage::user("u"), LlmMessage::assistant("a")],
        );
        assert_eq!(h.snapshot("oc_a").len(), 2);
    }

    #[tokio::test]
    async fn trait_impl_round_trips_via_ident() {
        let h = ConversationHistory::new(20);
        h.extend(
            "oc_a",
            vec![LlmMessage::user("u"), LlmMessage::assistant("a")],
        );
        let store: &dyn ImHistoryStore = &h;
        let got = store.snapshot(&ident("oc_a")).await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].content, "u");
    }

    #[tokio::test]
    async fn trait_extend_appends_via_ident() {
        let h = ConversationHistory::new(20);
        let store: &dyn ImHistoryStore = &h;
        store
            .extend(
                &ident("oc_a"),
                vec![LlmMessage::user("x"), LlmMessage::assistant("y")],
            )
            .await
            .unwrap();
        let got = store.snapshot(&ident("oc_a")).await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].content, "y");
    }
}
