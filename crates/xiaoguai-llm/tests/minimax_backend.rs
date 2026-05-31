//! `MinimaxBackend` integration tests.
//!
//! All HTTP is intercepted by mockito — no real `MiniMax` API calls.

use futures::StreamExt;
use xiaoguai_llm::{ChatRequest, FinishReason, LlmBackend, Message, MinimaxBackend};

/// Stream with an interleaved reasoning chunk + a content chunk + finish.
fn thinking_mode_sse() -> &'static str {
    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Step 1: parse the question.\"}}]}\n\n\
     data: {\"choices\":[{\"delta\":{\"reasoning_content\":\" Step 2: compute the answer.\"}}]}\n\n\
     data: {\"choices\":[{\"delta\":{\"content\":\"Hello,\"}}]}\n\n\
     data: {\"choices\":[{\"delta\":{\"content\":\" world!\"}}]}\n\n\
     data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"
}

#[tokio::test]
async fn minimax_streams_content_and_reasoning_deltas() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/chat/completions")
        .match_header("authorization", "Bearer mm-test-key")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(thinking_mode_sse())
        .create_async()
        .await;

    let backend = MinimaxBackend::with_base_url(server.url(), "mm-test-key");
    let req = ChatRequest::new("MiniMax-M2", vec![Message::user("hi")]);

    let mut stream = backend.chat_stream(req).await.expect("stream");
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut finish_reason = None;
    let mut saw_done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        content.push_str(&chunk.delta);
        if let Some(r) = chunk.reasoning_delta.as_deref() {
            reasoning.push_str(r);
        }
        if chunk.done {
            saw_done = true;
            finish_reason = chunk.finish_reason;
        }
    }

    mock.assert_async().await;
    assert_eq!(content, "Hello, world!");
    assert_eq!(
        reasoning,
        "Step 1: parse the question. Step 2: compute the answer."
    );
    assert!(saw_done, "stream must end with done=true");
    assert_eq!(finish_reason, Some(FinishReason::Stop));
}

#[tokio::test]
async fn minimax_chat_only_model_leaves_reasoning_none() {
    let body = "\
data: {\"choices\":[{\"delta\":{\"content\":\"plain\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";

    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(body)
        .create_async()
        .await;

    let backend = MinimaxBackend::with_base_url(server.url(), "k");
    let req = ChatRequest::new("abab6.5-chat", vec![Message::user("hi")]);
    let mut stream = backend.chat_stream(req).await.expect("stream");

    let mut content = String::new();
    let mut any_reasoning = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        content.push_str(&chunk.delta);
        if chunk.reasoning_delta.is_some() {
            any_reasoning = true;
        }
    }

    assert_eq!(content, "plain");
    assert!(!any_reasoning, "abab6.5-chat must not emit reasoning_delta");
}

#[tokio::test]
async fn minimax_reports_error_on_non_2xx() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(401)
        .with_body(r#"{"error":{"message":"invalid api key"}}"#)
        .create_async()
        .await;

    let backend = MinimaxBackend::with_base_url(server.url(), "bad");
    let req = ChatRequest::new("MiniMax-M2", vec![]);
    match backend.chat_stream(req).await {
        Err(e) => assert!(e.to_string().contains("401")),
        Ok(_) => panic!("expected error"),
    }
}

#[tokio::test]
async fn minimax_reasoning_increments_prometheus_counter() {
    use xiaoguai_observability::prometheus::{init_prometheus, llm_reasoning_tokens_total};

    let _ = init_prometheus();
    let counter = llm_reasoning_tokens_total().expect("counter registered");
    let before = counter.with_label_values(&["minimax", "MiniMax-M2"]).get();

    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(thinking_mode_sse())
        .create_async()
        .await;

    let backend = MinimaxBackend::with_base_url(server.url(), "k");
    let req = ChatRequest::new("MiniMax-M2", vec![Message::user("hi")]);
    let mut stream = backend.chat_stream(req).await.expect("stream");
    while let Some(c) = stream.next().await {
        let _ = c;
    }

    let after = counter.with_label_values(&["minimax", "MiniMax-M2"]).get();
    assert!(
        after > before,
        "counter must increment for reasoning chunks (before={before}, after={after})"
    );
}

#[test]
fn minimax_backend_name() {
    let b = MinimaxBackend::new("k");
    assert_eq!(b.name(), "minimax");
}
