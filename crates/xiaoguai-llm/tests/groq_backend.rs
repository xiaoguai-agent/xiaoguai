//! `GroqBackend` integration tests.
//! All HTTP is intercepted by mockito — no real Groq API calls.

use futures::StreamExt;
use xiaoguai_llm::{ChatRequest, FinishReason, GroqBackend, LlmBackend, Message};

/// OpenAI-format SSE for a simple text reply (Groq uses the same format).
fn simple_sse() -> &'static str {
    "data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}\n\n\
     data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n\
     data: [DONE]\n\n"
}

// ── Happy path — non-stream ────────────────────────────────────────────────

#[tokio::test]
async fn groq_backend_streams_text() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_header("authorization", "Bearer groq-test-key")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_sse())
        .create_async()
        .await;

    let backend = GroqBackend::with_base_url(server.url(), "groq-test-key");
    let req = ChatRequest::new("llama-3.3-70b-versatile", vec![Message::user("hi")]);

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

// ── Groq closes stream with finish_reason (no [DONE] sentinel) ────────────

#[tokio::test]
async fn groq_backend_handles_finish_reason_without_done_sentinel() {
    let body = "\
data: {\"choices\":[{\"delta\":{\"content\":\"Fast\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";

    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(body)
        .create_async()
        .await;

    let backend = GroqBackend::with_base_url(server.url(), "key");
    let req = ChatRequest::new("llama-3.3-70b-versatile", vec![Message::user("speed")]);
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
    assert_eq!(collected, "Fast");
    assert_eq!(done_count, 1, "exactly one done=true chunk expected");
}

// ── Error path: non-2xx response ───────────────────────────────────────────

#[tokio::test]
async fn groq_backend_reports_error_on_non_2xx() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(503)
        .with_body(r#"{"error":{"message":"Service unavailable"}}"#)
        .create_async()
        .await;

    let backend = GroqBackend::with_base_url(server.url(), "key");
    let req = ChatRequest::new("llama-3.3-70b-versatile", vec![]);

    match backend.chat_stream(req).await {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("503"),
                "expected status in error message, got: {msg}"
            );
        }
        Ok(_) => panic!("expected error, got Ok"),
    }
}

// ── Backend name ──────────────────────────────────────────────────────────

#[test]
fn groq_backend_name() {
    let backend = GroqBackend::new("key");
    assert_eq!(backend.name(), "groq");
}
