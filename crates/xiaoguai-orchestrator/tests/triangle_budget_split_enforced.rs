//! §7 case 5 — Worker budget cap fires before completion;
//! `BudgetExhausted { role: Worker }` + `Final { BudgetExhausted }`
//! are emitted.
//!
//! Strategy: two-task plan + a custom `TriangleBudget` of 1/89/10 so
//! `worker_cap = 20` while `critic_cap = 200` (enough for one Critic
//! call). Task 1 burns ~25 tokens of Worker budget. Task 2's pre-call
//! Worker gate sees remaining = 0 → `BudgetExhausted` { Worker }.
//!
//! Sprint-9 S9-6.

mod triangle_common;

use std::sync::Arc;

use tokio_stream::StreamExt;
use triangle_common::{make_critic_response, make_planner_response, CannedBackend};

use xiaoguai_llm::mock::{MockBackend, ScriptStep};
use xiaoguai_llm::LlmBackend;
use xiaoguai_orchestrator::patterns::triangle::{
    OrchEvent, SessionId, TriangleRequest, TriangleRunner, TriangleStopReason,
};
use xiaoguai_orchestrator::triangle::critic_agent::CriticAgent;
use xiaoguai_orchestrator::triangle::memory_view::InMemoryMemoryView;
use xiaoguai_orchestrator::triangle::planner_agent::PlannerAgent;
use xiaoguai_orchestrator::triangle::roles::Role;
use xiaoguai_orchestrator::triangle::worker_agent::WorkerAgent;
use xiaoguai_orchestrator::triangle::TriangleBudget;

#[tokio::test]
async fn triangle_budget_split_enforced() {
    // Two-task plan. Task 1 burns budget; task 2's pre-call gate fires.
    let plan = make_planner_response(
        "two-step expensive job",
        &[
            ("step one — burn budget", "non-empty"),
            ("step two — should never run", "non-empty"),
        ],
    );
    let planner_backend = CannedBackend::new("planner", vec![&plan]);

    // Worker script: one long text → ~25 tokens via estimate_tokens
    // (100 chars + 3) / 4 = 25.
    let burn = "x".repeat(100);
    let worker_mock = MockBackend::with_script(vec![ScriptStep::text(burn.as_str())]);
    let worker_backend: Arc<dyn LlmBackend> = Arc::new(worker_mock);

    // Critic approves task 1; we never reach task 2's Critic call.
    let critic_approve = make_critic_response("approve", "task 1 ok");
    let critic_backend = CannedBackend::new("critic", vec![&critic_approve]);

    // Custom budget: 1/89/10 → with parent=2000, worker_cap=20,
    // planner_cap=1780, critic_cap=200 (exactly equals
    // CRITIC_CALL_TOKENS_ESTIMATE = 200 — enough for one Critic call).
    let budget = TriangleBudget::new(1, 89, 10).unwrap();
    let parent_budget = 2000u64;

    let planner = Arc::new(PlannerAgent::new(
        planner_backend.clone() as Arc<dyn LlmBackend>,
        "Planner".into(),
    ));
    let worker = Arc::new(WorkerAgent::new(worker_backend, "Worker".into(), vec![]));
    let critic = Arc::new(CriticAgent::new(
        critic_backend.clone() as Arc<dyn LlmBackend>,
        "Critic".into(),
    ));
    let memory = InMemoryMemoryView::new();

    let runner = TriangleRunner::new(planner, worker, critic, memory, budget, parent_budget, 3, 3);

    let req = TriangleRequest {
        goal: "two-step expensive job".into(),
        session_id: SessionId::new(),
    };

    let mut stream = Box::pin(runner.stream(req));
    let mut events: Vec<OrchEvent> = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }

    // Expected sequence:
    //   PlanProduced, TaskStarted (task 1), WorkerCompleted (ok=true),
    //   CriticVerdict (Approve), TaskStarted (task 2),
    //   BudgetExhausted { Worker }, Final { BudgetExhausted }
    let has_budget_exhausted_worker = events
        .iter()
        .any(|e| matches!(e, OrchEvent::BudgetExhausted { role: Role::Worker }));
    assert!(
        has_budget_exhausted_worker,
        "expected BudgetExhausted {{ Worker }} event, got: {events:?}"
    );

    // Task 1's Worker completed successfully before task 2 ran out of budget.
    let worker_ok = events
        .iter()
        .filter(|e| matches!(e, OrchEvent::WorkerCompleted { ok: true, .. }))
        .count();
    assert_eq!(worker_ok, 1, "task 1 should have completed");

    let final_event = events.last().expect("at least one event");
    match final_event {
        OrchEvent::Final {
            stop_reason: TriangleStopReason::BudgetExhausted,
            summary,
        } => {
            assert!(
                summary.to_lowercase().contains("worker"),
                "summary should mention worker, got: {summary}"
            );
        }
        other => panic!("expected Final {{ BudgetExhausted }}, got {other:?}"),
    }
}
