//! `GeminiBackend` integration tests.
//! All HTTP is intercepted by mockito — no real Google API calls.

use futures::StreamExt;
use xiaoguai_llm::{ChatRequest, FinishReason, GeminiBackend, LlmBackend, Message, ToolSpec};

// ── SSE fixture builders ───────────────────────────────────────────────────

/// Minimal Gemini `alt=sse` streaming response for a text reply.
/// Gemini SSE wraps each JSON payload in `data:` lines.
fn simple_text_sse() -> String {
    let line = serde_json::json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{"text": "Hello"}]
            },
            "finishReason": "STOP"
        }]
    });
    format!("data: {line}\n\n")
}

/// Gemini SSE response carrying a function call.
fn function_call_sse() -> String {
    let line = serde_json::json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{
                    "functionCall": {
                        "name": "get_weather",
                        "args": {"city": "Tokyo"}
                    }
                }]
            },
            "finishReason": "STOP"
        }]
    });
    format!("data: {line}\n\n")
}

// ── Happy path ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn gemini_backend_streams_text() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock(
            "POST",
            mockito::Matcher::Regex(
                r"/v1beta/models/gemini-2\.0-flash:streamGenerateContent".to_string(),
            ),
        )
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_text_sse())
        .create_async()
        .await;

    let backend = GeminiBackend::with_base_url(server.url(), "test-api-key");
    let req = ChatRequest::new("gemini-2.0-flash", vec![Message::user("hi")]);

    let mut stream = backend.chat_stream(req).await.expect("stream");
    let mut collected = String::new();
    let mut saw_done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        collected.push_str(&chunk.delta);
        if chunk.done {
            saw_done = true;
        }
    }

    mock.assert_async().await;
    assert_eq!(collected, "Hello");
    assert!(saw_done, "stream must end with done=true");
}

// ── Function calling ───────────────────────────────────────────────────────

#[tokio::test]
async fn gemini_backend_emits_function_calls() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock(
            "POST",
            mockito::Matcher::Regex(
                r"/v1beta/models/gemini-2\.5-pro:streamGenerateContent".to_string(),
            ),
        )
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(function_call_sse())
        .create_async()
        .await;

    let backend = GeminiBackend::with_base_url(server.url(), "key");
    let tool = ToolSpec {
        name: "get_weather".to_string(),
        description: Some("Get weather for a city".to_string()),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    };
    let mut req = ChatRequest::new("gemini-2.5-pro", vec![Message::user("weather in Tokyo")]);
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
    assert_eq!(chunk.tool_calls.len(), 1);
    let tc = &chunk.tool_calls[0];
    assert_eq!(tc.name, "get_weather");
    assert!(
        tc.arguments_json.contains("Tokyo"),
        "args: {}",
        tc.arguments_json
    );
}

// ── SEC-04: API key in header, never in the URL ───────────────────────────

#[tokio::test]
async fn gemini_backend_sends_api_key_in_header_not_url() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock(
            "POST",
            mockito::Matcher::Regex(
                r"/v1beta/models/gemini-2\.0-flash:streamGenerateContent".to_string(),
            ),
        )
        // SEC-04: the key must arrive via the `x-goog-api-key` header…
        .match_header("x-goog-api-key", "secret-key")
        // …and the query string must be exactly `alt=sse` — no `key=` leak.
        .match_query(mockito::Matcher::Regex(r"^alt=sse$".to_string()))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_text_sse())
        .create_async()
        .await;

    let backend = GeminiBackend::with_base_url(server.url(), "secret-key");
    let req = ChatRequest::new("gemini-2.0-flash", vec![Message::user("hi")]);

    let mut stream = backend.chat_stream(req).await.expect("stream");
    while let Some(chunk) = stream.next().await {
        let _ = chunk.expect("chunk");
    }
    mock.assert_async().await;
}

// ── Error path: non-2xx HTTP ───────────────────────────────────────────────

#[tokio::test]
async fn gemini_backend_reports_error_on_non_2xx() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock(
            "POST",
            mockito::Matcher::Regex(r"/v1beta/models/.*:streamGenerateContent".to_string()),
        )
        .with_status(400)
        .with_body(
            r#"{"error":{"code":400,"message":"API key not valid","status":"INVALID_ARGUMENT"}}"#,
        )
        .create_async()
        .await;

    let backend = GeminiBackend::with_base_url(server.url(), "bad-key");
    let req = ChatRequest::new("gemini-2.0-flash", vec![Message::user("hi")]);

    match backend.chat_stream(req).await {
        Err(e) => {
            let msg = e.to_string();
            assert!(msg.contains("400"), "expected status in error, got: {msg}");
        }
        Ok(_) => panic!("expected error, got Ok"),
    }
}

// ── Multi-chunk streaming (two SSE data lines) ────────────────────────────

#[tokio::test]
async fn gemini_backend_assembles_multiple_chunks() {
    let chunk1 = serde_json::json!({
        "candidates": [{
            "content": {"role": "model", "parts": [{"text": "He"}]}
        }]
    });
    let chunk2 = serde_json::json!({
        "candidates": [{
            "content": {"role": "model", "parts": [{"text": "llo"}]},
            "finishReason": "STOP"
        }]
    });
    let body = format!("data: {chunk1}\n\ndata: {chunk2}\n\n");

    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock(
            "POST",
            mockito::Matcher::Regex(r"/v1beta/models/.*:streamGenerateContent".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(body)
        .create_async()
        .await;

    let backend = GeminiBackend::with_base_url(server.url(), "key");
    let req = ChatRequest::new("gemini-2.0-flash", vec![Message::user("hi")]);

    let mut stream = backend.chat_stream(req).await.expect("stream");
    let mut collected = String::new();
    let mut done_count = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        collected.push_str(&chunk.delta);
        if chunk.done {
            done_count += 1;
        }
    }
    assert_eq!(collected, "Hello");
    assert_eq!(done_count, 1, "exactly one done=true chunk expected");
}

// ── System prompt forwarded as `systemInstruction` ────────────────────────

#[tokio::test]
async fn gemini_backend_sends_system_as_system_instruction() {
    let mut server = mockito::Server::new_async().await;

    let mock = server
        .mock(
            "POST",
            mockito::Matcher::Regex(r"/v1beta/models/.*:streamGenerateContent".to_string()),
        )
        .match_body(mockito::Matcher::PartialJson(serde_json::json!({
            "systemInstruction": {
                "parts": [{"text": "Be concise."}]
            }
        })))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_text_sse())
        .create_async()
        .await;

    let backend = GeminiBackend::with_base_url(server.url(), "key");
    let req = ChatRequest::new(
        "gemini-2.0-flash",
        vec![Message::system("Be concise."), Message::user("Hello")],
    );
    let mut stream = backend.chat_stream(req).await.expect("stream");
    while let Some(c) = stream.next().await {
        let _ = c.expect("chunk");
    }
    mock.assert_async().await;
}

// ── Backend name ──────────────────────────────────────────────────────────

#[test]
fn gemini_backend_name() {
    let backend = GeminiBackend::new("key");
    assert_eq!(backend.name(), "gemini");
}

// ── `FinishReason::Stop` on STOP response ────────────────────────────────

#[tokio::test]
async fn gemini_backend_maps_stop_finish_reason() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock(
            "POST",
            mockito::Matcher::Regex(r"/v1beta/models/.*:streamGenerateContent".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_text_sse())
        .create_async()
        .await;

    let backend = GeminiBackend::with_base_url(server.url(), "key");
    let req = ChatRequest::new("gemini-2.0-flash", vec![Message::user("hi")]);

    let mut stream = backend.chat_stream(req).await.expect("stream");
    let mut finish = None;
    while let Some(c) = stream.next().await {
        let c = c.expect("chunk");
        if c.done {
            finish = c.finish_reason;
        }
    }
    assert_eq!(finish, Some(FinishReason::Stop));
}
