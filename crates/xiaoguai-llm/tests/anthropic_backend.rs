//! `AnthropicBackend` integration tests.
//! All HTTP is intercepted by mockito — no real Anthropic API calls.

use futures::StreamExt;
use xiaoguai_llm::{
    AnthropicBackend, ChatRequest, FinishReason, LlmBackend, Message, ToolCallSpec, ToolSpec,
};

// ── Happy path — non-stream (simulated via streaming endpoint) ─────────────

/// Anthropic streaming SSE for a simple text reply.
///
/// Event sequence: `message_start` → `content_block_start` (text) →
/// `content_block_delta` (`text_delta`) × 2 → `content_block_stop` →
/// `message_delta` (`stop_reason=end_turn`) → `message_stop`.
fn simple_text_sse() -> &'static str {
    "event: message_start\n\
     data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"claude-sonnet-4-6\"}}\n\n\
     event: content_block_start\n\
     data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
     event: content_block_delta\n\
     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"He\"}}\n\n\
     event: content_block_delta\n\
     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"llo\"}}\n\n\
     event: content_block_stop\n\
     data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
     event: message_delta\n\
     data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n\
     event: message_stop\n\
     data: {\"type\":\"message_stop\"}\n\n"
}

#[tokio::test]
async fn anthropic_backend_streams_text() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/messages")
        .match_header("x-api-key", "test-key")
        .match_header("anthropic-version", "2023-06-01")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_text_sse())
        .create_async()
        .await;

    let backend = AnthropicBackend::new(server.url(), "test-key");
    let req = ChatRequest::new("claude-sonnet-4-6", vec![Message::user("hi")]);

    let mut stream = backend.chat_stream(req).await.expect("stream");
    let mut collected = String::new();
    let mut saw_done = false;
    let mut finish_reason = None;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        collected.push_str(&chunk.delta);
        if chunk.done {
            saw_done = true;
            finish_reason = chunk.finish_reason;
        }
    }

    mock.assert_async().await;
    assert_eq!(collected, "Hello");
    assert!(saw_done, "stream must end with done=true");
    assert_eq!(finish_reason, Some(FinishReason::Stop));
}

// ── Tool use ──────────────────────────────────────────────────────────────

/// Anthropic streaming SSE for a tool-use response.
fn tool_use_sse() -> &'static str {
    "event: message_start\n\
     data: {\"type\":\"message_start\",\"message\":{\"id\":\"m2\",\"model\":\"claude-sonnet-4-6\"}}\n\n\
     event: content_block_start\n\
     data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_abc\",\"name\":\"get_weather\"}}\n\n\
     event: content_block_delta\n\
     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\"}}\n\n\
     event: content_block_delta\n\
     data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"Tokyo\\\"}\"}}\n\n\
     event: content_block_stop\n\
     data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
     event: message_delta\n\
     data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n\
     event: message_stop\n\
     data: {\"type\":\"message_stop\"}\n\n"
}

#[tokio::test]
async fn anthropic_backend_assembles_tool_calls() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/messages")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(tool_use_sse())
        .create_async()
        .await;

    let backend = AnthropicBackend::new(server.url(), "key");
    let tool = ToolSpec {
        name: "get_weather".to_string(),
        description: Some("Get weather".to_string()),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    };
    let mut req = ChatRequest::new("claude-sonnet-4-6", vec![Message::user("weather in Tokyo")]);
    req.tools = vec![tool];

    let mut stream = backend.chat_stream(req).await.expect("stream");
    let mut final_chunk = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        if chunk.done {
            final_chunk = Some(chunk);
        }
    }

    let chunk = final_chunk.expect("expected a done chunk");
    assert_eq!(chunk.finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(chunk.tool_calls.len(), 1);
    let tc = &chunk.tool_calls[0];
    assert_eq!(tc.id, "call_abc");
    assert_eq!(tc.name, "get_weather");
    // Arguments should be the concatenated JSON
    assert!(
        tc.arguments_json.contains("Tokyo"),
        "args: {}",
        tc.arguments_json
    );
}

// ── Error case: non-2xx HTTP status ──────────────────────────────────────

#[tokio::test]
async fn anthropic_backend_reports_error_on_non_2xx() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/messages")
        .with_status(401)
        .with_body(r#"{"type":"error","error":{"type":"authentication_error","message":"invalid api key"}}"#)
        .create_async()
        .await;

    let backend = AnthropicBackend::new(server.url(), "bad-key");
    let req = ChatRequest::new("claude-sonnet-4-6", vec![Message::user("hi")]);

    match backend.chat_stream(req).await {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("401"),
                "expected status in error message, got: {msg}"
            );
        }
        Ok(_) => panic!("expected error, got Ok"),
    }
}

// ── Error event in SSE stream ─────────────────────────────────────────────

#[tokio::test]
async fn anthropic_backend_surfaces_sse_error_event() {
    let body = "event: error\n\
                data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"API overloaded\"}}\n\n";

    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/messages")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(body)
        .create_async()
        .await;

    let backend = AnthropicBackend::new(server.url(), "key");
    let req = ChatRequest::new("claude-sonnet-4-6", vec![Message::user("hi")]);

    let mut stream = backend.chat_stream(req).await.expect("stream opened");
    let mut got_error = false;
    while let Some(item) = stream.next().await {
        if item.is_err() {
            got_error = true;
        }
    }
    assert!(
        got_error,
        "expected an error chunk from the SSE error event"
    );
}

// ── Backend name ──────────────────────────────────────────────────────────

#[test]
fn anthropic_backend_name() {
    let backend = AnthropicBackend::new("https://api.anthropic.com", "key");
    assert_eq!(backend.name(), "anthropic");
}

// ── System prompt and multi-turn conversation ─────────────────────────────

#[tokio::test]
async fn anthropic_backend_sends_system_as_top_level_field() {
    let mut server = mockito::Server::new_async().await;

    // Verify the request body contains a top-level `system` field.
    let mock = server
        .mock("POST", "/v1/messages")
        .match_body(mockito::Matcher::PartialJson(serde_json::json!({
            "system": "You are a helpful assistant."
        })))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_text_sse())
        .create_async()
        .await;

    let backend = AnthropicBackend::new(server.url(), "key");
    let req = ChatRequest::new(
        "claude-haiku-4-5",
        vec![
            Message::system("You are a helpful assistant."),
            Message::user("Hello"),
        ],
    );
    let mut stream = backend.chat_stream(req).await.expect("stream");
    while let Some(c) = stream.next().await {
        let _ = c.expect("chunk");
    }
    mock.assert_async().await;
}

// ── Tool-use round-trip: assert tool result encodes as user content block ─

#[tokio::test]
async fn anthropic_backend_encodes_tool_result_as_user_block() {
    let mut server = mockito::Server::new_async().await;

    // The tool result message should be encoded as a user turn with a
    // `tool_result` content block.
    let mock = server
        .mock("POST", "/v1/messages")
        .match_body(mockito::Matcher::PartialJson(serde_json::json!({
            "messages": [
                {"role": "user", "content": "What's the weather?"},
                {
                    "role": "assistant",
                    "content": [{"type": "tool_use", "id": "call_1", "name": "get_weather"}]
                },
                {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "call_1"}]
                }
            ]
        })))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_text_sse())
        .create_async()
        .await;

    let backend = AnthropicBackend::new(server.url(), "key");
    let req = ChatRequest::new(
        "claude-sonnet-4-6",
        vec![
            Message::user("What's the weather?"),
            Message::assistant_tool_calls(vec![ToolCallSpec {
                id: "call_1".to_string(),
                name: "get_weather".to_string(),
                arguments_json: "{}".to_string(),
            }]),
            Message::tool("call_1", "Sunny, 25°C"),
        ],
    );
    let mut stream = backend.chat_stream(req).await.expect("stream");
    while let Some(c) = stream.next().await {
        let _ = c.expect("chunk");
    }
    mock.assert_async().await;
}
