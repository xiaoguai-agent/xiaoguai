//! §7 case 4 — Two tasks run sequentially; Worker B's `Scratchpad`
//! must NOT contain any of Worker A's writes (DEC-021 §4.5 quarantine
//! invariant).
//!
//! The runner enforces this structurally by calling
//! `Scratchpad::new(task.id)` inside the per-task loop — see
//! `patterns/triangle.rs` `run_loop`. This test verifies the
//! invariant **behaviourally** via the Critic's side-channel: the
//! Critic reads the scratchpad tail into its system prompt, so any
//! cross-task leak would surface there.
//!
//! Method:
//! - Each task description embeds a distinct sentinel
//!   (`TASK_A_SENTINEL` / `TASK_B_SENTINEL`).
//! - The Worker's `CannedBackend` mirrors that sentinel verbatim into
//!   the final assistant text — so it lands in the scratchpad.
//! - After the run, we inspect the captured Critic `ChatRequest`s:
//!   - Call 1 (reviewing Worker A) must contain the A-sentinel and
//!     NOT the B-sentinel.
//!   - Call 2 (reviewing Worker B) must contain the B-sentinel and
//!     NOT the A-sentinel.
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
use xiaoguai_orchestrator::triangle::worker_agent::WorkerAgent;

const SENTINEL_A: &str = "QUARANTINE_ALPHA_E5G7Q";
const SENTINEL_B: &str = "QUARANTINE_BRAVO_M9X2K";

#[tokio::test]
async fn triangle_scratchpad_quarantine() {
    // 2-task plan. Each task description carries a distinct sentinel
    // so we can check cross-contamination via the captured prompts.
    let plan = make_planner_response(
        "two-task quarantine",
        &[
            (&format!("first job — produce {SENTINEL_A}"), "non-empty"),
            (&format!("second job — produce {SENTINEL_B}"), "non-empty"),
        ],
    );
    let planner_backend = CannedBackend::new("planner", vec![&plan]);

    // Worker echoes the sentinel into its final assistant text → it
    // lands in the scratchpad entry for that task.
    let worker_resp_a = format!("Done — see {SENTINEL_A}");
    let worker_resp_b = format!("Done — see {SENTINEL_B}");
    let worker_backend = CannedBackend::new("worker", vec![&worker_resp_a, &worker_resp_b]);

    // Critic approves both.
    let critic_a = make_critic_response("approve", "ok A");
    let critic_b = make_critic_response("approve", "ok B");
    let critic_backend = CannedBackend::new("critic", vec![&critic_a, &critic_b]);

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
        goal: "two-task quarantine".into(),
        session_id: SessionId::new(),
    };

    let mut stream = Box::pin(runner.stream(req));
    let mut events: Vec<OrchEvent> = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }

    let final_event = events.last().expect("at least one event");
    match final_event {
        OrchEvent::Final {
            stop_reason: TriangleStopReason::Completed,
            ..
        } => {}
        other => panic!("expected Final {{ Completed }}, got {other:?}"),
    }

    // Now the heart of the test: inspect the Critic's captured
    // requests.
    let critic_calls = critic_backend.captured();
    assert_eq!(critic_calls.len(), 2, "expected two Critic calls");

    // The Critic's system prompt embeds the scratchpad tail (see
    // critic_agent.rs::build_system_prompt). The Worker writes each
    // ReAct iteration's text — including the sentinel — into the
    // scratchpad.

    let prompt_a = critic_calls[0]
        .messages
        .iter()
        .find(|m| m.role == xiaoguai_llm::Role::System)
        .expect("system message in call 1")
        .content
        .clone();
    let prompt_b = critic_calls[1]
        .messages
        .iter()
        .find(|m| m.role == xiaoguai_llm::Role::System)
        .expect("system message in call 2")
        .content
        .clone();

    // The Critic reviewing Worker A sees A-sentinel from scratchpad.
    assert!(
        prompt_a.contains(SENTINEL_A),
        "Critic call 1 should see Worker A's scratchpad with {SENTINEL_A}; got: {prompt_a}"
    );
    // The Critic reviewing Worker A must NEVER see B-sentinel.
    assert!(
        !prompt_a.contains(SENTINEL_B),
        "Critic call 1 must NOT see Worker B's content — quarantine breach! Got: {prompt_a}"
    );

    // Symmetric: the Critic reviewing Worker B sees B-sentinel only.
    assert!(
        prompt_b.contains(SENTINEL_B),
        "Critic call 2 should see Worker B's scratchpad with {SENTINEL_B}; got: {prompt_b}"
    );
    assert!(
        !prompt_b.contains(SENTINEL_A),
        "Critic call 2 must NOT see Worker A's content — quarantine breach! Got: {prompt_b}"
    );

    // Additional verification: the TaskStarted events carry distinct
    // task_ids — the orchestrator never reuses ids across tasks.
    let task_ids: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            OrchEvent::TaskStarted { task_id, .. } => Some(*task_id),
            _ => None,
        })
        .collect();
    assert_eq!(task_ids.len(), 2);
    assert_ne!(
        task_ids[0], task_ids[1],
        "TaskIds must be distinct per task"
    );
}
