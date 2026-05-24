//! Shared test helpers: `MockWorker` and `MockPlanner`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoguai_orchestrator::{
    plan::PlanStep, planner::Planner, worker::Worker, OrchestratorError, Task, WorkerResult,
};

// ── MockWorker ────────────────────────────────────────────────────────────────

/// Scripted worker — returns pre-loaded `WorkerResult`s in FIFO order.
#[allow(dead_code)]
pub struct MockWorker {
    pub name: String,
    pub responses: Mutex<VecDeque<Result<WorkerResult, OrchestratorError>>>,
    pub call_log: Mutex<Vec<Task>>,
}

#[allow(dead_code)]
impl MockWorker {
    pub fn always_ok(name: &str, output: &str) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            responses: Mutex::new(VecDeque::new()),
            call_log: Mutex::new(Vec::new()),
        })
        .with_default_ok(output)
    }

    fn with_default_ok(self: Arc<Self>, output: &str) -> Arc<Self> {
        let result = WorkerResult {
            output: output.to_string(),
            success: true,
        };
        self.responses.lock().unwrap().push_back(Ok(result));
        self
    }

    /// Push a scripted success response.
    pub fn push_ok(self: Arc<Self>, output: &str) -> Arc<Self> {
        self.responses.lock().unwrap().push_back(Ok(WorkerResult {
            output: output.to_string(),
            success: true,
        }));
        self
    }

    pub fn push_err(self: Arc<Self>, msg: &str) -> Arc<Self> {
        self.responses
            .lock()
            .unwrap()
            .push_back(Err(OrchestratorError::WorkerFailed(msg.to_string())));
        self
    }

    pub fn call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }
}

#[async_trait]
impl Worker for MockWorker {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        self.call_log.lock().unwrap().push(task);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Ok(WorkerResult {
                    output: "mock default ok".to_string(),
                    success: true,
                })
            })
    }
}

// ── MockPlanner ───────────────────────────────────────────────────────────────

/// Scripted planner — emits `PlanStep`s in FIFO order; returns `None` when
/// queue is empty (signals goal achieved).
#[allow(dead_code)]
pub struct MockPlanner {
    steps: Mutex<VecDeque<PlanStep>>,
}

#[allow(dead_code)]
impl MockPlanner {
    /// Build with an ordered list of steps. Once exhausted the planner returns
    /// `None`, which signals the supervisor that the goal is met.
    pub fn new(steps: Vec<PlanStep>) -> Self {
        Self {
            steps: Mutex::new(VecDeque::from(steps)),
        }
    }

    /// Convenience: build N steps named "step-0", "step-1", … with no deps.
    pub fn linear(n: usize) -> Self {
        let steps = (0..n)
            .map(|i| PlanStep::new(format!("step-{i}"), format!("Do step {i}"), vec![]))
            .collect();
        Self::new(steps)
    }
}

#[async_trait]
impl Planner for MockPlanner {
    async fn next_step(
        &self,
        _goal: &str,
        _history: &[xiaoguai_orchestrator::StepResult],
    ) -> Result<Option<PlanStep>, OrchestratorError> {
        Ok(self.steps.lock().unwrap().pop_front())
    }
}
