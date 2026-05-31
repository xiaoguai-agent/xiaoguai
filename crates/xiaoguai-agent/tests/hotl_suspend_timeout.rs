//! Sprint-12 S12-5/S12-9 — suspend → no operator decision → ticket expires.
//!
//! Sets a 50ms ticket expiry, never sends on the registered sender, and
//! asserts the loop emits `HotlResolved { verdict: Timeout }` plus a
//! synthetic `ToolCallFinished { ok: false }` carrying the timeout marker.

mod common;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use xiaoguai_agent::hotl_gate::{
    HotlDecisionVerdict, HotlGate, HotlGateVerdict, HotlSuspensionTicket,
};
use xiaoguai_agent::{
    AgentConfig, AgentEvent, HotlResolution as EventResolution, ReactAgent, StopReason, Toolbox,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, Message, MockBackend, ToolCallSpec};

use common::{MockMcpClient, ToolResponse};

/// Suspend gate with a configurable short expiry. Senders are stashed so
/// they stay alive past `check()` (Drop would otherwise resolve as
/// `ChannelDropped`, which is a different code path).
#[derive(Debug)]
struct TimeoutSuspendGate {
    senders: Arc<Mutex<Vec<oneshot::Sender<HotlDecisionVerdict>>>>,
    expiry: Duration,
}

impl TimeoutSuspendGate {
    fn new(expiry: Duration) -> Arc<Self> {
        Arc::new(Self {
            senders: Arc::new(Mutex::new(Vec::new())),
            expiry,
        })
    }
}

#[async_trait]
impl HotlGate for TimeoutSuspendGate {
    async fn check(&self, _tenant: Uuid, scope: &str, _amount: f64) -> HotlGateVerdict {
        let escalation_id = Uuid::new_v4();
        let expires_at = Instant::now() + self.expiry;
        let (ticket, sender) = HotlSuspensionTicket::new(escalation_id, expires_at);
        // Keep the sender alive — never sends. The ticket's internal
        // sleep_until fires first and produces a Timeout verdict.
        self.senders.lock().push(sender);
        HotlGateVerdict::Suspend {
            escalation_id,
            scope: scope.to_string(),
            ticket,
            args_redacted: serde_json::Value::Null,
        }
    }
}

fn make_call(id: &str, name: &str, args: &serde_json::Value) -> ToolCallSpec {
    ToolCallSpec {
        id: id.into(),
        name: name.into(),
        arguments_json: args.to_string(),
    }
}

#[tokio::test]
async fn suspend_with_no_decision_resolves_as_timeout() {
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("hit".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c1", "search", &serde_json::json!({}))]),
        ScriptStep::text("model recovers gracefully"),
    ]));
    let gate = TimeoutSuspendGate::new(Duration::from_millis(50));
    let gate_dyn: Arc<dyn HotlGate> = gate.clone();
    let mut cfg = AgentConfig::new("mock").with_hotl_gate(gate_dyn);
    cfg.tenant_id = Some(Uuid::new_v4().to_string());

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let (outcome, events) = agent
        .run_to_completion(vec![Message::user("hi")], CancellationToken::new())
        .await
        .expect("agent ok");

    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(
        mock.call_count("search"),
        0,
        "tool must NOT dispatch on Timeout — synthetic failure only"
    );

    let pending = events
        .iter()
        .find(|e| matches!(e, AgentEvent::HotlPending { .. }))
        .expect("HotlPending emitted");
    let _ = pending;

    let resolved = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::HotlResolved {
                verdict,
                decided_by,
                ..
            } => Some((verdict.clone(), decided_by.clone())),
            _ => None,
        })
        .expect("HotlResolved emitted");
    assert_eq!(resolved.0, EventResolution::Timeout);
    assert!(
        resolved.1.is_none(),
        "Timeout has no operator → decided_by must be None"
    );

    let (finished_ok, finished_err) = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCallFinished {
                name, ok, error, ..
            } if name == "search" => Some((*ok, error.clone())),
            _ => None,
        })
        .expect("ToolCallFinished emitted");
    assert!(!finished_ok, "Timeout path is a synthetic failure");
    let err = finished_err.expect("error message present");
    assert!(
        err.to_lowercase().contains("timeout") || err.to_lowercase().contains("timed out"),
        "error must mention the timeout, got: {err}"
    );
}
