//! Three-step research pipeline using mock workers and a hand-rolled planner.
//!
//! Steps:
//!   1. research  — gather raw notes on the topic
//!   2. summarise — distil the notes into key points (depends on research)
//!   3. format    — wrap the summary into a structured report (depends on summarise)
//!
//! Run with:
//!   `cargo run --example three_step_research -p xiaoguai-orchestrator`

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoguai_orchestrator::{
    budget::Budget,
    plan::PlanStep,
    planner::Planner,
    supervisor::RunReport,
    worker::{Task, Worker, WorkerResult},
    OrchestratorError, StepResult, Supervisor,
};

// ── Workers ───────────────────────────────────────────────────────────────────

struct ResearchWorker;

#[async_trait]
impl Worker for ResearchWorker {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        println!("[research] executing: {}", task.description);
        Ok(WorkerResult {
            output: "Raw notes: Rust ownership model eliminates entire classes of bugs. \
                     Zero-cost abstractions. Fearless concurrency via Send/Sync."
                .to_string(),
            success: true,
        })
    }
}

struct SummariseWorker;

#[async_trait]
impl Worker for SummariseWorker {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        let ctx = task.context.join("; ");
        println!("[summarise] context from prior steps: {ctx}");
        Ok(WorkerResult {
            output: "Key points: (1) Memory safety without GC, \
                     (2) Zero-cost abstractions, \
                     (3) Fearless concurrency."
                .to_string(),
            success: true,
        })
    }
}

struct FormatWorker;

#[async_trait]
impl Worker for FormatWorker {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        let ctx = task.context.join("\n");
        println!("[format] composing final report from:\n{ctx}");
        Ok(WorkerResult {
            output: "# Rust Programming Language — Summary Report\n\
                     \n\
                     ## Key Points\n\
                     1. Memory safety without garbage collection\n\
                     2. Zero-cost abstractions\n\
                     3. Fearless concurrency via Send + Sync traits\n"
                .to_string(),
            success: true,
        })
    }
}

// ── Planner ───────────────────────────────────────────────────────────────────

/// Hand-rolled three-step planner with dependency edges:
///   research → summarise → format
struct ThreeStepPlanner {
    emitted: Mutex<Vec<String>>,
}

impl ThreeStepPlanner {
    fn new() -> Self {
        Self {
            emitted: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Planner for ThreeStepPlanner {
    async fn next_step(
        &self,
        goal: &str,
        history: &[StepResult],
    ) -> Result<Option<PlanStep>, OrchestratorError> {
        let done: Vec<String> = history
            .iter()
            .filter(|r| r.success)
            .map(|r| r.step_id.clone())
            .collect();
        let mut emitted = self.emitted.lock().unwrap();

        if !emitted.contains(&"research".to_string()) {
            emitted.push("research".to_string());
            return Ok(Some(PlanStep::new(
                "research",
                format!("Gather raw research notes about: {goal}"),
                vec![],
            )));
        }
        if !emitted.contains(&"summarise".to_string()) && done.contains(&"research".to_string()) {
            emitted.push("summarise".to_string());
            return Ok(Some(PlanStep::new(
                "summarise",
                "Distil the research notes into concise key points.",
                vec!["research".to_string()],
            )));
        }
        if !emitted.contains(&"format".to_string()) && done.contains(&"summarise".to_string()) {
            emitted.push("format".to_string());
            return Ok(Some(PlanStep::new(
                "format",
                "Format the key points into a structured report.",
                vec!["summarise".to_string()],
            )));
        }
        // All steps done → signal goal achieved.
        Ok(None)
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let goal = "Rust programming language";

    println!("=== three_step_research example ===");
    println!("Goal: {goal}\n");

    let budget = Budget::new().with_max_steps(10);
    let planner = ThreeStepPlanner::new();

    let mut supervisor = Supervisor::new(budget, Box::new(planner));

    // Each step type gets its own worker; round-robin will pick them
    // in order because we add exactly three and each step appears once.
    // In a real deployment you'd use a single general-purpose agent worker.
    supervisor.add_worker(Arc::new(ResearchWorker));
    supervisor.add_worker(Arc::new(SummariseWorker));
    supervisor.add_worker(Arc::new(FormatWorker));

    let RunReport { outcome, history } = supervisor
        .run_detailed(goal)
        .await
        .expect("supervisor run failed");

    println!("\n=== run complete: {outcome:?} ===");
    for step in &history {
        let status = if step.success { "OK" } else { "FAIL" };
        println!("[{status}] {}: {}", step.step_id, step.output);
    }

    assert_eq!(
        outcome,
        xiaoguai_orchestrator::RunOutcome::GoalAchieved,
        "expected GoalAchieved"
    );
    assert_eq!(history.len(), 3, "expected exactly 3 steps");

    println!("\nFinal report:");
    if let Some(last) = history.last() {
        println!("{}", last.output);
    }
}
