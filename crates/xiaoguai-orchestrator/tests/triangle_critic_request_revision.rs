//! §7 case 2 — Critic asks for revision once; Worker re-runs with
//! feedback; Critic Approves on the second pass.
//!
//! Sprint-9 S9-6.

mod triangle_common;

use std::sync::Arc;

use tokio_stream::StreamExt;
use triangle_common::{make_critic_response, make_planner_response, CannedBackend};

use xiaoguai_llm::LlmBackend;
use xiaoguai_orchestrator::patterns::triangle::{
    OrchEvent, SessionId, TriangleRequest, TriangleRunner, TriangleStopReason,
};
use xiaoguai_orchestrator::triangle::critic_agent::CriticAgent;
use xiaoguai_orchestrator::triangle::memory_view::InMemoryMemoryView;
use xiaoguai_orchestrator::triangle::planner_agent::PlannerAgent;
use xiaoguai_orchestrator::triangle::verdict::VerdictKind;
use xiaoguai_orchestrator::triangle::worker_agent::WorkerAgent;

#[tokio::test]
async fn triangle_critic_request_revision() {
    // Single-task plan.
    let plan = make_planner_response(
        "answer the question",
        &[("write the answer", "mentions the source")],
    );
    let planner_backend = CannedBackend::new("planner", vec![&plan]);

    // Worker called twice — first pass: initial draft, second pass:
    // improved draft incorporating the Critic's feedback.
    let worker_backend = CannedBackend::new(
        "worker",
        vec![
            "initial draft without citation",
            "improved draft with [src]",
        ],
    );

    // Critic responses: first RequestRevision, then Approve.
    let critic_revise = make_critic_response("request_revision", "add a citation");
    let critic_approve = make_critic_response("approve", "now has citation");
    let critic_backend = CannedBackend::new("critic", vec![&critic_revise, &critic_approve]);

    let planner = Arc::new(PlannerAgent::new(
        planner_backend.clone() as Arc<dyn LlmBackend>,
        "Planner".into(),
    ));
    let worker = Arc::new(WorkerAgent::new(
        worker_backend.clone() as Arc<dyn LlmBackend>,
        "Worker".into(),
        vec![],
    ));
    let critic = Arc::new(CriticAgent::new(
        critic_backend.clone() as Arc<dyn LlmBackend>,
        "Critic".into(),
    ));
    let memory = InMemoryMemoryView::new();

    let runner = TriangleRunner::new_with_defaults(planner, worker, critic, memory, 10_000);
    let req = TriangleRequest {
        goal: "answer the question".into(),
        session_id: SessionId::new(),
    };

    let mut stream = Box::pin(runner.stream(req));
    let mut events: Vec<OrchEvent> = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }

    // Exactly one TaskStarted (we're testing within-task revision, not replan).
    let task_starts = events
        .iter()
        .filter(|e| matches!(e, OrchEvent::TaskStarted { .. }))
        .count();
    assert_eq!(task_starts, 1);

    // Two WorkerCompleted events (initial + revision).
    let worker_done = events
        .iter()
        .filter(|e| matches!(e, OrchEvent::WorkerCompleted { ok: true, .. }))
        .count();
    assert_eq!(worker_done, 2, "Worker should have run twice for revision");

    // Critic verdicts in order: RequestRevision → Approve.
    let verdicts: Vec<VerdictKind> = events
        .iter()
        .filter_map(|e| match e {
            OrchEvent::CriticVerdict { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        verdicts,
        vec![VerdictKind::RequestRevision, VerdictKind::Approve]
    );

    // No Replan event — revision is within-task; replans happen
    // across plan-rounds.
    assert!(
        !events.iter().any(|e| matches!(e, OrchEvent::Replan { .. })),
        "RequestRevision should not trigger Replan"
    );

    let final_event = events.last().expect("at least one event");
    match final_event {
        OrchEvent::Final {
            stop_reason: TriangleStopReason::Completed,
            ..
        } => {}
        other => panic!("expected Final {{ Completed }}, got {other:?}"),
    }

    // Backend call counts: planner=1, worker=2, critic=2.
    assert_eq!(planner_backend.call_count(), 1);
    assert_eq!(worker_backend.call_count(), 2);
    assert_eq!(critic_backend.call_count(), 2);
}
