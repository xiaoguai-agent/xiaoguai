//! Composition test: the ReAct loop drives a *registered* coding tool.
//!
//! Proves the tool-registration seam end-to-end — a model `edit_file` tool call
//! flows loop → `CodingMcpClient` → `GovernedTools` → a real file mutation, with
//! the governed `code.edit` step recorded. And, crucially, that when the
//! **loop's** `HotL` gate denies `tool_call.edit_file`, dispatch is skipped: no
//! file written, no `code.edit` recorded — the exact governance contract the
//! allow-all coding gate relies on.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use xiaoguai_agent::{AgentConfig, ReactAgent, ScopeDenyGate, SharedHotlGate, Toolbox};
use xiaoguai_coding::{
    coding_tool_descriptors, CodingGate, CodingMcpClient, CodingStep, GateDecision, GovernedTools,
    StepRecorder, Workspace,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, Message, ToolCallSpec};
use xiaoguai_mcp::McpClient;

/// Allow-all coding gate — mirrors the production wiring, where the loop (not
/// this layer) is the real `HotL` gate; this layer only checkpoints + audits.
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

/// Register the coding tools over a fresh workspace exactly as core does, with a
/// recorder we can inspect. Returns the toolbox, the temp dir, and the actions log.
async fn registered() -> (Toolbox, tempfile::TempDir, Arc<Mutex<Vec<String>>>) {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::open_or_create(dir.path()).await.unwrap();
    let actions = Arc::new(Mutex::new(Vec::new()));
    let tools = GovernedTools::new(
        ws,
        AllowGate,
        RecordingRecorder {
            actions: actions.clone(),
        },
    );
    let client: Arc<dyn McpClient> = Arc::new(CodingMcpClient::new(tools, false));
    let toolbox = Toolbox::from_server(client, coding_tool_descriptors(false)).expect("toolbox");
    (toolbox, dir, actions)
}

fn edit_script() -> Arc<dyn LlmBackend> {
    let edit_call = ToolCallSpec {
        id: "c1".into(),
        name: "edit_file".into(),
        arguments_json: serde_json::json!({ "path": "hello.txt", "content": "hi from the loop" })
            .to_string(),
    };
    Arc::new(xiaoguai_llm::MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![edit_call]),
        ScriptStep::text("done — wrote hello.txt"),
    ]))
}

#[tokio::test]
async fn model_edit_file_tool_call_mutates_the_workspace_through_the_governed_client() {
    let (toolbox, dir, actions) = registered().await;

    let agent = ReactAgent::new(edit_script(), toolbox, AgentConfig::new("mock"));
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

    assert!(outcome
        .messages
        .iter()
        .any(|m| m.content.contains("wrote hello.txt")));
}

#[tokio::test]
async fn loop_gate_deny_skips_dispatch_no_mutation_no_audit() {
    let (toolbox, dir, actions) = registered().await;

    // The LOOP's gate denies `tool_call.edit_file` — the production contract the
    // allow-all coding gate depends on. Dispatch must be skipped entirely.
    let gate: SharedHotlGate = Arc::new(ScopeDenyGate::new(
        vec!["tool_call.edit_file".to_string()],
        "edits not allowed in this test",
    ));
    let cfg = AgentConfig::new("mock").with_hotl_gate(gate);
    let agent = ReactAgent::new(edit_script(), toolbox, cfg);
    let _ = agent
        .run_to_completion(
            vec![Message::user("write hello.txt")],
            CancellationToken::new(),
        )
        .await
        .expect("loop ok");

    assert!(
        tokio::fs::metadata(dir.path().join("hello.txt"))
            .await
            .is_err(),
        "a loop-denied edit must not create the file"
    );
    assert!(
        !actions.lock().unwrap().iter().any(|a| a == "code.edit"),
        "a loop-denied edit must not record a code.edit step, got {:?}",
        actions.lock().unwrap()
    );
}
