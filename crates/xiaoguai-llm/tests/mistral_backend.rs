//! `MistralBackend` integration tests.
//! All HTTP is intercepted by mockito — no real Mistral API calls.

use futures::StreamExt;
use xiaoguai_llm::{ChatRequest, FinishReason, LlmBackend, Message, MistralBackend, ToolSpec};

/// OpenAI-format SSE for a simple text reply (Mistral uses the same format).
fn simple_sse() -> &'static str {
    "data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}\n\n\
     data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n\
     data: [DONE]\n\n"
}

// ── Happy path — non-stream ────────────────────────────────────────────────

#[tokio::test]
async fn mistral_backend_streams_text() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_header("authorization", "Bearer mistral-test-key")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_sse())
        .create_async()
        .await;

    let backend = MistralBackend::with_base_url(server.url(), "mistral-test-key");
    let req = ChatRequest::new("mistral-large-latest", vec![Message::user("hi")]);

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

// ── Tool calling ──────────────────────────────────────────────────────────

#[tokio::test]
async fn mistral_backend_handles_tool_call_finish_reason() {
    let tool_sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"Paris\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n\
                    data: [DONE]\n\n";

    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(tool_sse)
        .create_async()
        .await;

    let backend = MistralBackend::with_base_url(server.url(), "key");
    let tool = ToolSpec {
        name: "get_weather".to_string(),
        description: Some("Get weather".to_string()),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    };
    let mut req = ChatRequest::new(
        "mistral-large-latest",
        vec![Message::user("weather in Paris")],
    );
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
    assert_eq!(chunk.tool_calls[0].name, "get_weather");
}

// ── Error path: non-2xx response ───────────────────────────────────────────

#[tokio::test]
async fn mistral_backend_reports_error_on_non_2xx() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(429)
        .with_body(r#"{"message":"Rate limit exceeded"}"#)
        .create_async()
        .await;

    let backend = MistralBackend::with_base_url(server.url(), "key");
    let req = ChatRequest::new("mistral-large-latest", vec![Message::user("hi")]);

    match backend.chat_stream(req).await {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("429"),
                "expected status in error message, got: {msg}"
            );
        }
        Ok(_) => panic!("expected error, got Ok"),
    }
}

// ── Backend name ──────────────────────────────────────────────────────────

#[test]
fn mistral_backend_name() {
    let backend = MistralBackend::new("key");
    assert_eq!(backend.name(), "mistral");
}
