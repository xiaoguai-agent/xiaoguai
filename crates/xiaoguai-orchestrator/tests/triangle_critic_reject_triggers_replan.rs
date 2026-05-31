//! §7 case 3 — Critic Rejects on round 0 → Planner re-plans (round 1)
//! → that plan's task is Approved → Final { Completed }.
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
async fn triangle_critic_reject_triggers_replan() {
    // Two distinct Planner outputs — round 0 + round 1.
    let plan_round_0 = make_planner_response(
        "answer the question",
        &[("first approach (wrong domain)", "rubric A")],
    );
    let plan_round_1 = make_planner_response(
        "answer the question",
        &[("second approach (right domain)", "rubric B")],
    );
    let planner_backend = CannedBackend::new("planner", vec![&plan_round_0, &plan_round_1]);

    // Worker returns one answer per round.
    let worker_backend = CannedBackend::new("worker", vec!["off-topic answer", "on-topic answer"]);

    // Critic: round-0 task gets rejected; round-1 task gets approved.
    let critic_reject = make_critic_response("reject", "wrong domain entirely");
    let critic_approve = make_critic_response("approve", "matches the rubric");
    let critic_backend = CannedBackend::new("critic", vec![&critic_reject, &critic_approve]);

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

    // max_replans=3 — allows up to 3 plan-rounds. We only need 2.
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

    // Two PlanProduced events, rounds 0 and 1.
    let plan_rounds: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            OrchEvent::PlanProduced { round, .. } => Some(*round),
            _ => None,
        })
        .collect();
    assert_eq!(plan_rounds, vec![0, 1]);

    // Exactly one Replan event, citing the rejection reason.
    let replans: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            OrchEvent::Replan { reason, prev_round } => Some((reason.clone(), *prev_round)),
            _ => None,
        })
        .collect();
    assert_eq!(replans.len(), 1);
    assert_eq!(replans[0].1, 0, "Replan should fire after round 0");
    assert!(
        replans[0].0.contains("wrong domain"),
        "Replan should cite reject reason, got: {}",
        replans[0].0
    );

    // Verdict sequence: Reject (round 0) then Approve (round 1).
    let verdicts: Vec<VerdictKind> = events
        .iter()
        .filter_map(|e| match e {
            OrchEvent::CriticVerdict { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    assert_eq!(verdicts, vec![VerdictKind::Reject, VerdictKind::Approve]);

    // Final event = Completed.
    let final_event = events.last().expect("at least one event");
    match final_event {
        OrchEvent::Final {
            stop_reason: TriangleStopReason::Completed,
            summary,
        } => {
            assert!(
                summary.contains("approved=1"),
                "summary should report approved=1, got: {summary}"
            );
        }
        other => panic!("expected Final {{ Completed }}, got {other:?}"),
    }

    // Planner called twice (round 0 + round 1).
    assert_eq!(planner_backend.call_count(), 2);
}
