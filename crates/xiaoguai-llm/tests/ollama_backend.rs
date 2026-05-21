//! `OllamaBackend` talks to an Ollama-compatible /api/chat endpoint.
//! These tests mock the HTTP layer with mockito — no real Ollama needed.

use futures::StreamExt;
use xiaoguai_llm::{ChatRequest, LlmBackend, Message, OllamaBackend, Role};

#[tokio::test]
async fn ollama_backend_parses_streamed_response() {
    let mut server = mockito::Server::new_async().await;
    // Ollama streams JSON-per-line, one object per chunk.
    let body = r#"{"model":"qwen2.5","message":{"role":"assistant","content":"He"},"done":false}
{"model":"qwen2.5","message":{"role":"assistant","content":"llo"},"done":false}
{"model":"qwen2.5","done":true}
"#;
    let mock = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_header("content-type", "application/x-ndjson")
        .with_body(body)
        .create_async()
        .await;

    let backend = OllamaBackend::new(server.url());
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
