//! Sprint-12 S12-5/S12-9 — happy-path suspend → operator allows.
//!
//! Drives a `Suspend`-emitting gate (mimicking `SuspendingHotlGate`), then
//! resolves the registered ticket via its `oneshot::Sender` to simulate an
//! operator approve. Asserts the SSE-visible event sequence and that the
//! underlying MCP tool actually dispatched after the resolve.

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
    HotlDecisionVerdict, HotlGate, HotlGateVerdict, HotlResolution as GateResolution,
    HotlSuspensionTicket,
};
use xiaoguai_agent::{
    AgentConfig, AgentEvent, HotlResolution as EventResolution, ReactAgent, StopReason, Toolbox,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, Message, MockBackend, ToolCallSpec};

use common::{MockMcpClient, ToolResponse};

/// Tuple of `(request_id, sender)` stashed by `TestSuspendGate` so tests
/// can simulate `DecisionRegistry::resolve(...)` from the outside.
type PendingEntry = (Uuid, oneshot::Sender<HotlDecisionVerdict>);

/// Test fixture gate that always emits `Suspend`. Each `check()` invocation
/// mints a fresh ticket and stashes the matching `oneshot::Sender` in a
/// shared list so the test can pop one and send through it.
#[derive(Debug)]
struct TestSuspendGate {
    /// Pending senders, in registration order so tests can drive the first
    /// one without juggling specific UUIDs.
    pending: Arc<Mutex<Vec<PendingEntry>>>,
    /// Expiry passed into each ticket. Tests vary this to exercise the
    /// timeout branch.
    expiry: Duration,
}

impl TestSuspendGate {
    fn new(expiry: Duration) -> Arc<Self> {
        Arc::new(Self {
            pending: Arc::new(Mutex::new(Vec::new())),
            expiry,
        })
    }

    /// Block until the gate has at least one registered waiter, then return
    /// `(request_id, sender)` for the first one. Polls because the loop runs
    /// in a spawned task and may not have hit the gate yet.
    async fn take_first_pending(self: &Arc<Self>) -> PendingEntry {
        for _ in 0..200 {
            if let Some(entry) = self.pending.lock().pop() {
                return entry;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no waiter registered within 2s");
    }
}

#[async_trait]
impl HotlGate for TestSuspendGate {
    async fn check(&self, _tenant: Uuid, scope: &str, _amount: f64) -> HotlGateVerdict {
        let request_id = Uuid::new_v4();
        let expires_at = Instant::now() + self.expiry;
        let (ticket, sender) = HotlSuspensionTicket::new(request_id, expires_at);
        self.pending.lock().push((request_id, sender));
        HotlGateVerdict::Suspend {
            request_id,
            scope: scope.to_string(),
            ticket,
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
async fn suspend_then_operator_allow_dispatches_tool() {
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("hit".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c1", "search", &serde_json::json!({}))]),
        ScriptStep::text("done"),
    ]));
    let gate = TestSuspendGate::new(Duration::from_secs(60));
    let gate_dyn: Arc<dyn HotlGate> = gate.clone();
    let mut cfg = AgentConfig::new("mock").with_hotl_gate(gate_dyn);
    cfg.tenant_id = Some(Uuid::new_v4().to_string());

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let cancel = CancellationToken::new();
    let (handle, mut stream) =
        agent.run_stream(vec![Message::user("hi")], cancel.clone());

    // Collect events on a side task so we can drive the registry concurrently.
    let collected: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let collected_clone = collected.clone();
    let drain = tokio::spawn(async move {
        use tokio_stream::StreamExt;
        while let Some(ev) = stream.next().await {
            collected_clone.lock().push(ev);
        }
    });

    // Wait for the gate to register a waiter, then resolve it.
    let (request_id, sender) = gate.take_first_pending().await;
    sender
        .send(HotlDecisionVerdict {
            verdict: GateResolution::Allow,
            decided_by: Some("operator@example.com".into()),
            recorded_at: chrono::Utc::now(),
        })
        .expect("send must succeed");

    drain.await.expect("drain task ok");
    let outcome = handle.await.expect("join").expect("agent ok");
    assert_eq!(outcome.stop_reason, StopReason::Completed);
    assert_eq!(mock.call_count("search"), 1, "tool must dispatch on Allow");

    let events = collected.lock().clone();
    let pending_idx = events
        .iter()
        .position(|e| matches!(e, AgentEvent::HotlPending { request_id: r, .. } if *r == request_id))
        .expect("HotlPending emitted");
    let resolved_idx = events
        .iter()
        .position(|e| matches!(e, AgentEvent::HotlResolved { request_id: r, .. } if *r == request_id))
        .expect("HotlResolved emitted");
    let finished_idx = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ToolCallFinished { name, .. } if name == "search"))
        .expect("ToolCallFinished emitted");

    assert!(
        pending_idx < resolved_idx,
        "HotlPending must precede HotlResolved"
    );
    assert!(
        resolved_idx < finished_idx,
        "HotlResolved must precede ToolCallFinished"
    );

    let resolved = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::HotlResolved {
                request_id: r,
                verdict,
                decided_by,
                ..
            } if *r == request_id => Some((verdict.clone(), decided_by.clone())),
            _ => None,
        })
        .expect("resolved present");
    assert_eq!(resolved.0, EventResolution::Allow);
    assert_eq!(resolved.1.as_deref(), Some("operator@example.com"));

    let finished_ok = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCallFinished { name, ok, .. } if name == "search" => Some(*ok),
            _ => None,
        })
        .expect("finished present");
    assert!(finished_ok, "Allow path must end ok=true");

    // Pending HotlPending fields must mirror the request_id + tool + scope.
    let pending = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::HotlPending {
                request_id: r,
                tool,
                scope,
                ..
            } if *r == request_id => Some((tool.clone(), scope.clone())),
            _ => None,
        })
        .expect("pending present");
    assert_eq!(pending.0, "search");
    assert_eq!(pending.1, "tool_call.search");
}
