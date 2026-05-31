//! Conversation-history management.
//!
//! Two strategies live here, both preserving the `System` message at the
//! front and the `tool_call_id` pairing invariant in the tail:
//!
//! - [`slide`] — fixed-window oldest-drop. Cheap, deterministic, no LLM
//!   call. Used by default.
//! - [`compact`] — LLM-summarisation of older turns, keeping recent N
//!   turns verbatim. Used when the conversation crosses a token-budget
//!   threshold. Falls back to [`slide`] on summariser failure.
//!
//! v0.5.4 shipped [`slide`] only; v0.5.4.1 adds [`compact`] for long
//! local-model sessions (see `docs/runbooks/compaction.md`).

use futures::StreamExt;
use tracing::warn;
use xiaoguai_llm::{estimate_message_tokens, ChatRequest, LlmBackend, Message, Role, ToolChoice};

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

/// Compaction tuning knobs. Default values target a 32k-context local
/// model (e.g. `qwen2.5-coder`) with a 25 % headroom for tool definitions
/// and the next turn's request/response.
#[derive(Debug, Clone, Copy)]
pub struct CompactionConfig {
    /// Hard ceiling on tokens the model can accept. Default 30 000.
    pub max_context_tokens: usize,
    /// Percentage of `max_context_tokens` at which compaction kicks in.
    /// Default 75.
    pub trigger_at_pct: u32,
    /// How many *recent* non-system messages to keep verbatim.
    /// Default 6 (≈ 3 user/assistant exchanges).
    pub keep_recent: usize,
    /// Model identifier passed to the backend when summarising.
    /// Default `qwen2.5-coder`.
    pub summary_model: &'static str,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 30_000,
            trigger_at_pct: 75,
            keep_recent: 6,
            summary_model: "qwen2.5-coder",
        }
    }
}

impl CompactionConfig {
    /// Threshold in *tokens* at which the agent loop should call
    /// [`compact`].
    #[must_use]
    pub fn trigger_threshold(&self) -> usize {
        self.max_context_tokens
            .saturating_mul(self.trigger_at_pct as usize)
            / 100
    }
}

/// Outcome of a compaction attempt.
///
/// `Compacted` means an LLM summary was generated and substituted for the
/// older head of the conversation. `FellBack` means the summariser was
/// unavailable / failed / returned empty, and we used `slide` to bound
/// the history instead. `NoOp` means the conversation already fits and
/// nothing was changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionOutcome {
    Compacted,
    FellBack,
    NoOp,
}

/// LLM-driven history compaction with a slide-window fallback.
///
/// Algorithm:
/// 1. Partition `messages` into `system` and `tail`.
/// 2. If `tail.len() <= cfg.keep_recent`, return the input unchanged
///    ([`CompactionOutcome::NoOp`]).
/// 3. Walk the split boundary back to avoid cutting between an assistant
///    `tool_calls` message and its [`Role::Tool`] reply — that would
///    create a dangling `tool_call_id` reference.
/// 4. Render the older head as plain text and ask `backend` to summarise
///    it.
/// 5. On success: replace the head with one synthetic `System` message
///    carrying the summary, then the preserved tail.
/// 6. On failure (network, empty summary, timeout): emit a `warn!` and
///    fall back to [`slide`], keeping `cfg.keep_recent + 1` tail entries.
///
/// Returns `(compacted_messages, outcome)`.
///
/// # Errors
/// This function never returns `Err`; transport / provider errors are
/// absorbed into the [`CompactionOutcome::FellBack`] path so the agent
/// loop can continue without re-trying.
pub async fn compact(
    messages: Vec<Message>,
    backend: &dyn LlmBackend,
    cfg: CompactionConfig,
) -> (Vec<Message>, CompactionOutcome) {
    let (system, tail): (Vec<_>, Vec<_>) = messages
        .into_iter()
        .partition(|m| matches!(m.role, Role::System));

    if tail.len() <= cfg.keep_recent {
        let mut out = system;
        out.extend(tail);
        return (out, CompactionOutcome::NoOp);
    }

    // Split: head = older messages we'll summarise; recent = kept verbatim.
    let split = tail.len() - cfg.keep_recent;
    let split = walk_split_back_past_tool_pairs(&tail, split);

    let head: Vec<Message> = tail[..split].to_vec();
    let recent: Vec<Message> = tail[split..].to_vec();

    // Edge case: walking the split back consumed everything.
    if head.is_empty() {
        let mut out = system;
        out.extend(recent);
        return (out, CompactionOutcome::NoOp);
    }

    match summarise(&head, backend, cfg.summary_model).await {
        Ok(summary) if !summary.trim().is_empty() => {
            let mut out = system;
            out.push(Message::system(format!(
                "[Compacted summary of {} earlier messages]\n\n{}",
                head.len(),
                summary
            )));
            // Strip leading Tool messages from the recent half — same
            // invariant as slide().
            let mut recent = recent;
            while matches!(recent.first().map(|m| m.role), Some(Role::Tool)) {
                recent.remove(0);
            }
            out.extend(recent);
            (out, CompactionOutcome::Compacted)
        }
        Ok(_) => {
            warn!(
                target: "xiaoguai_agent::history",
                "compaction summary empty; falling back to slide"
            );
            let mut all = system;
            all.extend(tail);
            (slide(all, cfg.keep_recent + 1), CompactionOutcome::FellBack)
        }
        Err(e) => {
            warn!(
                target: "xiaoguai_agent::history",
                error = %e,
                "compaction summariser failed; falling back to slide"
            );
            let mut all = system;
            all.extend(tail);
            (slide(all, cfg.keep_recent + 1), CompactionOutcome::FellBack)
        }
    }
}

/// Convenience: same as [`compact`] but ignores the outcome. Used in
/// places where the caller only needs the messages.
pub async fn compact_messages(
    messages: Vec<Message>,
    backend: &dyn LlmBackend,
    cfg: CompactionConfig,
) -> Vec<Message> {
    compact(messages, backend, cfg).await.0
}

/// Decide whether the message list exceeds the compaction trigger
/// threshold.
#[must_use]
pub fn should_compact(messages: &[Message], cfg: CompactionConfig) -> bool {
    estimate_message_tokens(messages) > cfg.trigger_threshold()
}

/// Walk `split` backwards if it would land between an assistant message
/// carrying `tool_calls` and the matching `Role::Tool` reply(ies). The
/// invariant: every retained `Tool` message must have its issuer
/// assistant message also retained.
fn walk_split_back_past_tool_pairs(tail: &[Message], mut split: usize) -> usize {
    while split > 0 && split < tail.len() {
        // If the message at `split` is a Tool, it belongs to an
        // assistant tool_calls message earlier; pull the boundary back so
        // both end up on the "recent" side.
        if matches!(tail[split].role, Role::Tool) {
            split -= 1;
            continue;
        }
        // If the message immediately before the boundary is an assistant
        // that emitted tool_calls, and the messages right after the
        // boundary are its Tool replies, we'd be cutting them apart —
        // pull boundary back so the assistant joins the recent side too.
        let prev = &tail[split - 1];
        let next_is_tool_reply = tail
            .get(split)
            .is_some_and(|m| matches!(m.role, Role::Tool));
        if matches!(prev.role, Role::Assistant) && !prev.tool_calls.is_empty() && next_is_tool_reply
        {
            split -= 1;
            continue;
        }
        break;
    }
    split
}

async fn summarise(
    head: &[Message],
    backend: &dyn LlmBackend,
    model: &str,
) -> Result<String, xiaoguai_llm::LlmError> {
    let rendered = render_for_summary(head);
    let summary_messages = vec![
        Message::system(
            "You summarise an agent's earlier conversation so it can be \
             dropped from the live context window. Produce a dense \
             plain-text summary in 500 tokens or fewer. Keep concrete \
             facts (names, IDs, file paths, error codes, decisions). \
             Drop pleasantries and commentary. Do NOT invent facts \
             not present in the input.",
        ),
        Message::user(format!("Summarise:\n\n{rendered}")),
    ];

    let mut req = ChatRequest::new(model.to_string(), summary_messages);
    req.temperature = Some(0.0);
    req.max_tokens = Some(800);
    req.tool_choice = ToolChoice::None;

    let mut stream = backend.chat_stream(req).await?;
    let mut out = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        out.push_str(&chunk.delta);
    }
    Ok(out)
}

fn render_for_summary(head: &[Message]) -> String {
    let mut s = String::new();
    for m in head {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool_result",
        };
        s.push_str("== ");
        s.push_str(role);
        s.push_str(" ==\n");
        if !m.content.is_empty() {
            s.push_str(&m.content);
            s.push('\n');
        }
        for tc in &m.tool_calls {
            s.push_str(&format!(
                "[tool_call name={} args={}]\n",
                tc.name, tc.arguments_json
            ));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use xiaoguai_llm::backend::{ChatStream, LlmError};
    use xiaoguai_llm::{ChatChunk, ChatRequest, ToolCallSpec};

    // --- slide tests (unchanged from v0.5.4) -------------------------------

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
        assert_eq!(out.len(), 4);
        assert!(matches!(out[0].role, Role::System));
        assert_eq!(out[1].content, "mid");
        assert_eq!(out[3].content, "new");
    }

    #[test]
    fn drops_dangling_tool_message_at_new_head() {
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
        let out = slide(msgs, 2);
        assert_eq!(out.len(), 3);
        assert!(!matches!(out[1].role, Role::Tool));

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

    // --- compact tests -----------------------------------------------------

    /// Mock backend that returns a canned summary string.
    struct CannedSummaryBackend(String);

    #[async_trait]
    impl LlmBackend for CannedSummaryBackend {
        async fn chat_stream(&self, _req: ChatRequest) -> Result<ChatStream, LlmError> {
            let chunk = ChatChunk {
                delta: self.0.clone(),
                tool_calls: vec![],
                finish_reason: None,
                done: true,
                reasoning_delta: None,
            };
            let stream = stream::iter(vec![Ok(chunk)]);
            Ok(Box::pin(stream))
        }
        fn name(&self) -> &'static str {
            "canned-summary"
        }
    }

    /// Mock backend that always fails — exercises the fallback path.
    struct FailingBackend;

    #[async_trait]
    impl LlmBackend for FailingBackend {
        async fn chat_stream(&self, _req: ChatRequest) -> Result<ChatStream, LlmError> {
            Err(LlmError::Network("simulated".into()))
        }
        fn name(&self) -> &'static str {
            "failing"
        }
    }

    fn make_long_conversation(n_turns: usize) -> Vec<Message> {
        let mut msgs = vec![Message::system("you are helpful")];
        for i in 0..n_turns {
            msgs.push(Message::user(format!("question {i}")));
            msgs.push(Message::assistant(format!("answer {i}")));
        }
        msgs
    }

    #[tokio::test]
    async fn compact_noop_when_under_keep_recent() {
        let msgs = make_long_conversation(2); // 1 system + 4 messages
        let cfg = CompactionConfig {
            keep_recent: 6,
            ..Default::default()
        };
        let backend = CannedSummaryBackend("should-not-be-called".into());
        let (out, outcome) = compact(msgs.clone(), &backend, cfg).await;
        assert_eq!(outcome, CompactionOutcome::NoOp);
        assert_eq!(out.len(), msgs.len());
    }

    #[tokio::test]
    async fn compact_replaces_head_with_summary() {
        let msgs = make_long_conversation(20); // 1 system + 40 messages
        let cfg = CompactionConfig {
            keep_recent: 6,
            ..Default::default()
        };
        let backend = CannedSummaryBackend("Earlier the user asked about 20 things.".into());
        let (out, outcome) = compact(msgs, &backend, cfg).await;
        assert_eq!(outcome, CompactionOutcome::Compacted);
        // 1 original system + 1 synthetic summary system + 6 recent = 8.
        assert_eq!(out.len(), 8);
        assert!(matches!(out[0].role, Role::System));
        assert!(matches!(out[1].role, Role::System));
        assert!(out[1].content.contains("Compacted summary"));
        assert!(out[1]
            .content
            .contains("Earlier the user asked about 20 things."));
        // The last 6 messages are the recent verbatim trio of pairs.
        assert_eq!(out[2].content, "question 17");
        assert_eq!(out[7].content, "answer 19");
    }

    #[tokio::test]
    async fn compact_falls_back_to_slide_on_backend_error() {
        let msgs = make_long_conversation(20);
        let cfg = CompactionConfig {
            keep_recent: 6,
            ..Default::default()
        };
        let backend = FailingBackend;
        let (out, outcome) = compact(msgs, &backend, cfg).await;
        assert_eq!(outcome, CompactionOutcome::FellBack);
        // Slide with window keep_recent + 1 = 7. 1 system + 7 tail.
        assert_eq!(out.len(), 8);
        assert!(matches!(out[0].role, Role::System));
        // No synthetic summary line.
        assert!(!out[1].content.contains("Compacted summary"));
    }

    #[tokio::test]
    async fn compact_preserves_tool_pairing() {
        // Build a conversation where a tool_calls / tool reply lands at
        // what would naturally be the split boundary.
        let mut msgs = vec![Message::system("s")];
        // 10 pairs to make sure we cross keep_recent=6.
        for i in 0..5 {
            msgs.push(Message::user(format!("u{i}")));
            msgs.push(Message::assistant(format!("a{i}")));
        }
        // Insert a tool-calling assistant + reply right before the
        // would-be split.
        msgs.push(Message::assistant_tool_calls(vec![ToolCallSpec {
            id: "call_x".into(),
            name: "look_up".into(),
            arguments_json: "{}".into(),
        }]));
        msgs.push(Message::tool("call_x", "the answer is 42"));
        // Then more recent pairs.
        for i in 5..10 {
            msgs.push(Message::user(format!("u{i}")));
            msgs.push(Message::assistant(format!("a{i}")));
        }

        let cfg = CompactionConfig {
            keep_recent: 6,
            ..Default::default()
        };
        let backend = CannedSummaryBackend("ok".into());
        let (out, outcome) = compact(msgs, &backend, cfg).await;
        assert_eq!(outcome, CompactionOutcome::Compacted);
        // No Tool message should appear without a preceding assistant
        // tool_calls in the same retained set (synthetic system summary
        // doesn't qualify, so any Tool at index ≥ 2 must follow an
        // assistant with non-empty tool_calls).
        for i in 0..out.len() {
            if matches!(out[i].role, Role::Tool) {
                assert!(i > 0, "Tool can't be the first message");
                let prev = &out[i - 1];
                assert!(
                    matches!(prev.role, Role::Assistant) && !prev.tool_calls.is_empty(),
                    "dangling Tool at index {i}"
                );
            }
        }
    }

    #[test]
    fn should_compact_threshold() {
        let cfg = CompactionConfig {
            max_context_tokens: 100,
            trigger_at_pct: 75,
            ..Default::default()
        };
        assert_eq!(cfg.trigger_threshold(), 75);
        // 1 message with empty content = 4 tokens overhead → below 75.
        assert!(!should_compact(&[Message::user("")], cfg));
        // A long string forces over the threshold.
        let big = Message::user("x".repeat(400)); // ≈ 100 + 4 overhead
        assert!(should_compact(&[big], cfg));
    }
}
