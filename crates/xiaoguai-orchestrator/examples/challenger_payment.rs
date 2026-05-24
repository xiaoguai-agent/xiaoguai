//! Challenger payment example — institutional bias-checking middleware.
//!
//! Demonstrates a 3-step payment plan:
//!   1. `draft-payment`  (Low risk)  — prepare the payment record.
//!   2. `send-payment`   (High risk) — wire funds externally.
//!   3. `send-receipt`   (Low risk)  — email the receipt.
//!
//! A `MockChallenger` rejects step 2.  The example asserts:
//!   - Step 2 is never dispatched to the worker.
//!   - The audit trail (history) contains the critique reasons for step 2.
//!   - Steps 1 and 3 proceed normally.
//!
//! Run with:
//!   `cargo run --example challenger_payment -p xiaoguai-orchestrator`

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoguai_orchestrator::{
    budget::Budget,
    challenger::{Critique, MockChallenger},
    plan::{PlanStep, RiskLevel},
    planner::Planner,
    supervisor::RunReport,
    worker::{Task, Worker, WorkerResult},
    OrchestratorError, StepResult, Supervisor,
};

// ── Worker ────────────────────────────────────────────────────────────────────

/// A simple echo worker that records which step IDs it executed.
struct AuditedWorker {
    executed: Mutex<Vec<String>>,
}

impl AuditedWorker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            executed: Mutex::new(Vec::new()),
        })
    }

    fn executed_ids(&self) -> Vec<String> {
        self.executed.lock().unwrap().clone()
    }
}

#[async_trait]
impl Worker for AuditedWorker {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        println!("  [worker] executing step '{}'", task.step_id);
        self.executed.lock().unwrap().push(task.step_id.clone());
        Ok(WorkerResult {
            output: format!("completed: {}", task.description),
            success: true,
        })
    }
}

// ── Planner ───────────────────────────────────────────────────────────────────

/// A 3-step sequential payment planner.
struct PaymentPlanner {
    emitted: Mutex<usize>,
}

impl PaymentPlanner {
    fn new() -> Self {
        Self {
            emitted: Mutex::new(0),
        }
    }
}

#[async_trait]
impl Planner for PaymentPlanner {
    async fn next_step(
        &self,
        _goal: &str,
        history: &[StepResult],
    ) -> Result<Option<PlanStep>, OrchestratorError> {
        // Filter out revision markers — only count real results.
        let real_count = history
            .iter()
            .filter(|r| !r.step_id.starts_with("__revision_"))
            .count();

        let mut emitted = self.emitted.lock().unwrap();

        // Only advance when the prior real step has settled.
        if real_count < *emitted {
            return Ok(None);
        }

        let next = match *emitted {
            0 => Some(
                PlanStep::new(
                    "draft-payment",
                    "Draft payment record for $500 wire to vendor-42",
                    vec![],
                ), // Low risk — challenger bypassed.
            ),
            1 => Some(
                PlanStep::new(
                    "send-payment",
                    "Initiate $500 wire transfer to vendor-42 (external, irreversible)",
                    vec![],
                )
                // High risk — will be challenged.
                .with_risk(RiskLevel::High),
            ),
            2 => Some(
                PlanStep::new(
                    "send-receipt",
                    "Email payment confirmation to finance@company.com",
                    vec![],
                ), // Low risk — challenger bypassed.
            ),
            _ => None,
        };

        if next.is_some() {
            *emitted += 1;
        }
        Ok(next)
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    println!("=== challenger_payment example ===");
    println!("Goal: process vendor payment of $500\n");

    let budget = Budget::new().with_max_steps(20);
    let planner = PaymentPlanner::new();
    let worker = AuditedWorker::new();

    // The MockChallenger is pre-loaded with a single Reject verdict.
    // It will be consumed when step 2 (High-risk) hits the challenger.
    let challenger = Arc::new(MockChallenger::new().push(Critique::reject(
        vec![
            "wire transfer is irreversible once submitted".to_string(),
            "dual-approval not recorded in draft".to_string(),
            "vendor-42 has no signed contract on file".to_string(),
        ],
        0.92,
    )));

    let mut supervisor = Supervisor::new(budget, Box::new(planner)).with_challenger(challenger);
    supervisor.add_worker(worker.clone());

    let RunReport { outcome, history } = supervisor
        .run_detailed("vendor payment $500")
        .await
        .expect("supervisor run failed");

    // ── Print the audit trail ─────────────────────────────────────────────────

    println!("\n=== Audit trail ===");
    for step in &history {
        let status = if step.success { "OK  " } else { "SKIP" };
        println!(
            "[{status}] step='{}' output='{}'",
            step.step_id, step.output
        );
        if let Some(reasons) = &step.critique_reasons {
            println!(
                "       CRITIQUE (risk={:.2}):",
                step.critique_risk_score.unwrap_or(0.0)
            );
            for r in reasons {
                println!("         - {r}");
            }
        }
    }

    println!("\n=== Run outcome: {outcome:?} ===");
    println!("Worker executed steps: {:?}", worker.executed_ids());

    // ── Assertions ────────────────────────────────────────────────────────────

    // Steps 1 and 3 were dispatched; step 2 was rejected.
    let executed = worker.executed_ids();
    assert!(
        executed.contains(&"draft-payment".to_string()),
        "draft-payment must be executed"
    );
    assert!(
        !executed.contains(&"send-payment".to_string()),
        "send-payment must NOT be executed (challenger rejected it)"
    );
    assert!(
        executed.contains(&"send-receipt".to_string()),
        "send-receipt must be executed"
    );

    // The rejected step must appear in history with critique data.
    let rejected = history
        .iter()
        .find(|r| r.step_id == "send-payment")
        .expect("send-payment must appear in history even when rejected");
    assert!(!rejected.success, "rejected step must be marked failed");
    assert!(
        rejected.critique_reasons.is_some(),
        "critique reasons must be recorded"
    );
    let reasons = rejected.critique_reasons.as_ref().unwrap();
    assert!(
        reasons.iter().any(|r| r.contains("irreversible")),
        "audit trail must contain the irreversibility reason"
    );
    assert_eq!(
        rejected.critique_risk_score,
        Some(0.92),
        "risk score must be recorded"
    );

    println!("\nAll assertions passed — challenger successfully blocked the payment send.");
    println!("The audit trail captures all three critique reasons for compliance review.");
}
