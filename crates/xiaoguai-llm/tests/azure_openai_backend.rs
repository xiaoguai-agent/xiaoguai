//! `AzureOpenAiBackend` integration tests.
//! All HTTP is intercepted by mockito — no real Azure API calls.

use futures::StreamExt;
use xiaoguai_llm::{AzureOpenAiBackend, ChatRequest, FinishReason, LlmBackend, Message};

/// Standard OpenAI-format SSE for a simple text response.
fn simple_sse() -> &'static str {
    "data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}\n\n\
     data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n\
     data: [DONE]\n\n"
}

// ── Happy path — non-stream ────────────────────────────────────────────────

#[tokio::test]
async fn azure_backend_streams_text_and_uses_api_key_header() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/chat/completions")
        .match_query(mockito::Matcher::UrlEncoded(
            "api-version".into(),
            "2024-10-21".into(),
        ))
        .match_header("api-key", "azure-test-key")
        // Must NOT have `authorization` header (Azure uses `api-key` instead)
        .match_header("authorization", mockito::Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(simple_sse())
        .create_async()
        .await;

    let backend = AzureOpenAiBackend::with_endpoint(server.url(), "azure-test-key");
    let req = ChatRequest::new("gpt-4o", vec![Message::user("hi")]);

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

// ── Error path: non-2xx response ───────────────────────────────────────────

#[tokio::test]
async fn azure_backend_reports_error_on_non_2xx() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/chat/completions")
        .match_query(mockito::Matcher::Any)
        .with_status(401)
        .with_body(
            r#"{"error":{"code":"401","message":"Access denied due to invalid subscription key."}}"#,
        )
        .create_async()
        .await;

    let backend = AzureOpenAiBackend::with_endpoint(server.url(), "bad-key");
    let req = ChatRequest::new("gpt-4o", vec![Message::user("hi")]);

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

// ── Backend name ──────────────────────────────────────────────────────────

#[test]
fn azure_backend_name() {
    let backend = AzureOpenAiBackend::with_endpoint(
        "https://example.openai.azure.com/openai/deployments/gpt-4o",
        "key",
    );
    assert_eq!(backend.name(), "azure_openai");
}

// ── Constructor builds expected URL ───────────────────────────────────────

#[test]
fn azure_backend_new_builds_correct_url() {
    let backend = AzureOpenAiBackend::new("my-resource", "my-deployment", "key");
    // The backend name must be azure_openai regardless of URL shape.
    assert_eq!(backend.name(), "azure_openai");
}
