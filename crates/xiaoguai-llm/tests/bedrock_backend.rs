//! `BedrockBackend` integration tests.
//! All HTTP is intercepted by mockito — no real AWS API calls.
//!
//! For Bedrock, mockito mocks the HTTP layer. The `SigV4` signing itself is
//! tested separately in the unit tests inside `bedrock.rs` using known AWS
//! test vectors.
//!
//! The mockito server accepts any `Authorization` header (i.e., we do not
//! assert the full `SigV4` signature value in integration tests because the
//! signature changes every second via `x-amz-date`). We instead verify:
//!   1. The POST is made to the correct path for the given model ID.
//!   2. Text chunks are assembled correctly.
//!   3. Non-2xx responses produce `LlmError::Provider`.

use futures::StreamExt;
use xiaoguai_llm::{BedrockBackend, ChatRequest, FinishReason, LlmBackend, Message};

/// Helper to build a `BedrockBackend` pointing at a mockito server.
fn backend_for(server_url: &str) -> BedrockBackend {
    BedrockBackend::with_config(
        "us-east-1",
        "AKIAIOSFODNN7EXAMPLE",
        "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        None,
        Some(server_url.to_string()),
    )
}

// ── Happy path — Anthropic model on Bedrock ────────────────────────────────

/// Raw JSON lines simulating Bedrock's response for an Anthropic model.
/// Each line is a newline-delimited JSON object. In production these would
/// be wrapped in the binary event-stream format with `bytes` (base64), but
/// our streaming parser falls back to treating lines as raw JSON in test mode.
fn anthropic_bedrock_response() -> String {
    let chunk1 = r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"He"}}"#;
    let chunk2 = r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"llo"}}"#;
    let stop = r#"{"type":"message_stop"}"#;
    format!("{chunk1}\n{chunk2}\n{stop}\n")
}

// Bedrock returns a binary `application/vnd.amazon.eventstream` framing
// (with HMAC-validated prelude / payload sections); the mockito-served
// raw JSON-lines path exercises the JSON-lines fallback of the framing
// parser — the binary path is unit-tested in `bedrock.rs`.
#[tokio::test]
async fn bedrock_anthropic_streams_text() {
    let mut server = mockito::Server::new_async().await;
    let path = "/model/anthropic.claude-sonnet-4-6-v1:0/invoke-with-response-stream";
    let mock = server
        .mock("POST", path)
        .with_status(200)
        .with_header("content-type", "application/vnd.amazon.eventstream")
        .with_body(anthropic_bedrock_response())
        .create_async()
        .await;

    let backend = backend_for(&server.url());
    let req = ChatRequest::new(
        "anthropic.claude-sonnet-4-6-v1:0",
        vec![Message::user("hi")],
    );

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

// ── Happy path — Meta Llama model on Bedrock ──────────────────────────────

fn llama_bedrock_response() -> String {
    let chunk1 = r#"{"generation":"He","stop_reason":null}"#;
    let chunk2 = r#"{"generation":"llo","stop_reason":null}"#;
    let stop = r#"{"generation":"","stop_reason":"stop"}"#;
    format!("{chunk1}\n{chunk2}\n{stop}\n")
}

#[tokio::test]
async fn bedrock_llama_streams_text() {
    let mut server = mockito::Server::new_async().await;
    let path = "/model/meta.llama3-70b-instruct-v1:0/invoke-with-response-stream";
    let mock = server
        .mock("POST", path)
        .with_status(200)
        .with_header("content-type", "application/vnd.amazon.eventstream")
        .with_body(llama_bedrock_response())
        .create_async()
        .await;

    let backend = backend_for(&server.url());
    let req = ChatRequest::new(
        "meta.llama3-70b-instruct-v1:0",
        vec![Message::system("You are helpful."), Message::user("hi")],
    );

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

// ── Error path: non-2xx response ───────────────────────────────────────────

#[tokio::test]
async fn bedrock_backend_reports_error_on_non_2xx() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", mockito::Matcher::Any)
        .with_status(403)
        .with_body(r#"{"message":"The security token included in the request is invalid."}"#)
        .create_async()
        .await;

    let backend = backend_for(&server.url());
    let req = ChatRequest::new(
        "anthropic.claude-sonnet-4-6-v1:0",
        vec![Message::user("hi")],
    );

    match backend.chat_stream(req).await {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("403"),
                "expected status in error message, got: {msg}"
            );
        }
        Ok(_) => panic!("expected error, got Ok"),
    }
}

// ── Unsupported model family returns InvalidRequest at stream time ─────────

#[tokio::test]
async fn bedrock_unsupported_model_returns_invalid_request() {
    // We still need a server to avoid connection errors, but the error should
    // come before any HTTP call is made (during body construction).
    let backend = BedrockBackend::with_config(
        "us-east-1",
        "AKID",
        "SECRET",
        None,
        Some("http://127.0.0.1:1".to_string()), // unreachable — shouldn't be reached
    );
    let req = ChatRequest::new("amazon.titan-text-express-v1", vec![Message::user("hi")]);

    match backend.chat_stream(req).await {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("unsupported") || msg.contains("invalid"),
                "expected InvalidRequest error, got: {msg}"
            );
        }
        Ok(_) => panic!("expected error for unsupported model, got Ok"),
    }
}

// ── Backend name ──────────────────────────────────────────────────────────

#[test]
fn bedrock_backend_name() {
    let backend = backend_for("http://localhost");
    assert_eq!(backend.name(), "bedrock");
}
