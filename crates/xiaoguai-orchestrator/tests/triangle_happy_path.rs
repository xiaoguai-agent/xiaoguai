//! §7 case 1 — Planner → 2 Workers (sequential) → Critic Approves both
//! → Final { Completed }.
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
async fn triangle_happy_path() {
    // Planner: returns a 2-task plan once.
    let planner_resp = make_planner_response(
        "summarise the Q3 release",
        &[
            ("collect Q3 PR titles", "non-empty list"),
            ("draft the summary", "<= 3 paragraphs"),
        ],
    );
    let planner_backend = CannedBackend::new("planner", vec![&planner_resp]);

    // Worker: each task gets one Worker call → final assistant text.
    // The backend is shared across both Worker calls; we pre-load both
    // answers in order. The MockBackend Critic does not invoke the
    // Worker; the Worker calls the worker backend twice in sequence.
    let worker_backend =
        CannedBackend::new("worker", vec!["answer for task one", "answer for task two"]);

    // Critic: approves both Worker results.
    let critic_a = make_critic_response("approve", "task one looks great");
    let critic_b = make_critic_response("approve", "task two looks great");
    let critic_backend = CannedBackend::new("critic", vec![&critic_a, &critic_b]);

    let planner = Arc::new(PlannerAgent::new(
        planner_backend.clone() as Arc<dyn LlmBackend>,
        "You are the Planner.".into(),
    ));
    let worker = Arc::new(WorkerAgent::new(
        worker_backend.clone() as Arc<dyn LlmBackend>,
        "You are the Worker.".into(),
        vec![],
    ));
    let critic = Arc::new(CriticAgent::new(
        critic_backend.clone() as Arc<dyn LlmBackend>,
        "You are the Critic.".into(),
    ));
    let memory = InMemoryMemoryView::new();

    let runner = TriangleRunner::new_with_defaults(planner, worker, critic, memory, 10_000);
    let req = TriangleRequest {
        goal: "summarise the Q3 release".into(),
        session_id: SessionId::new(),
    };

    let mut stream = Box::pin(runner.stream(req));
    let mut events: Vec<OrchEvent> = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }

    // Expected event shape:
    // - PlanProduced { round: 0, task_count: 2 }
    // - TaskStarted * 2
    // - WorkerCompleted * 2
    // - CriticVerdict { Approve } * 2
    // - Final { Completed }
    assert!(
        events.iter().any(|e| matches!(
            e,
            OrchEvent::PlanProduced {
                round: 0,
                task_count: 2
            }
        )),
        "expected PlanProduced(round=0, task_count=2), got {events:?}"
    );

    let task_started_count = events
        .iter()
        .filter(|e| matches!(e, OrchEvent::TaskStarted { .. }))
        .count();
    assert_eq!(task_started_count, 2, "expected 2 TaskStarted events");

    let worker_completed_ok = events
        .iter()
        .filter(|e| matches!(e, OrchEvent::WorkerCompleted { ok: true, .. }))
        .count();
    assert_eq!(
        worker_completed_ok, 2,
        "expected 2 successful Worker completions"
    );

    let approvals = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                OrchEvent::CriticVerdict {
                    kind: VerdictKind::Approve,
                    ..
                }
            )
        })
        .count();
    assert_eq!(approvals, 2, "expected 2 Critic approvals");

    let final_event = events.last().expect("at least one event");
    match final_event {
        OrchEvent::Final {
            stop_reason: TriangleStopReason::Completed,
            summary,
        } => {
            assert!(
                summary.contains("approved=2"),
                "summary should report approved=2, got: {summary}"
            );
            assert!(
                summary.contains("task one looks great"),
                "summary should cite approve reasons, got: {summary}"
            );
        }
        other => panic!("expected Final {{ Completed }}, got {other:?}"),
    }

    // Quarantine sanity — exactly one Planner call (no re-plan).
    assert_eq!(planner_backend.call_count(), 1);
    // Two Critic calls.
    assert_eq!(critic_backend.call_count(), 2);
}
