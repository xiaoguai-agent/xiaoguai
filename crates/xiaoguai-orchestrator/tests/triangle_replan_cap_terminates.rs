//! §7 case 6 — Critic always rejects → after `max_replans` plan-rounds
//! the stream emits `Final { MaxReplansReached }`.
//!
//! Strategy: set `max_replans=2`. The Critic rejects every Worker
//! result. The runner should run plan-round 0 + plan-round 1, then
//! terminate with `MaxReplansReached` after the second Reject.
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
use xiaoguai_orchestrator::triangle::TriangleBudget;

#[tokio::test]
async fn triangle_replan_cap_terminates() {
    // Planner emits the same plan structure each round; backend
    // returns the same JSON twice (round 0 + round 1). The runner
    // will only call the Planner twice because max_replans=2 caps the
    // loop after 2 plan-rounds.
    let plan = make_planner_response("hopeless goal", &[("attempt", "rubric")]);
    let planner_backend = CannedBackend::new("planner", vec![&plan, &plan]);

    // Worker: one response per round (CannedBackend repeats the last
    // entry if drained).
    let worker_backend = CannedBackend::new("worker", vec!["answer A", "answer B"]);

    // Critic: reject everything.
    let critic_reject = make_critic_response("reject", "this is never acceptable");
    let critic_backend =
        CannedBackend::new("critic", vec![&critic_reject, &critic_reject]);

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

    // max_replans=2: exactly two plan-rounds before termination.
    let runner = TriangleRunner::new(
        planner,
        worker,
        critic,
        memory,
        TriangleBudget::DEFAULT,
        100_000,
        2, // max_replans
        3,
    );

    let req = TriangleRequest {
        goal: "hopeless goal".into(),
        session_id: SessionId::new(),
    };

    let mut stream = Box::pin(runner.stream(req));
    let mut events: Vec<OrchEvent> = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }

    // Two PlanProduced events (round 0 and round 1) — but NO round 2.
    let plan_rounds: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            OrchEvent::PlanProduced { round, .. } => Some(*round),
            _ => None,
        })
        .collect();
    assert_eq!(plan_rounds, vec![0, 1], "expected exactly 2 plan-rounds");

    // Exactly one Replan event (between round 0 and round 1).
    let replan_count = events
        .iter()
        .filter(|e| matches!(e, OrchEvent::Replan { .. }))
        .count();
    assert_eq!(replan_count, 1, "expected exactly 1 Replan event");

    // Both verdicts are Reject.
    let verdicts: Vec<VerdictKind> = events
        .iter()
        .filter_map(|e| match e {
            OrchEvent::CriticVerdict { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    assert_eq!(verdicts, vec![VerdictKind::Reject, VerdictKind::Reject]);

    // Final = MaxReplansReached.
    let final_event = events.last().expect("at least one event");
    match final_event {
        OrchEvent::Final {
            stop_reason: TriangleStopReason::MaxReplansReached,
            summary,
        } => {
            assert!(
                summary.contains("rejected=2"),
                "summary should report rejected=2, got: {summary}"
            );
        }
        other => panic!("expected Final {{ MaxReplansReached }}, got {other:?}"),
    }

    // Planner called exactly twice (no third round).
    assert_eq!(planner_backend.call_count(), 2);
    // Worker called exactly twice.
    assert_eq!(worker_backend.call_count(), 2);
    // Critic called exactly twice.
    assert_eq!(critic_backend.call_count(), 2);
}
