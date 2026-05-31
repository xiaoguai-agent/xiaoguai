//! Deterministic token-count estimator.
//!
//! v0.5.4.1 adds a cheap, dependency-free estimator used by
//! [`xiaoguai-agent`]'s history-compaction path. The estimator is
//! intentionally **conservative** — it rounds up — so that callers using
//! it to gate compaction triggers err on the side of compacting earlier
//! rather than later.
//!
//! We do *not* depend on tiktoken or a model-specific tokenizer here:
//!
//! - tiktoken adds a heavy native dependency and licensing complexity
//! - Ollama / Anthropic / Gemini all use different tokenizers anyway
//! - the estimator only needs to be accurate-enough to drive a trigger
//!   threshold, not exact
//!
//! The 4-characters-per-token heuristic is the long-standing GPT-3
//! rule-of-thumb and is conservative for most natural-language inputs.
//! Code and CJK tokenise denser; whitespace-heavy text tokenises sparser.
//! Both directions are within the safety margin of `trigger_at_pct`
//! (default 75 %).

use crate::types::Message;

/// Conservative per-string estimate.
///
/// Returns the number of tokens we *expect* the string to consume,
/// rounded up. An empty string returns `0`.
#[must_use]
pub fn estimate_tokens(s: &str) -> usize {
    // `char` count rather than `len()` (bytes) so CJK text doesn't
    // double-count. `+3` then integer-divide-by-4 rounds up.
    if s.is_empty() {
        return 0;
    }
    s.chars().count().div_ceil(4)
}

/// Per-message overhead added by the chat-completion wire format
/// (role marker, JSON envelope, role-separator). 4 tokens is the
/// number OpenAI documents for `gpt-3.5-turbo`. We use it as a
/// reasonable upper bound across all providers.
const PER_MESSAGE_OVERHEAD: usize = 4;

/// Estimate the total tokens consumed by a sequence of messages,
/// including per-message wire-format overhead and tool-call payloads.
#[must_use]
pub fn estimate_message_tokens(messages: &[Message]) -> usize {
    let mut total = 0usize;
    for m in messages {
        total = total.saturating_add(PER_MESSAGE_OVERHEAD);
        total = total.saturating_add(estimate_tokens(&m.content));
        for tc in &m.tool_calls {
            total = total.saturating_add(estimate_tokens(&tc.name));
            total = total.saturating_add(estimate_tokens(&tc.arguments_json));
        }
        if let Some(id) = m.tool_call_id.as_ref() {
            total = total.saturating_add(estimate_tokens(id));
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, ToolCallSpec};

    #[test]
    fn empty_string_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn rounds_up_short_strings() {
        // 1, 2, 3, 4 chars all round to 1 token (conservative).
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("ab"), 1);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn cjk_does_not_double_count() {
        // 4 Chinese chars → 1 token (chars, not bytes).
        // If we measured bytes, 4×3 = 12 bytes → 3 tokens. We don't.
        let s = "你好世界";
        assert_eq!(s.chars().count(), 4);
        assert_eq!(estimate_tokens(s), 1);
    }

    #[test]
    fn message_overhead_applied() {
        let m = Message::user("");
        // Empty content + 4-token overhead.
        assert_eq!(estimate_message_tokens(&[m]), PER_MESSAGE_OVERHEAD);
    }

    #[test]
    fn message_aggregates_content_and_tool_calls() {
        let msg = Message {
            role: crate::types::Role::Assistant,
            content: "abcd".into(), // 1 token
            tool_calls: vec![ToolCallSpec {
                id: "call_1".into(),
                name: "execute".into(), // 2 tokens (6 chars → ceil(6/4)=2)
                arguments_json: "{\"x\":1}".into(), // 2 tokens (7 chars)
            }],
            tool_call_id: None,
        };
        // 4 (overhead) + 1 (content) + 2 (name) + 2 (args) = 9.
        assert_eq!(estimate_message_tokens(&[msg]), 9);
    }

    #[test]
    fn tool_role_includes_tool_call_id() {
        let m = Message::tool("call_xyz", "result");
        // 4 (overhead) + 2 (content "result" → 2) + 2 (id "call_xyz" → 2) = 8.
        assert_eq!(estimate_message_tokens(&[m]), 8);
    }

    #[test]
    fn many_messages_sum_overhead() {
        let msgs = vec![Message::user(""), Message::user(""), Message::user("")];
        assert_eq!(estimate_message_tokens(&msgs), 3 * PER_MESSAGE_OVERHEAD);
    }

    #[test]
    fn realistic_paragraph() {
        // 200-char paragraph → ceil(200/4) = 50 tokens of content,
        // plus 4 overhead = 54 total.
        let para = "x".repeat(200);
        let m = Message::user(para);
        assert_eq!(estimate_message_tokens(&[m]), 50 + PER_MESSAGE_OVERHEAD);
    }
}
