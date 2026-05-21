//! `MockBackend` returns canned responses for deterministic tests.

use futures::StreamExt;
use xiaoguai_llm::{ChatRequest, LlmBackend, Message, MockBackend};

#[tokio::test]
async fn mock_backend_returns_canned_chunks() {
    let backend = MockBackend::with_response("Hello from Xiaoguai!");
    let req = ChatRequest::new("mock", vec![Message::user("hi")]);

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

    assert_eq!(collected, "Hello from Xiaoguai!");
    assert!(saw_done);
}

#[tokio::test]
async fn mock_backend_name() {
    let backend = MockBackend::with_response("");
    assert_eq!(backend.name(), "mock");
}
