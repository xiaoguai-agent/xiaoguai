//! Convert between transport-layer `xiaoguai_llm::Message` (what the agent
//! loop and providers speak) and persistence-layer `xiaoguai_types::Message`
//! (what the storage repos accept). The two live separately on purpose — the
//! LLM-side type matches the OpenAI wire shape, while the domain-side type
//! is a richer block list with IDs and timestamps suitable for replay.

use chrono::Utc;
use serde_json::Value as JsonValue;
use xiaoguai_llm::{Message as LlmMessage, Role as LlmRole};
use xiaoguai_types::{ContentBlock, Message as DomainMessage, MessageId, MessageRole, SessionId};

/// Build a domain message that can be persisted via `MessageRepository::append`.
/// IDs are freshly generated; the caller is responsible for ordering.
#[must_use]
pub fn llm_to_domain(session_id: &SessionId, m: &LlmMessage) -> DomainMessage {
    DomainMessage {
        id: MessageId::new(),
        session_id: session_id.clone(),
        role: map_role(m.role),
        content: build_blocks(m),
        created_at: Utc::now(),
    }
}

fn map_role(r: LlmRole) -> MessageRole {
    match r {
        LlmRole::System => MessageRole::System,
        LlmRole::User => MessageRole::User,
        LlmRole::Assistant => MessageRole::Assistant,
        LlmRole::Tool => MessageRole::Tool,
    }
}

fn build_blocks(m: &LlmMessage) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();

    if matches!(m.role, LlmRole::Tool) {
        // Tool role: the LLM `content` is the raw payload we injected. We
        // try to parse it as JSON for fidelity; if that fails, wrap as a
        // string. `is_error` is encoded inside the agent-emitted payload
        // (`{"error": ..., "text": ...}`) — we surface it explicitly so
        // downstream replay can render it cleanly.
        let (output, is_error) = parse_tool_payload(&m.content);
        let tool_call_id = m
            .tool_call_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        blocks.push(ContentBlock::ToolResult {
            tool_call_id,
            output,
            is_error,
        });
    } else {
        if !m.content.is_empty() {
            blocks.push(ContentBlock::Text {
                text: m.content.clone(),
            });
        }
        for tc in &m.tool_calls {
            let arguments = serde_json::from_str::<JsonValue>(&tc.arguments_json)
                .unwrap_or_else(|_| JsonValue::String(tc.arguments_json.clone()));
            blocks.push(ContentBlock::ToolCall {
                tool_call_id: tc.id.clone(),
                name: tc.name.clone(),
                arguments,
            });
        }
    }

    blocks
}

/// Inverse of `llm_to_domain` — rebuild an `LlmMessage` from a persisted
/// `DomainMessage`. Used when loading conversation history before the
/// next agent turn.
#[must_use]
pub fn domain_to_llm(m: &DomainMessage) -> LlmMessage {
    let role = match m.role {
        MessageRole::System => LlmRole::System,
        MessageRole::User => LlmRole::User,
        MessageRole::Assistant => LlmRole::Assistant,
        MessageRole::Tool => LlmRole::Tool,
    };

    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut tool_call_id: Option<String> = None;

    for block in &m.content {
        match block {
            ContentBlock::Text { text } => content.push_str(text),
            ContentBlock::ToolCall {
                tool_call_id: id,
                name,
                arguments,
            } => {
                tool_calls.push(xiaoguai_llm::ToolCallSpec {
                    id: id.clone(),
                    name: name.clone(),
                    arguments_json: arguments.to_string(),
                });
            }
            ContentBlock::ToolResult {
                tool_call_id: id,
                output,
                ..
            } => {
                // The agent loop expects `Role::Tool` content as a raw string;
                // re-serialise the JSON output to preserve fidelity.
                tool_call_id = Some(id.clone());
                content.push_str(&output.to_string());
            }
            ContentBlock::Citation { .. } => {
                // v0.9.3: citations are visible to the UI but invisible to
                // the LLM next-turn. The retrieval content already lived
                // in a `Text` block (or a tool result) when the model
                // produced the assistant turn; the citation is meta. Skip
                // here so we don't feed `[1] file:///x.md` back as text.
            }
        }
    }

    LlmMessage {
        role,
        content,
        tool_calls,
        tool_call_id,
    }
}

fn parse_tool_payload(raw: &str) -> (JsonValue, bool) {
    match serde_json::from_str::<JsonValue>(raw) {
        Ok(val) => {
            let is_error = val.get("error").is_some();
            (val, is_error)
        }
        Err(_) => (JsonValue::String(raw.to_string()), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xiaoguai_llm::ToolCallSpec;

    #[test]
    fn text_only_assistant_becomes_single_text_block() {
        let session = SessionId::from("sess_x".to_string());
        let m = LlmMessage::assistant("hello");
        let d = llm_to_domain(&session, &m);
        assert_eq!(d.content.len(), 1);
        assert!(matches!(d.content[0], ContentBlock::Text { .. }));
        assert_eq!(d.role, MessageRole::Assistant);
    }

    #[test]
    fn assistant_with_tool_calls_emits_text_then_call_blocks() {
        let session = SessionId::from("sess_x".to_string());
        let m = LlmMessage {
            role: LlmRole::Assistant,
            content: "thinking...".into(),
            tool_calls: vec![ToolCallSpec {
                id: "c1".into(),
                name: "search".into(),
                arguments_json: r#"{"q":"x"}"#.into(),
            }],
            tool_call_id: None,
        };
        let d = llm_to_domain(&session, &m);
        assert_eq!(d.content.len(), 2);
        assert!(matches!(d.content[0], ContentBlock::Text { .. }));
        match &d.content[1] {
            ContentBlock::ToolCall {
                name, arguments, ..
            } => {
                assert_eq!(name, "search");
                assert_eq!(arguments["q"], "x");
            }
            _ => panic!("expected tool call block"),
        }
    }

    #[test]
    fn assistant_tool_calls_only_skips_empty_text_block() {
        let session = SessionId::from("sess_x".to_string());
        let m = LlmMessage::assistant_tool_calls(vec![ToolCallSpec {
            id: "c1".into(),
            name: "n".into(),
            arguments_json: "{}".into(),
        }]);
        let d = llm_to_domain(&session, &m);
        assert_eq!(d.content.len(), 1);
        assert!(matches!(d.content[0], ContentBlock::ToolCall { .. }));
    }

    #[test]
    fn tool_role_with_error_payload_marks_is_error_true() {
        let session = SessionId::from("sess_x".to_string());
        let m = LlmMessage::tool("c1", r#"{"error":"boom","text":""}"#);
        let d = llm_to_domain(&session, &m);
        assert_eq!(d.role, MessageRole::Tool);
        match &d.content[0] {
            ContentBlock::ToolResult {
                tool_call_id,
                is_error,
                output,
            } => {
                assert_eq!(tool_call_id, "c1");
                assert!(*is_error);
                assert_eq!(output["error"], "boom");
            }
            _ => panic!("expected tool result"),
        }
    }

    #[test]
    fn tool_role_with_plain_text_is_not_error() {
        let session = SessionId::from("sess_x".to_string());
        let m = LlmMessage::tool("c1", "raw output");
        let d = llm_to_domain(&session, &m);
        match &d.content[0] {
            ContentBlock::ToolResult {
                is_error, output, ..
            } => {
                assert!(!*is_error);
                assert_eq!(output.as_str().unwrap(), "raw output");
            }
            _ => panic!("expected tool result"),
        }
    }
}
