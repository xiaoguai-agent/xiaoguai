//! In-memory conversation history for IM webhooks.
//!
//! Keyed by `conversation_id` (Feishu `chat_id`, DingTalk channel id,
//! etc). Each chat thread gets a sliding window of the last `max_turns`
//! `LlmMessage`s so subsequent webhook deliveries pick up where the
//! last one left off.
//!
//! Why not PG: the `messages` table is FK'd to `sessions`, which is
//! FK'd to `users` and `tenants`. Auto-creating those rows from a
//! Feishu `tenant_key`/`open_id` requires a tenant/user mapping
//! decision that v0.7.2 deliberately defers. v0.7.2 keeps history in
//! process so single-replica deployments get coherent multi-turn
//! conversations now; multi-replica + persistence is a follow-up.

use std::collections::HashMap;

use parking_lot::Mutex;
use xiaoguai_llm::Message as LlmMessage;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
