//! Conversation-history trimming.
//!
//! v0.5.4 implements a simple sliding window: keep every `System` message at
//! the front, then keep at most `window` of the remaining messages, dropping
//! oldest first. We also enforce a `tool_call_id` invariant — every
//! `Role::Tool` message we retain must be paired with the assistant message
//! that emitted its `tool_call_id`, otherwise providers reject the request.
//!
//! LLM-driven summarisation (and the `/compact` slash command) are tracked in
//! the v0.5.4.1 backlog. The current window keeps the loop bounded without
//! losing the system prompt.

use xiaoguai_llm::{Message, Role};

/// Trim `messages` so the non-system tail has at most `window` entries.
/// When `window == 0` the function is a no-op (sentinel for "unbounded").
#[must_use]
pub fn slide(messages: Vec<Message>, window: usize) -> Vec<Message> {
    if window == 0 {
        return messages;
    }
    let (system, mut tail): (Vec<_>, Vec<_>) = messages
        .into_iter()
        .partition(|m| matches!(m.role, Role::System));
    if tail.len() <= window {
        let mut out = system;
        out.extend(tail);
        return out;
    }
    let drop_count = tail.len() - window;
    tail.drain(..drop_count);
    // Ensure the new tail doesn't start with a dangling `Tool` message — that
    // would reference an assistant `tool_calls` entry we just dropped, which
    // every major provider rejects with a 400.
    while matches!(tail.first().map(|m| m.role), Some(Role::Tool)) {
        tail.remove(0);
    }
    let mut out = system;
    out.extend(tail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use xiaoguai_llm::ToolCallSpec;

    #[test]
    fn noop_when_under_window() {
        let msgs = vec![
            Message::system("s"),
            Message::user("u1"),
            Message::assistant("a1"),
        ];
        let out = slide(msgs.clone(), 4);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn drops_oldest_non_system() {
        let msgs = vec![
            Message::system("s"),
            Message::user("old"),
            Message::assistant("a"),
            Message::user("mid"),
            Message::assistant("b"),
            Message::user("new"),
        ];
        let out = slide(msgs, 3);
        // system stays; last 3 of tail stay.
        assert_eq!(out.len(), 4);
        assert!(matches!(out[0].role, Role::System));
        assert_eq!(out[1].content, "mid");
        assert_eq!(out[3].content, "new");
    }

    #[test]
    fn drops_dangling_tool_message_at_new_head() {
        // After windowing, the head might be a `Tool` whose answering
        // assistant call was dropped. Provider would 400 — strip it.
        let msgs = vec![
            Message::system("s"),
            Message::assistant_tool_calls(vec![ToolCallSpec {
                id: "c1".into(),
                name: "n".into(),
                arguments_json: "{}".into(),
            }]),
            Message::tool("c1", "result"),
            Message::assistant("ok"),
            Message::user("next"),
        ];
        // window=2 → keep last 2 of the 4 non-system: ["ok", "next"] — neither
        // is a Tool, so no further pruning. Verify head is not Tool.
        let out = slide(msgs, 2);
        assert_eq!(out.len(), 3); // system + 2
        assert!(!matches!(out[1].role, Role::Tool));

        // window=3 → keeps tool/assistant/user trio; head is Tool, must be
        // pruned to avoid dangling reference.
        let msgs = vec![
            Message::system("s"),
            Message::assistant_tool_calls(vec![ToolCallSpec {
                id: "c1".into(),
                name: "n".into(),
                arguments_json: "{}".into(),
            }]),
            Message::tool("c1", "result"),
            Message::assistant("ok"),
            Message::user("next"),
        ];
        let out = slide(msgs, 3);
        // Tool at head dropped → 1 system + 2 tail.
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0].role, Role::System));
        assert!(!matches!(out[1].role, Role::Tool));
    }

    #[test]
    fn window_zero_is_unbounded() {
        let msgs = vec![Message::user("a"), Message::user("b"), Message::user("c")];
        let out = slide(msgs.clone(), 0);
        assert_eq!(out.len(), 3);
    }
}
