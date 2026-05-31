//! Sprint-12 S12-5/S12-9 — suspend → parent cancel wins over operator.
//!
//! Per DEC-LLD-AGENT-004 + the design pseudocode in lld-agent.md §4.5,
//! when the agent's `CancellationToken` fires mid-suspend the loop must:
//!  - NOT emit `HotlResolved` (the cancel path will emit `Final(Cancelled)`),
//!  - terminate with `StopReason::Cancelled`.
//!
//! Uses a 60s ticket expiry so the cancel always wins the `tokio::select!`.

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
use xiaoguai_agent::{AgentConfig, AgentEvent, ReactAgent, StopReason, Toolbox};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, Message, MockBackend, ToolCallSpec};

use common::{MockMcpClient, ToolResponse};

#[derive(Debug)]
struct LongSuspendGate {
    /// Hold senders alive so the cancel path (not `ChannelDropped`) is
    /// what terminates the ticket.
    senders: Arc<Mutex<Vec<oneshot::Sender<HotlDecisionVerdict>>>>,
    /// Signals the test the gate has registered a waiter so it can cancel.
    registered: Arc<tokio::sync::Notify>,
}

impl LongSuspendGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            senders: Arc::new(Mutex::new(Vec::new())),
            registered: Arc::new(tokio::sync::Notify::new()),
        })
    }
}

#[async_trait]
impl HotlGate for LongSuspendGate {
    async fn check(&self, _tenant: Uuid, scope: &str, _amount: f64) -> HotlGateVerdict {
        let escalation_id = Uuid::new_v4();
        let expires_at = Instant::now() + Duration::from_secs(60);
        let (ticket, sender) = HotlSuspensionTicket::new(escalation_id, expires_at);
        self.senders.lock().push(sender);
        self.registered.notify_one();
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
async fn cancel_wins_over_pending_operator_decision() {
    let mock = MockMcpClient::new(vec![("search", ToolResponse::Ok("hit".into()))]);
    let toolbox = Toolbox::from_server(mock.clone(), mock.descriptors.clone()).expect("toolbox");
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![make_call("c1", "search", &serde_json::json!({}))]),
        ScriptStep::text("never reached"),
    ]));
    let gate = LongSuspendGate::new();
    let gate_dyn: Arc<dyn HotlGate> = gate.clone();
    let mut cfg = AgentConfig::new("mock").with_hotl_gate(gate_dyn);
    cfg.tenant_id = Some(Uuid::new_v4().to_string());

    let agent = ReactAgent::new(backend, toolbox, cfg);
    let cancel = CancellationToken::new();
    let registered = gate.registered.clone();

    // Cancel the loop once the gate has registered a waiter.
    let cancel_clone = cancel.clone();
    let canceller = tokio::spawn(async move {
        registered.notified().await;
        // Tiny extra grace so the loop is actually awaiting the ticket
        // (not just past register but pre-await).
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_clone.cancel();
    });

    let (outcome, events) = agent
        .run_to_completion(vec![Message::user("hi")], cancel)
        .await
        .expect("agent ok");
    canceller.await.expect("canceller ok");

    assert_eq!(
        outcome.stop_reason,
        StopReason::Cancelled,
        "loop must terminate with Cancelled when token fires mid-suspend"
    );
    assert_eq!(
        mock.call_count("search"),
        0,
        "tool must not dispatch when cancel beat the operator"
    );

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::HotlPending { .. })),
        "HotlPending must have been emitted before cancel"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::HotlResolved { .. })),
        "HotlResolved must NOT be emitted on cancel — Final(Cancelled) is the sole terminator"
    );
}
