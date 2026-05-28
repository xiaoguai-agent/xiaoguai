//! Tier-2 prereq: ReAct loop ↔ HOTL gate integration.
//!
//! Verifies the gate is consulted per tool call (not per batch), that a
//! Deny verdict suppresses the dispatch and surfaces the reason to the
//! LLM, and that the legacy "no gate" path is unchanged.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use xiaoguai_agent::hotl_gate::{HotlGate, HotlGateVerdict};
use xiaoguai_agent::{
    AgentConfig, AgentEvent, AllowAllGate, DenyAllGate, ReactAgent, ScopeDenyGate, StopReason,
    Toolbox,
};
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

fn tenant_uuid_str() -> String {
    Uuid::new_v4().to_string()
}

/// `HotlGate` that counts how many times `check` is called and what scopes
/// it saw. Used to assert per-tool semantics.
#[derive(Debug)]
struct CountingGate {
    calls: Arc<parking_lot::Mutex<Vec<String>>>,
    verdict: HotlGateVerdict,
}

#[async_trait]
impl HotlGate for CountingGate {
    async fn check(&self, _tenant: Uuid, scope: &str, _amount: f64) -> HotlGateVerdict {
        self.calls.lock().push(scope.to_string());
        self.verdict.clone()
    }
}

#[tokio::test]
async fn allow_gate_dispatches_tools_normally() {
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("hit".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c1", "search", &serde_json::json!({}))]),
        ScriptStep::text("ok"),
    ]));
    let cfg = AgentConfig::new("mock").with_hotl_gate(Arc::new(AllowAllGate));
    let mut cfg = cfg;
    cfg.tenant_id = Some(tenant_uuid_str());

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let (outcome, _events) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(mock.call_count("search"), 1, "tool must have run");
}

#[tokio::test]
async fn deny_gate_blocks_tool_dispatch_and_surfaces_reason() {
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("hit".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c1", "search", &serde_json::json!({}))]),
        ScriptStep::text("graceful fallback"),
    ]));
    let cfg = AgentConfig::new("mock")
        .with_hotl_gate(Arc::new(DenyAllGate::new("budget exceeded for tier")));
    let mut cfg = cfg;
    cfg.tenant_id = Some(tenant_uuid_str());

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let (outcome, events) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");

    assert_eq!(
        mock.call_count("search"),
        0,
        "deny verdict must suppress the MCP dispatch entirely"
    );

    // The ToolCallFinished event must be ok=false with the gate's reason.
    let denied = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCallFinished {
                ok: false, error, ..
            } => error.clone(),
            _ => None,
        })
        .expect("must have a failed tool event");
    assert!(
        denied.contains("HOTL gate denied") && denied.contains("budget exceeded"),
        "denial reason must include gate marker + upstream reason: {denied}"
    );

    // The synthetic Role::Tool message must propagate the reason to the LLM.
    let tool_msg = outcome
        .messages
        .iter()
        .find(|m| matches!(m.role, xiaoguai_llm::Role::Tool))
        .expect("synthesised tool message in history");
    assert!(
        tool_msg.content.contains("budget exceeded"),
        "tool message must carry the denial reason for the LLM: {}",
        tool_msg.content
    );
    assert_eq!(outcome.stop_reason, StopReason::Completed);
}

#[tokio::test]
async fn no_gate_means_legacy_path_unchanged() {
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("hit".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c1", "search", &serde_json::json!({}))]),
        ScriptStep::text("ok"),
    ]));
    // Default config: no gate, no tenant. Existing tests already cover this
    // path — we just pin the contract that a None gate is a true no-op.
    let cfg = AgentConfig::new("mock");
    assert!(
        cfg.hotl_gate.is_none(),
        "default config must not set a gate"
    );

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let (outcome, _) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");
    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(mock.call_count("search"), 1);
}

#[tokio::test]
async fn gate_is_consulted_once_per_tool_call_not_per_batch() {
    // Two parallel tool calls in one turn → the gate must see exactly two
    // checks with the matching scopes.
    let mock = MockMcpClient::new(vec![
        ("a", ToolResponse::Ok("A".into())),
        ("b", ToolResponse::Ok("B".into())),
    ]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![
            make_call("c1", "a", &serde_json::json!({})),
            make_call("c2", "b", &serde_json::json!({})),
        ]),
        ScriptStep::text("done"),
    ]));
    let calls = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let gate = Arc::new(CountingGate {
        calls: calls.clone(),
        verdict: HotlGateVerdict::Allow,
    });
    let cfg = AgentConfig::new("mock").with_hotl_gate(gate);
    let mut cfg = cfg;
    cfg.tenant_id = Some(tenant_uuid_str());

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let (_, _) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");

    let observed = calls.lock().clone();
    assert_eq!(
        observed.len(),
        2,
        "one check per tool call, got {observed:?}"
    );
    let mut sorted = observed.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec!["tool_call.a".to_string(), "tool_call.b".to_string()],
        "scopes must be `tool_call.<name>` per spec"
    );
}

#[tokio::test]
async fn per_scope_deny_blocks_one_tool_and_allows_the_other() {
    let mock = MockMcpClient::new(vec![
        ("safe_tool", ToolResponse::Ok("safe out".into())),
        ("danger_tool", ToolResponse::Ok("danger out".into())),
    ]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![
            make_call("c1", "safe_tool", &serde_json::json!({})),
            make_call("c2", "danger_tool", &serde_json::json!({})),
        ]),
        ScriptStep::text("done"),
    ]));
    let gate = Arc::new(ScopeDenyGate::new(
        vec!["tool_call.danger_tool".into()],
        "tool not approved for this tenant",
    ));
    let cfg = AgentConfig::new("mock").with_hotl_gate(gate);
    let mut cfg = cfg;
    cfg.tenant_id = Some(tenant_uuid_str());

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let (_outcome, events) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");

    assert_eq!(
        mock.call_count("safe_tool"),
        1,
        "safe_tool must have run (allow verdict)"
    );
    assert_eq!(
        mock.call_count("danger_tool"),
        0,
        "danger_tool must NOT have run (deny verdict)"
    );

    let denied_count = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                AgentEvent::ToolCallFinished {
                    ok: false,
                    error: Some(_),
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        denied_count, 1,
        "exactly one tool call must have failed (the denied one)"
    );
}

#[tokio::test]
async fn missing_tenant_id_bypasses_gate() {
    // Even with a Deny gate plugged in, if the agent has no tenant scope
    // the gate is skipped — there's no policy bucket. This mirrors the
    // upstream `send_message` semantics where the HOTL check is gated on
    // `session_tenant.parse::<Uuid>()` succeeding.
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("hit".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c1", "search", &serde_json::json!({}))]),
        ScriptStep::text("ok"),
    ]));
    let gate = Arc::new(DenyAllGate::new("would deny if tenant were present"));
    // tenant_id is None → gate must be skipped.
    let cfg = AgentConfig::new("mock").with_hotl_gate(gate);

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let (_, _) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");

    assert_eq!(
        mock.call_count("search"),
        1,
        "tool must dispatch when tenant_id is absent"
    );
}

#[tokio::test]
async fn unparseable_tenant_id_bypasses_gate() {
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("hit".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c1", "search", &serde_json::json!({}))]),
        ScriptStep::text("ok"),
    ]));
    let calls = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let gate = Arc::new(CountingGate {
        calls: calls.clone(),
        verdict: HotlGateVerdict::Deny("should not be called".into()),
    });
    let cfg = AgentConfig::new("mock").with_hotl_gate(gate);
    let mut cfg = cfg;
    cfg.tenant_id = Some("not-a-uuid".into());

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let _ = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("ok");

    assert!(calls.lock().is_empty(), "gate must NOT be invoked");
    assert_eq!(mock.call_count("search"), 1, "tool must still dispatch");
}

// Sanity check the `AtomicUsize` import isn't dead. Keeps clippy happy if we
// add concurrent-check tests later.
#[allow(dead_code)]
fn _atomic_smoke() {
    let _ = AtomicUsize::new(0).fetch_add(1, Ordering::Relaxed);
}
