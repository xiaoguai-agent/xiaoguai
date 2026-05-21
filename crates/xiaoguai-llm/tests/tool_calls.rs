//! Regression coverage for v0.5.4 tool-calling additions:
//!   - `OpenAiCompatBackend` accumulates streamed `tool_call` deltas and emits
//!     a single terminal `ChatChunk` with `finish_reason = ToolCalls`.
//!   - `MockBackend::with_script` returns scripted responses in order then
//!     replays the final step (so multi-iteration loops can stabilise without
//!     the test knowing iteration count in advance).
//!   - `Message` builders and `ChatRequest::new` produce backend-compatible
//!     wire shapes (`tool_calls` / `tool_call_id` / `tools` / `tool_choice`).

use futures::StreamExt;
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{
    ChatRequest, FinishReason, LlmBackend, Message, MockBackend, OpenAiCompatBackend, ToolCallSpec,
    ToolChoice, ToolSpec,
};

#[tokio::test]
async fn openai_backend_accumulates_streamed_tool_call_deltas() {
    let mut server = mockito::Server::new_async().await;
    // Two-delta tool call across two SSE events, then finish_reason=tool_calls.
    let body = "\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_x\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"cit\"}}]}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"y\\\":\\\"sf\\\"}\"}}]}}]}\n\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n";
    let _mock = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(body)
        .create_async()
        .await;

    let backend = OpenAiCompatBackend::new(server.url(), None);
    let req = ChatRequest::new("gpt-x", vec![Message::user("hi")]);

    let mut stream = backend.chat_stream(req).await.expect("stream");
    let mut final_chunk = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("chunk");
        if chunk.done {
            final_chunk = Some(chunk);
        }
    }
    let chunk = final_chunk.expect("exactly one done chunk");
    assert_eq!(chunk.finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(chunk.tool_calls.len(), 1);
    let tc = &chunk.tool_calls[0];
    assert_eq!(tc.id, "call_x");
    assert_eq!(tc.name, "get_weather");
    assert_eq!(tc.arguments_json, r#"{"city":"sf"}"#);
}

#[tokio::test]
async fn openai_backend_serializes_tool_choice_and_tools() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::PartialJsonString(
            r#"{"tool_choice":{"type":"function","function":{"name":"get_weather"}}}"#.to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body("data: [DONE]\n\n")
        .create_async()
        .await;

    let backend = OpenAiCompatBackend::new(server.url(), None);
    let mut req = ChatRequest::new("gpt-x", vec![Message::user("hi")]);
    req.tools = vec![ToolSpec {
        name: "get_weather".into(),
        description: Some("look up the weather".into()),
        parameters: serde_json::json!({"type":"object","properties":{}}),
    }];
    req.tool_choice = ToolChoice::Function("get_weather".into());

    let mut stream = backend.chat_stream(req).await.expect("stream");
    while stream.next().await.is_some() {}
    mock.assert_async().await;
}

#[tokio::test]
async fn mock_backend_script_yields_steps_in_order_then_replays_last() {
    let backend = MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![ToolCallSpec {
            id: "c1".into(),
            name: "search".into(),
            arguments_json: r#"{"q":"x"}"#.into(),
        }]),
        ScriptStep::text("done"),
    ]);

    let req = || ChatRequest::new("m", vec![Message::user("hi")]);

    // 1st call: tool_calls step
    let mut s = backend.chat_stream(req()).await.unwrap();
    let mut got_calls: Option<Vec<ToolCallSpec>> = None;
    while let Some(c) = s.next().await {
        let c = c.unwrap();
        if c.done {
            got_calls = Some(c.tool_calls);
        }
    }
    assert_eq!(got_calls.unwrap().len(), 1);

    // 2nd call: text step
    let mut s = backend.chat_stream(req()).await.unwrap();
    let mut text = String::new();
    while let Some(c) = s.next().await {
        text.push_str(&c.unwrap().delta);
    }
    assert_eq!(text, "done");

    // 3rd call: replays the final text step (no panic).
    let mut s = backend.chat_stream(req()).await.unwrap();
    let mut text2 = String::new();
    while let Some(c) = s.next().await {
        text2.push_str(&c.unwrap().delta);
    }
    assert_eq!(text2, "done");
}

#[test]
fn message_builders_produce_expected_shapes() {
    let m = Message::tool("call_42", "result body");
    assert_eq!(m.tool_call_id.as_deref(), Some("call_42"));
    assert_eq!(m.content, "result body");

    let m = Message::assistant_tool_calls(vec![ToolCallSpec {
        id: "c".into(),
        name: "n".into(),
        arguments_json: "{}".into(),
    }]);
    assert!(m.content.is_empty());
    assert_eq!(m.tool_calls.len(), 1);
}
