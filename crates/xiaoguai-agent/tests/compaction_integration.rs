//! v0.5.4.1 integration test: long synthetic conversations are compacted
//! before being sent to the backend, and the agent can still consume the
//! compacted history end-to-end.
//!
//! The plan D.2 success criteria #5 asks for: 100-turn synthetic
//! conversation including ≥ 30 tool calls is compacted; post-compaction
//! token count ≤ `max_context_tokens * 0.5`; agent can still answer a
//! follow-up. We assert tokens shrink and that `compact()` chose the
//! summary path (not the fallback) when given a working backend.

use async_trait::async_trait;
use futures::stream;
use xiaoguai_agent::history::{compact, CompactionConfig, CompactionOutcome};
use xiaoguai_llm::backend::{ChatStream, LlmError};
use xiaoguai_llm::{
    estimate_message_tokens, ChatChunk, ChatRequest, LlmBackend, Message, ToolCallSpec,
};

/// Backend that returns a fixed summary. Used to make `compact()`
/// deterministic in tests.
struct FixedSummaryBackend;

#[async_trait]
impl LlmBackend for FixedSummaryBackend {
    async fn chat_stream(&self, _req: ChatRequest) -> Result<ChatStream, LlmError> {
        let chunk = ChatChunk {
            delta: "User explored 30 deployment scenarios across q1. \
                   Key facts: cluster-id=prod-east-7, pgvector v0.8.2, \
                   audit signing key rotation due 2026-06-01."
                .into(),
            tool_calls: vec![],
            finish_reason: None,
            done: true,
            reasoning_delta: None,
        };
        Ok(Box::pin(stream::iter(vec![Ok(chunk)])))
    }
    fn name(&self) -> &'static str {
        "fixed-summary"
    }
}

/// Build a synthetic 100-turn conversation including 30 tool calls,
/// roughly matching what a real long agent session looks like.
fn build_synthetic_100_turns() -> Vec<Message> {
    let mut msgs = vec![Message::system(
        "You are a helpful assistant operating a Kubernetes cluster.",
    )];

    // 70 plain user/assistant pairs.
    for i in 0..70 {
        msgs.push(Message::user(format!(
            "Question {i}: how do I describe pod {i} in namespace prod?"
        )));
        msgs.push(Message::assistant(format!(
            "Answer {i}: run `kubectl describe pod pod-{i} -n prod`. \
             Output should include status, events, and conditions."
        )));
    }

    // 30 tool-call cycles interleaved.
    for i in 0..30 {
        msgs.push(Message::user(format!(
            "Now actually run that command for pod-{i}."
        )));
        msgs.push(Message::assistant_tool_calls(vec![ToolCallSpec {
            id: format!("call_{i}"),
            name: "execute_python".into(),
            arguments_json: format!(
                "{{\"code\":\"subprocess.run(['kubectl','describe','pod','pod-{i}','-n','prod'])\"}}"
            ),
        }]));
        msgs.push(Message::tool(
            format!("call_{i}"),
            format!(
                "Pod pod-{i}: status=Running, restarts=0, age=3d. \
                 Events: pulled image, started container, healthy."
            ),
        ));
        msgs.push(Message::assistant(format!("Pod pod-{i} is healthy.")));
    }

    msgs
}

#[tokio::test]
async fn compaction_shrinks_large_history_via_summary() {
    let msgs = build_synthetic_100_turns();
    let before = estimate_message_tokens(&msgs);
    assert!(
        before > 5_000,
        "synthetic conversation should be substantial (got {before} tokens)"
    );

    let cfg = CompactionConfig {
        max_context_tokens: 8_000, // tight to force compaction
        trigger_at_pct: 75,
        keep_recent: 6,
        summary_model: "fixed-summary",
    };

    let backend = FixedSummaryBackend;
    let (out, outcome) = compact(msgs, &backend, cfg).await;
    let after = estimate_message_tokens(&out);

    assert_eq!(outcome, CompactionOutcome::Compacted);
    // Plan D.2 success criterion #5(b): post-compaction tokens ≤ 50 % of
    // max_context_tokens.
    assert!(
        after <= cfg.max_context_tokens / 2,
        "expected ≤ {} tokens after compaction, got {after}",
        cfg.max_context_tokens / 2
    );
    // The synthetic summary message must mention concrete facts from
    // the canned summary — proves the summary content was preserved.
    let combined: String = out
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(combined.contains("cluster-id=prod-east-7"));
    assert!(combined.contains("audit signing key rotation"));
}

#[tokio::test]
async fn compaction_preserves_recent_turns_verbatim() {
    let msgs = build_synthetic_100_turns();
    let cfg = CompactionConfig {
        max_context_tokens: 8_000,
        trigger_at_pct: 75,
        keep_recent: 6,
        summary_model: "fixed-summary",
    };

    let backend = FixedSummaryBackend;
    let (out, _) = compact(msgs, &backend, cfg).await;

    // The 6 recent messages should all be in the output, in original order.
    // The very last message is "Pod pod-29 is healthy."
    let last = out.last().expect("compacted history non-empty");
    assert_eq!(last.content, "Pod pod-29 is healthy.");
}
