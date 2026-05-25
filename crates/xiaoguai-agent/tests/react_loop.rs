//! End-to-end coverage for the v0.5.4 ReAct loop driving a scripted
//! `MockBackend` against an in-memory `MockMcpClient`. Each test pins one
//! invariant of the loop contract so regressions surface immediately.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use xiaoguai_agent::{AgentConfig, AgentEvent, ReactAgent, StopReason, Toolbox};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, Message, MockBackend, ToolCallSpec};

use common::{MockMcpClient, ToolResponse};

fn make_call(id: &str, name: &str, args: &serde_json::Value) -> ToolCallSpec {
    ToolCallSpec {
        id: id.into(),
        name: name.into(),
        arguments_json: args.to_string(),
    }
}

fn make_agent(steps: Vec<ScriptStep>, toolbox: Toolbox) -> ReactAgent {
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(steps));
    ReactAgent::new(backend, toolbox, AgentConfig::new("mock-model"))
}

#[tokio::test]
async fn no_tools_no_calls_one_iteration_done() {
    let agent = make_agent(vec![ScriptStep::text("hello, world")], Toolbox::new());
    let (outcome, events) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");
    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.iterations, 1);

    // Must have at least: TextDelta("hello, world"), IterationCompleted{0},
    // Done{Completed}.
    let deltas: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { delta } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, "hello, world");
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            stop_reason: StopReason::Completed
        })
    ));
    // Final assistant message preserved in history.
    let last = outcome.messages.last().unwrap();
    assert_eq!(last.content, "hello, world");
}

#[tokio::test]
async fn single_tool_call_then_final_text() {
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("found A".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");

    let agent = make_agent(
        vec![
            ScriptStep::tool_calls(vec![make_call(
                "c1",
                "search",
                &serde_json::json!({"q": "x"}),
            )]),
            ScriptStep::text("answer: found A"),
        ],
        toolbox,
    );

    let (outcome, events) = agent
        .run_to_completion(vec![Message::user("find x")], CancellationToken::new())
        .await
        .expect("ok");
    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(outcome.iterations, 2);
    assert_eq!(mock.call_count("search"), 1);

    // Events must include exactly one ToolCallStarted, one ToolCallFinished(ok),
    // and at least one TextDelta.
    let started = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallStarted { .. }))
        .count();
    let finished_ok = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallFinished { ok: true, .. }))
        .count();
    assert_eq!(started, 1);
    assert_eq!(finished_ok, 1);

    // History must contain assistant(tool_calls), tool(result), assistant(text).
    let kinds: Vec<&str> = outcome
        .messages
        .iter()
        .map(|m| match m.role {
            xiaoguai_llm::Role::User => "u",
            xiaoguai_llm::Role::System => "s",
            xiaoguai_llm::Role::Assistant if !m.tool_calls.is_empty() => "a_tc",
            xiaoguai_llm::Role::Assistant => "a_text",
            xiaoguai_llm::Role::Tool => "t",
        })
        .collect();
    assert_eq!(kinds, vec!["u", "a_tc", "t", "a_text"]);
}

#[tokio::test]
async fn parallel_tool_dispatch_is_actually_parallel() {
    // Two scripted delayed tools (200ms each). Sequential dispatch would take
    // ~400ms; parallel ~200ms. Pick a forgiving threshold (300ms) to avoid
    // CI flake on slow runners while still catching a regression to serial.
    let mock = MockMcpClient::new(vec![
        (
            "a",
            ToolResponse::Delayed(Duration::from_millis(200), "A".into()),
        ),
        (
            "b",
            ToolResponse::Delayed(Duration::from_millis(200), "B".into()),
        ),
    ]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let agent = make_agent(
        vec![
            ScriptStep::tool_calls(vec![
                make_call("c1", "a", &serde_json::json!({})),
                make_call("c2", "b", &serde_json::json!({})),
            ]),
            ScriptStep::text("done"),
        ],
        toolbox,
    );

    let start = Instant::now();
    let (outcome, _events) = agent
        .run_to_completion(vec![Message::user("go")], CancellationToken::new())
        .await
        .expect("ok");
    let elapsed = start.elapsed();
    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert!(
        elapsed < Duration::from_millis(350),
        "expected parallel dispatch (~200ms), got {elapsed:?}"
    );
}

#[tokio::test]
async fn tool_error_surfaces_as_error_event_and_keeps_loop_alive() {
    let mock = MockMcpClient::new(vec![("broken", ToolResponse::Err("disk full".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let agent = make_agent(
        vec![
            ScriptStep::tool_calls(vec![make_call("c1", "broken", &serde_json::json!({}))]),
            ScriptStep::text("apology"),
        ],
        toolbox,
    );

    let (outcome, events) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");

    let failed = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallFinished { ok: false, .. }))
        .count();
    assert_eq!(failed, 1);
    assert_eq!(outcome.stop_reason, StopReason::Completed);

    // The tool message we injected for the next LLM turn must encode the error.
    let tool_msg = outcome
        .messages
        .iter()
        .find(|m| matches!(m.role, xiaoguai_llm::Role::Tool))
        .expect("tool message in history");
    assert!(tool_msg.content.contains("disk full"));
}

#[tokio::test]
async fn max_iterations_stops_the_loop() {
    // Both script steps return tool_calls, so the loop would run forever
    // unless max_iterations bites.
    let mock = MockMcpClient::new(vec![("loop_tool", ToolResponse::Ok("once".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::tool_calls(
            vec![make_call("c", "loop_tool", &serde_json::json!({}))],
        )]));
    let mut cfg = AgentConfig::new("mock");
    cfg.max_iterations = 3;
    let agent = ReactAgent::new(backend, toolbox, cfg);

    let (outcome, _events) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");
    assert_eq!(outcome.stop_reason, StopReason::MaxIterations);
    assert_eq!(outcome.iterations, 3);
    // The mock backend was called once per iteration.
    assert_eq!(mock.call_count("loop_tool"), 3);
}

#[tokio::test]
async fn cancellation_token_stops_between_iterations() {
    let mock = MockMcpClient::new(vec![(
        "slow",
        ToolResponse::Delayed(Duration::from_millis(100), "x".into()),
    )]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::tool_calls(
            vec![make_call("c", "slow", &serde_json::json!({}))],
        )]));
    let agent = ReactAgent::new(backend, toolbox, AgentConfig::new("mock"));
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();

    // Cancel after the first dispatch completes — agent should observe
    // cancellation before the next LLM call.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel2.cancel();
    });

    let (outcome, _events) = agent
        .run_to_completion(vec![Message::user("hi")], cancel)
        .await
        .expect("ok");
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    assert!(outcome.iterations >= 1);
}

#[tokio::test]
async fn streaming_events_are_emitted_in_order() {
    let mock = MockMcpClient::new(vec![("tool", ToolResponse::Ok("r".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let agent = make_agent(
        vec![
            ScriptStep::tool_calls(vec![make_call("c1", "tool", &serde_json::json!({}))]),
            ScriptStep::text("hi"),
        ],
        toolbox,
    );

    let (_, mut stream) = agent.run_stream(vec![Message::user("go")], CancellationToken::new());
    let mut order = Vec::new();
    while let Some(ev) = stream.next().await {
        order.push(match ev {
            AgentEvent::TextDelta { .. } => "text",
            AgentEvent::ToolCallStarted { .. } => "tool_started",
            AgentEvent::ToolCallFinished { .. } => "tool_finished",
            AgentEvent::IterationCompleted { .. } => "iter_done",
            AgentEvent::Done { .. } => "done",
            AgentEvent::Error { .. } => "error",
        });
    }

    // Expected order: tool_started, tool_finished, iter_done (iter 0),
    // text, iter_done (iter 1), done.
    assert_eq!(
        order,
        vec![
            "tool_started",
            "tool_finished",
            "iter_done",
            "text",
            "iter_done",
            "done",
        ]
    );
}

#[tokio::test]
async fn sliding_window_keeps_system_message() {
    let mock = MockMcpClient::new(vec![("noop", ToolResponse::Ok("ok".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c", "noop", &serde_json::json!({}))]),
        ScriptStep::tool_calls(vec![make_call("c", "noop", &serde_json::json!({}))]),
        ScriptStep::text("final"),
    ]));
    let mut cfg = AgentConfig::new("mock");
    cfg.history_window = 2; // very tight to force trimming
    let agent = ReactAgent::new(backend, toolbox, cfg);

    let initial = vec![Message::system("you are xiaoguai"), Message::user("start")];
    let (outcome, _) = agent
        .run_to_completion(initial, CancellationToken::new())
        .await
        .expect("ok");
    assert_eq!(outcome.stop_reason, StopReason::Completed);

    // System must still be at the head.
    assert!(matches!(
        outcome.messages.first().map(|m| m.role),
        Some(xiaoguai_llm::Role::System)
    ));
    assert_eq!(
        outcome.messages.first().unwrap().content,
        "you are xiaoguai"
    );
}

#[tokio::test]
async fn unknown_tool_name_marks_call_failed() {
    // Toolbox without registering "ghost", but model asks for it.
    let mock = MockMcpClient::new(vec![("real", ToolResponse::Ok("r".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let agent = make_agent(
        vec![
            ScriptStep::tool_calls(vec![make_call("c1", "ghost", &serde_json::json!({}))]),
            ScriptStep::text("fallback"),
        ],
        toolbox,
    );

    let (outcome, events) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");
    let failed = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolCallFinished { ok: false, .. }))
        .count();
    assert_eq!(failed, 1);
    assert_eq!(outcome.stop_reason, StopReason::Completed);
}
