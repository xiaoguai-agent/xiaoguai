//! Sprint-12 S12-5/S12-9 — back-compat regression guard (default-off proof).
//!
//! This test pins the v1.8.x behaviour: when the configured `HotlGate` only
//! emits `Allow`/`Deny` (i.e. legacy `EnforcerGate`, the production default
//! while `agent.hotl.suspend_on_escalate=false`), the ReAct loop MUST NOT
//! emit any `HotlPending` / `HotlResolved` events. Tool dispatch proceeds
//! exactly as before — Escalate is mapped to Allow upstream of the gate.
//!
//! This is the §3.2 behaviour-gate verification: it proves the S12-5 loop
//! changes are gated on the new `Suspend` variant and do NOT regress any
//! tenant still on the legacy adapter.

mod common;

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, AgentEvent, AllowAllGate, ReactAgent, StopReason, Toolbox};
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

#[tokio::test]
async fn legacy_allow_path_emits_no_hotl_events() {
    // Simulates the production v1.8.x setup: `EnforcerGate` mapping every
    // upstream verdict (including `Escalate`) to `HotlGateVerdict::Allow`.
    // The standin `AllowAllGate` covers the same surface — Allow / no Suspend.
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("hit".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c1", "search", &serde_json::json!({}))]),
        ScriptStep::text("done"),
    ]));
    let mut cfg = AgentConfig::new("mock").with_hotl_gate(Arc::new(AllowAllGate));
    cfg.tenant_id = Some(Uuid::new_v4().to_string());

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let (outcome, events) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("agent ok");

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(mock.call_count("search"), 1, "tool must dispatch");

    let hotl_pending_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::HotlPending { .. }))
        .count();
    let hotl_resolved_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::HotlResolved { .. }))
        .count();
    assert_eq!(
        hotl_pending_count, 0,
        "legacy path must NOT emit HotlPending (suspend_on_escalate=false)"
    );
    assert_eq!(
        hotl_resolved_count, 0,
        "legacy path must NOT emit HotlResolved (suspend_on_escalate=false)"
    );

    // And the successful dispatch must surface as the normal ToolCallFinished.
    let finished = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCallFinished { ok, name, .. } if name == "search" => Some(*ok),
            _ => None,
        })
        .expect("ToolCallFinished for search");
    assert!(finished, "Allow path must dispatch successfully");
}
