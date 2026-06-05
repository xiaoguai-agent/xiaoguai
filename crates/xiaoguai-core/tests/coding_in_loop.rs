//! Composition test: the ReAct loop drives a *registered* coding tool.
//!
//! Proves the tool-registration seam end-to-end — a model `edit_file` tool call
//! flows loop → `CodingMcpClient` → `GovernedTools` → a real file mutation, with
//! the governed `code.edit` step recorded. The loop's gating/dispatch and the
//! client's dispatch are each unit-tested elsewhere; this pins that they compose.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use xiaoguai_agent::{AgentConfig, ReactAgent, Toolbox};
use xiaoguai_coding::{
    coding_tool_descriptors, CodingGate, CodingMcpClient, CodingStep, GateDecision, GovernedTools,
    StepRecorder, Workspace,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, Message, ToolCallSpec};
use xiaoguai_mcp::McpClient;

/// Allow-all gate (the loop is the real gate; this layer only checkpoints+audits).
struct AllowGate;
#[async_trait]
impl CodingGate for AllowGate {
    async fn decide(&self, _scope: &str) -> GateDecision {
        GateDecision::Allow
    }
}

/// Recorder that collects the action names it was handed, for assertion.
#[derive(Clone)]
struct RecordingRecorder {
    actions: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl StepRecorder for RecordingRecorder {
    async fn record(&self, step: CodingStep) {
        self.actions.lock().unwrap().push(step.action);
    }
}

/// Deny-everything gate, for the negative test.
struct DenyGate;
#[async_trait]
impl CodingGate for DenyGate {
    async fn decide(&self, _scope: &str) -> GateDecision {
        GateDecision::Deny("nope".into())
    }
}

/// Recorder that discards steps.
struct NoopRecorder;
#[async_trait]
impl StepRecorder for NoopRecorder {
    async fn record(&self, _step: CodingStep) {}
}

#[tokio::test]
async fn model_edit_file_tool_call_mutates_the_workspace_through_the_governed_client() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::open_or_create(dir.path()).await.unwrap();

    let actions = Arc::new(Mutex::new(Vec::new()));
    let recorder = RecordingRecorder {
        actions: actions.clone(),
    };
    let tools = GovernedTools::new(ws, AllowGate, recorder);
    let client: Arc<dyn McpClient> = Arc::new(CodingMcpClient::new(tools));

    // Register the coding tools exactly as xiaoguai-core does at boot.
    let toolbox = Toolbox::from_server(client, coding_tool_descriptors()).expect("toolbox");

    // The model: call edit_file, then (after the tool result) finish with text.
    let edit_call = ToolCallSpec {
        id: "c1".into(),
        name: "edit_file".into(),
        arguments_json: serde_json::json!({ "path": "hello.txt", "content": "hi from the loop" })
            .to_string(),
    };
    let backend: Arc<dyn LlmBackend> = Arc::new(xiaoguai_llm::MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![edit_call]),
        ScriptStep::text("done — wrote hello.txt"),
    ]));

    let agent = ReactAgent::new(backend, toolbox, AgentConfig::new("mock"));
    let (outcome, _events) = agent
        .run_to_completion(
            vec![Message::user("write hello.txt")],
            CancellationToken::new(),
        )
        .await
        .expect("loop ok");

    // The file the model asked for was actually written, via the governed path.
    let written = tokio::fs::read_to_string(dir.path().join("hello.txt"))
        .await
        .expect("file should exist");
    assert_eq!(written, "hi from the loop");

    // The governed mutation recorded a `code.edit` step (checkpoint + audit half).
    assert!(
        actions.lock().unwrap().iter().any(|a| a == "code.edit"),
        "expected a code.edit audit step, got {:?}",
        actions.lock().unwrap()
    );

    // The loop ran to completion with the final assistant text.
    assert!(outcome
        .messages
        .iter()
        .any(|m| m.content.contains("wrote hello.txt")));
}

/// A denied edit (the loop's gate says no) must NOT mutate the file. Here we
/// simulate the deny at the coding-gate layer to prove the gate→no-mutation
/// contract holds through the registered client too.
#[tokio::test]
async fn denied_edit_does_not_mutate() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::open_or_create(dir.path()).await.unwrap();
    let tools = GovernedTools::new(ws, DenyGate, NoopRecorder);
    let client: Arc<dyn McpClient> = Arc::new(CodingMcpClient::new(tools));

    let res = client
        .call_tool(
            "edit_file",
            serde_json::json!({ "path": "x.txt", "content": "should not appear" }),
        )
        .await
        .unwrap();

    assert!(res.is_error, "denied edit should surface as a tool error");
    assert!(
        tokio::fs::metadata(dir.path().join("x.txt")).await.is_err(),
        "denied edit must not create the file"
    );
}
