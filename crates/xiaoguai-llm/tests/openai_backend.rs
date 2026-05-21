//! `OpenAiCompatBackend` talks to any `OpenAI`-compatible `/v1/chat/completions`
//! endpoint that streams Server-Sent Events. Covered: `OpenAI`, vLLM,
//! `DeepSeek`, 通义, 智谱, etc.

use futures::StreamExt;
use xiaoguai_llm::{ChatRequest, LlmBackend, Message, OpenAiCompatBackend, Role};

#[tokio::test]
async fn openai_backend_parses_streamed_sse() {
    let mut server = mockito::Server::new_async().await;
    let body = "\
data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n\
data: [DONE]\n\n";
    let mock = server
        .mock("POST", "/chat/completions")
        .match_header("authorization", "Bearer test-key")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(body)
        .create_async()
        .await;

    let backend = OpenAiCompatBackend::new(server.url(), Some("test-key".to_string()));
    let req = ChatRequest {
        model: "deepseek-chat".into(),
        messages: vec![Message {
            role: Role::User,
            content: "hi".into(),
        }],
        temperature: None,
        max_tokens: None,
    };

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
    assert!(saw_done);
}

#[tokio::test]
async fn openai_backend_omits_auth_header_when_no_key() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/chat/completions")
        .match_header("authorization", mockito::Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body("data: [DONE]\n\n")
        .create_async()
        .await;

    let backend = OpenAiCompatBackend::new(server.url(), None);
    let req = ChatRequest {
        model: "qwen".into(),
        messages: vec![Message {
            role: Role::User,
            content: "hi".into(),
        }],
        temperature: None,
        max_tokens: None,
    };
    let mut stream = backend.chat_stream(req).await.expect("stream");
    while let Some(c) = stream.next().await {
        let _ = c.expect("chunk");
    }
    mock.assert_async().await;
}

#[tokio::test]
async fn openai_backend_reports_provider_error_on_non_2xx() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/chat/completions")
        .with_status(503)
        .with_body("upstream timeout")
        .create_async()
        .await;

    let backend = OpenAiCompatBackend::new(server.url(), None);
    let req = ChatRequest {
        model: "x".into(),
        messages: vec![],
        temperature: None,
        max_tokens: None,
    };
    let err = match backend.chat_stream(req).await {
        Ok(_) => panic!("expected error, got Ok"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("503"), "expected status in error, got: {msg}");
}

/// vLLM and some self-hosted gateways terminate the stream with `finish_reason`
/// inside the last `data:` event and **omit** the `[DONE]` sentinel. We must
/// still surface a single `done: true` chunk.
#[tokio::test]
async fn openai_backend_handles_finish_reason_without_done_sentinel() {
    let mut server = mockito::Server::new_async().await;
    let body = "\
data: {\"choices\":[{\"delta\":{\"content\":\"vLLM\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";
    let _mock = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(body)
        .create_async()
        .await;

    let backend = OpenAiCompatBackend::new(server.url(), None);
    let req = ChatRequest {
        model: "qwen2.5".into(),
        messages: vec![Message {
            role: Role::User,
            content: "hi".into(),
        }],
        temperature: None,
        max_tokens: None,
    };
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
    assert_eq!(collected, "vLLM");
    assert_eq!(done_count, 1, "exactly one done=true chunk expected");
}

#[test]
fn openai_backend_name() {
    let backend = OpenAiCompatBackend::new("http://x", None);
    assert_eq!(backend.name(), "openai_compat");
}
