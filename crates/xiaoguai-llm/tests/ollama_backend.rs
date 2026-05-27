//! `OllamaBackend` talks to an Ollama-compatible /api/chat endpoint.
//! These tests mock the HTTP layer with mockito — no real Ollama needed.

use futures::StreamExt;
use xiaoguai_llm::{ChatRequest, FinishReason, LlmBackend, Message, OllamaBackend, ToolSpec};

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
    let req = ChatRequest::new("qwen2.5", vec![Message::user("hi")]);

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
async fn ollama_backend_parses_tool_call() {
    let mut server = mockito::Server::new_async().await;
    // Ollama returns a completed tool call in `message.tool_calls`, arguments
    // as a JSON object, typically on the final (done) chunk.
    let body = r#"{"model":"qwen2.5","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"get_weather","arguments":{"city":"Paris"}}}]},"done":true}
"#;
    let mock = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_header("content-type", "application/x-ndjson")
        .with_body(body)
        .create_async()
        .await;

    let backend = OllamaBackend::new(server.url());
    let mut req = ChatRequest::new("qwen2.5", vec![Message::user("weather in Paris?")]);
    req.tools = vec![ToolSpec {
        name: "get_weather".to_string(),
        description: Some("Look up the weather for a city".to_string()),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"]
        }),
    }];

    let mut stream = backend.chat_stream(req).await.expect("stream");
    let mut calls = Vec::new();
    let mut finish = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        calls.extend(chunk.tool_calls);
        if chunk.finish_reason.is_some() {
            finish = chunk.finish_reason;
        }
    }

    mock.assert_async().await;
    assert_eq!(calls.len(), 1, "expected one tool call");
    assert_eq!(calls[0].name, "get_weather");
    // Object arguments are re-serialised to the JSON-string form ToolCallSpec carries.
    let args: serde_json::Value =
        serde_json::from_str(&calls[0].arguments_json).expect("valid args json");
    assert_eq!(args["city"], "Paris");
    assert_eq!(finish, Some(FinishReason::ToolCalls));
}
