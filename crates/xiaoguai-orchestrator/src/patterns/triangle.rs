//! `TriangleRunner` — wires Planner / Worker / Critic into the
//! structured loop spec'd by DEC-021 / `lld-orchestrator.md` §4.4–4.7.
//!
//! Sprint-9 S9-5 + S9-6. Sub-plan:
//! `docs/plans/2026-05-31-sprint9-s9-5-s9-6-triangle-pattern.md`.
//!
//! ## Loop shape (§4.4)
//!
//! ```text
//! per plan-round:
//!   snap = memory.snapshot(round)                  // §4.5 invariant
//!   plan = planner.plan(goal, &snap)
//!   for task in plan.tasks:
//!       loop:                                       // revision loop
//!           sp = Scratchpad::new(task.id)           // quarantine (§4.5)
//!           result = worker.execute(task, &mut sp, &snap, remaining_worker)
//!           verdict = critic.review(&result, &task.acceptance_criteria, &sp)
//!           match verdict:
//!               Approve         => next task
//!               RequestRevision => if revisions < max, retry; else force Reject
//!               Reject          => mark plan as failed; break
//!   if any rejected and rounds_left: replan
//!   else: Final
//! ```
//!
//! ## Quarantine invariant (§4.5)
//!
//! Each Task gets a fresh `Scratchpad::new(task.id)` inside the
//! per-task loop. The `Scratchpad` is moved into the Worker call
//! (`&mut`) then handed to the Critic (`&`), and dropped before the
//! next Task starts. No other Worker can see another's notes —
//! physical separation enforced by control flow.
//!
//! ## Budget split (§4.6)
//!
//! `TriangleBudget::split` carves the parent budget into
//! Worker/Planner/Critic caps. The runner tracks cumulative spend
//! per role and emits `BudgetExhausted { role }` + `Final {
//! BudgetExhausted }` when a cap is hit before the loop completes.
//! `TriangleBudget::BudgetTooSmall` (parent budget too small to split)
//! surfaces as `Final { PlannerFailed("budget too small: ...") }`
//! before any agent spawns — see §4 risk row in the sprint plan.
//!
//! ## Memory promotion (deferred)
//!
//! DEC-021 §4.5 spec: on Critic Approve, the Scratchpad's contents
//! migrate into session memory and become visible to the next
//! `MemorySnapshot`. This wiring is **deferred** to a follow-up:
//! - the runner emits an `Approve` verdict + the approved
//!   `WorkerResult`'s artefact as part of the final summary, so
//!   downstream consumers can see what was approved.
//! - the actual `MemoryView::promote_facts` call is its own concern
//!   (touches `xiaoguai-memory::SqliteMemoryStore` semantics; needs a
//!   trait method addition we don't want to ship in this pattern PR).
//!
//! ## `SessionId` placeholder
//!
//! The brief says "existing type from xiaoguai-types or similar"; no
//! such type exists in workspace today. A local
//! `SessionId(Uuid)` newtype lives here and will be replaced by the
//! canonical type when production wiring lands in `xiaoguai-core` in
//! a later sprint.
//!
//! ## Critic spend approximation
//!
//! `CriticAgent::review` does not surface backend token usage. We
//! charge a fixed `CRITIC_CALL_TOKENS_ESTIMATE = 200` per review call
//! — mirrors the `WorkerAgent` pragmatism (text-delta heuristic) from
//! S9-3. Replaced when backend usage reporting lands.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream};
use uuid::Uuid;

use crate::triangle::{
    budget::{BudgetError, TriangleBudget},
    critic_agent::{CriticAgent, CriticError},
    memory_view::MemoryView,
    plan::TaskId,
    planner_agent::{PlannerAgent, PlannerError},
    roles::Role,
    scratchpad::Scratchpad,
    verdict::{Verdict, VerdictKind},
    worker_agent::{WorkerAgent, WorkerError, WorkerStopReason},
};

// =====================================================================
// Constants
// =====================================================================

/// Conservative per-Critic-call token estimate. Critic's `review()`
/// does not expose backend usage; we charge a fixed cost so the 10 %
/// critic budget can still be enforced. Documented as an
/// approximation per S9-3 precedent.
const CRITIC_CALL_TOKENS_ESTIMATE: u64 = 200;

/// Default cap on plan-rounds before we give up. Matches the
/// `max_replans=3` mention in the §4.4 algorithm comment.
pub const DEFAULT_MAX_REPLANS: u32 = 3;

/// Default cap on revisions per task. DEC-021 §4.7 hard cap.
pub const DEFAULT_MAX_REVISIONS_PER_TASK: u32 = 3;

/// Channel capacity for the event stream. 64 covers a handful of
/// plan-rounds × tasks × verdicts; if the consumer is slow the
/// sender awaits backpressure.
const EVENT_CHANNEL_CAPACITY: usize = 64;

// =====================================================================
// Public types — request, events, stop reason, SessionId
// =====================================================================

/// Local newtype while `xiaoguai-types::SessionId` is still pending.
/// Production wiring (next sprint) swaps this for the canonical type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Caller-facing request. `goal` is forwarded to the Planner; the
/// `session_id` is reserved for future memory + audit wiring.
#[derive(Debug, Clone)]
pub struct TriangleRequest {
    pub goal: String,
    pub session_id: SessionId,
}

/// One event from the orchestrator loop. The stream emits these in
/// the order spec'd by §4.4; consumers can match on a final `Final`
/// to find the run's stop reason + summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchEvent {
    PlanProduced {
        round: u32,
        task_count: usize,
    },
    TaskStarted {
        task_id: TaskId,
        round: u32,
    },
    WorkerCompleted {
        task_id: TaskId,
        ok: bool,
        cost_tokens: u64,
    },
    CriticVerdict {
        task_id: TaskId,
        kind: VerdictKind,
        reason: String,
    },
    Replan {
        reason: String,
        prev_round: u32,
    },
    BudgetExhausted {
        role: Role,
    },
    Final {
        stop_reason: TriangleStopReason,
        summary: String,
    },
}

/// Terminal classification for the Final event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriangleStopReason {
    Completed,
    MaxReplansReached,
    BudgetExhausted,
    PlannerFailed(String),
    Cancelled,
}

// =====================================================================
// Errors (internal — surfaced via events, not Result)
// =====================================================================

#[derive(Debug, Error)]
#[allow(dead_code)] // Reserved for the From<*Error> surfacing path; emitted via events for now.
enum LoopError {
    #[error("planner failed: {0}")]
    Planner(#[from] PlannerError),
    #[error("critic failed: {0}")]
    Critic(#[from] CriticError),
    #[error("worker failed: {0}")]
    Worker(#[from] WorkerError),
}

// =====================================================================
// TriangleRunner
// =====================================================================

/// Stateless across `.stream()` calls — runner holds only
/// configuration. Each call to `.stream()` returns a fresh event
/// stream backed by a tokio task that drives the §4.4 algorithm.
pub struct TriangleRunner {
    planner: Arc<PlannerAgent>,
    worker: Arc<WorkerAgent>,
    critic: Arc<CriticAgent>,
    memory: Arc<dyn MemoryView>,
    budget: TriangleBudget,
    parent_budget_tokens: u64,
    max_replans: u32,
    max_revisions_per_task: u32,
}

impl TriangleRunner {
    /// Construct a runner. Note the wide arity — every knob is
    /// caller-supplied to keep the runner pure / testable.
    ///
    /// `max_replans` defaults to [`DEFAULT_MAX_REPLANS`] when zero is
    /// passed only inside [`new_with_defaults`]; this constructor
    /// uses the supplied value verbatim so tests can pin it.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        planner: Arc<PlannerAgent>,
        worker: Arc<WorkerAgent>,
        critic: Arc<CriticAgent>,
        memory: Arc<dyn MemoryView>,
        budget: TriangleBudget,
        parent_budget_tokens: u64,
        max_replans: u32,
        max_revisions_per_task: u32,
    ) -> Self {
        Self {
            planner,
            worker,
            critic,
            memory,
            budget,
            parent_budget_tokens,
            max_replans,
            max_revisions_per_task,
        }
    }

    /// Convenience constructor — uses the documented defaults for
    /// `max_replans` (3) and `max_revisions_per_task` (3) and the
    /// canonical `TriangleBudget::DEFAULT` (50/40/10) split.
    #[must_use]
    pub fn new_with_defaults(
        planner: Arc<PlannerAgent>,
        worker: Arc<WorkerAgent>,
        critic: Arc<CriticAgent>,
        memory: Arc<dyn MemoryView>,
        parent_budget_tokens: u64,
    ) -> Self {
        Self::new(
            planner,
            worker,
            critic,
            memory,
            TriangleBudget::DEFAULT,
            parent_budget_tokens,
            DEFAULT_MAX_REPLANS,
            DEFAULT_MAX_REVISIONS_PER_TASK,
        )
    }

    /// Drive the §4.4 loop and return a stream of `OrchEvent`s. The
    /// stream completes after a terminal `Final` event.
    pub fn stream(&self, req: TriangleRequest) -> impl Stream<Item = OrchEvent> + Send + 'static {
        let (tx, rx) = mpsc::channel::<OrchEvent>(EVENT_CHANNEL_CAPACITY);

        // Clone owned state into the spawned task.
        let planner = self.planner.clone();
        let worker = self.worker.clone();
        let critic = self.critic.clone();
        let memory = self.memory.clone();
        let budget = self.budget;
        let parent_budget_tokens = self.parent_budget_tokens;
        let max_replans = self.max_replans;
        let max_revisions_per_task = self.max_revisions_per_task;

        tokio::spawn(async move {
            run_loop(
                planner,
                worker,
                critic,
                memory,
                budget,
                parent_budget_tokens,
                max_replans,
                max_revisions_per_task,
                req,
                tx,
            )
            .await;
        });

        ReceiverStream::new(rx)
    }
}

// =====================================================================
// Loop body
// =====================================================================

#[allow(clippy::too_many_arguments)]
async fn run_loop(
    planner: Arc<PlannerAgent>,
    worker: Arc<WorkerAgent>,
    critic: Arc<CriticAgent>,
    memory: Arc<dyn MemoryView>,
    budget: TriangleBudget,
    parent_budget_tokens: u64,
    max_replans: u32,
    max_revisions_per_task: u32,
    req: TriangleRequest,
    tx: mpsc::Sender<OrchEvent>,
) {
    // 1. Split budget up front. Fail-early on BudgetTooSmall.
    let caps = match budget.split(parent_budget_tokens) {
        Ok(c) => c,
        Err(BudgetError::BudgetTooSmall { .. } | BudgetError::PercentagesDontSumTo100(_)) => {
            // Format the budget error as a single string for the Final
            // summary; we reuse `BudgetError::Display`.
            let err = budget.split(parent_budget_tokens).unwrap_err();
            emit(
                &tx,
                OrchEvent::Final {
                    stop_reason: TriangleStopReason::PlannerFailed(format!(
                        "budget too small: {err}"
                    )),
                    summary: "budget split rejected before any agent spawned".to_string(),
                },
            )
            .await;
            return;
        }
    };

    let mut worker_spent: u64 = 0;
    let mut planner_spent: u64 = 0;
    let mut critic_spent: u64 = 0;

    // Track approved artefacts across rounds so the final summary can
    // cite both contributions + the Critic's reasons (LLD §7 system-
    // layer test expectation).
    let mut approved_artefacts: Vec<ApprovedArtefact> = Vec::new();
    let mut rejection_reasons: Vec<String> = Vec::new();

    let mut plan_round: u32 = 0;

    loop {
        // 2. Snapshot memory ONCE per plan-round (DEC-021 §4.5 invariant).
        let snapshot = memory.snapshot(plan_round).await;

        // 3. Planner LLM call.
        //    Spending: approximate by adding a fixed-cost estimate; the
        //    runner doesn't actually gate the Planner mid-loop, but we
        //    track spend for telemetry parity.
        let planner_remaining = caps.planner.saturating_sub(planner_spent);
        if planner_remaining == 0 {
            emit(
                &tx,
                OrchEvent::BudgetExhausted {
                    role: Role::Planner,
                },
            )
            .await;
            emit(
                &tx,
                OrchEvent::Final {
                    stop_reason: TriangleStopReason::BudgetExhausted,
                    summary: format!(
                        "planner budget exhausted at round {plan_round} \
                         (planner_spent={planner_spent}, cap={})",
                        caps.planner
                    ),
                },
            )
            .await;
            return;
        }

        let plan = match planner.plan(&req.goal, &snapshot).await {
            Ok(p) => p,
            Err(e) => {
                emit(
                    &tx,
                    OrchEvent::Final {
                        stop_reason: TriangleStopReason::PlannerFailed(e.to_string()),
                        summary: format!("planner failed at round {plan_round}"),
                    },
                )
                .await;
                return;
            }
        };
        // Charge a conservative planner estimate (no usage API yet);
        // mirrors the critic estimate strategy.
        planner_spent = planner_spent.saturating_add(CRITIC_CALL_TOKENS_ESTIMATE);
        emit(
            &tx,
            OrchEvent::PlanProduced {
                round: plan_round,
                task_count: plan.tasks.len(),
            },
        )
        .await;

        let mut any_rejected = false;
        let mut reject_reason_this_round: Option<String> = None;

        // 4. Iterate tasks sequentially. (Concurrent Workers — future
        //    work; v1.6+ ships sequential per LLD §6.)
        'tasks: for task in &plan.tasks {
            emit(
                &tx,
                OrchEvent::TaskStarted {
                    task_id: task.id,
                    round: plan_round,
                },
            )
            .await;

            let mut revision: u32 = 0;
            let mut revision_feedback: Option<String> = None;

            'revisions: loop {
                // 4a. Fresh Scratchpad per Worker attempt — quarantine
                //     invariant. Two tasks NEVER share a Scratchpad
                //     instance.
                let mut sp = Scratchpad::new(task.id);

                // Seed revision feedback (if any) as an entry so the
                // Worker's system prompt picks it up via the standard
                // scratchpad rendering path. Using a 0 token charge —
                // it's overhead, not real LLM work.
                if let Some(fb) = &revision_feedback {
                    // append() rejects empty content; feedback is non-empty by construction
                    // (Critic Verdict::RequestRevision carries a feedback string).
                    let _ = sp.append(task.id, format!("revision feedback: {fb}"), Some(0));
                }

                // 4b. Worker budget gate (cap minus cumulative spend).
                let worker_remaining = caps.worker.saturating_sub(worker_spent);
                if worker_remaining == 0 {
                    emit(&tx, OrchEvent::BudgetExhausted { role: Role::Worker }).await;
                    emit(
                        &tx,
                        OrchEvent::Final {
                            stop_reason: TriangleStopReason::BudgetExhausted,
                            summary: format!(
                                "worker budget exhausted at round {plan_round}, task {} \
                                 (worker_spent={worker_spent}, cap={})",
                                task.id, caps.worker
                            ),
                        },
                    )
                    .await;
                    return;
                }

                // 4c. Worker.execute.
                let result = match worker
                    .execute(task, &mut sp, &snapshot, worker_remaining)
                    .await
                {
                    Ok(r) => r,
                    Err(WorkerError::BudgetTooSmall) => {
                        emit(&tx, OrchEvent::BudgetExhausted { role: Role::Worker }).await;
                        emit(
                            &tx,
                            OrchEvent::Final {
                                stop_reason: TriangleStopReason::BudgetExhausted,
                                summary: "worker budget too small to spawn".to_string(),
                            },
                        )
                        .await;
                        return;
                    }
                    Err(e) => {
                        emit(
                            &tx,
                            OrchEvent::WorkerCompleted {
                                task_id: task.id,
                                ok: false,
                                cost_tokens: 0,
                            },
                        )
                        .await;
                        // Surface as a rejection — the Planner will get
                        // a chance to retry on the next round.
                        any_rejected = true;
                        reject_reason_this_round =
                            Some(format!("worker error on task {}: {e}", task.id));
                        rejection_reasons.push(format!("task {}: worker error: {e}", task.id));
                        break 'revisions;
                    }
                };

                worker_spent = worker_spent.saturating_add(result.cost_tokens);

                // Worker completed (in terms of life-cycle) — `ok=true`
                // means it produced an artefact. Budget-exhausted
                // mid-Worker is reported via stop_reason, not the
                // event-level ok flag.
                let worker_ok = matches!(result.stop_reason, WorkerStopReason::Completed);
                emit(
                    &tx,
                    OrchEvent::WorkerCompleted {
                        task_id: task.id,
                        ok: worker_ok,
                        cost_tokens: result.cost_tokens,
                    },
                )
                .await;

                // Worker hit budget-exhaustion partway through — that's
                // a budget-event from the runner's POV, surface it.
                if matches!(result.stop_reason, WorkerStopReason::BudgetExhausted) {
                    emit(&tx, OrchEvent::BudgetExhausted { role: Role::Worker }).await;
                    emit(
                        &tx,
                        OrchEvent::Final {
                            stop_reason: TriangleStopReason::BudgetExhausted,
                            summary: format!(
                                "worker exhausted budget on task {} (cost_tokens={})",
                                task.id, result.cost_tokens
                            ),
                        },
                    )
                    .await;
                    return;
                }

                // 4d. Critic budget gate.
                let critic_remaining = caps.critic.saturating_sub(critic_spent);
                if critic_remaining < CRITIC_CALL_TOKENS_ESTIMATE {
                    emit(&tx, OrchEvent::BudgetExhausted { role: Role::Critic }).await;
                    emit(
                        &tx,
                        OrchEvent::Final {
                            stop_reason: TriangleStopReason::BudgetExhausted,
                            summary: format!(
                                "critic budget exhausted before reviewing task {} \
                                 (critic_spent={critic_spent}, cap={})",
                                task.id, caps.critic
                            ),
                        },
                    )
                    .await;
                    return;
                }

                // 4e. Critic.review.
                let verdict = match critic.review(&result, &task.acceptance_criteria, &sp).await {
                    Ok(v) => v,
                    Err(e) => {
                        // Treat Critic failure as a rejection so the
                        // Planner gets a re-plan opportunity — same
                        // recovery path as a malformed worker artefact.
                        emit(
                            &tx,
                            OrchEvent::CriticVerdict {
                                task_id: task.id,
                                kind: VerdictKind::Reject,
                                reason: format!("critic error: {e}"),
                            },
                        )
                        .await;
                        any_rejected = true;
                        reject_reason_this_round = Some(format!("critic error: {e}"));
                        rejection_reasons.push(format!("task {}: critic error: {e}", task.id));
                        break 'revisions;
                    }
                };
                critic_spent = critic_spent.saturating_add(CRITIC_CALL_TOKENS_ESTIMATE);

                let kind = verdict.kind();
                emit(
                    &tx,
                    OrchEvent::CriticVerdict {
                        task_id: task.id,
                        kind,
                        reason: verdict.explanation().to_string(),
                    },
                )
                .await;

                match verdict {
                    Verdict::Approve { reason } => {
                        // Memory promotion is deferred (see module doc-
                        // comment). For now, stash the artefact for the
                        // final summary so consumers can cite it.
                        approved_artefacts.push(ApprovedArtefact {
                            task_id: task.id,
                            artefact: result.artefact.clone(),
                            approve_reason: reason,
                        });
                        // Move to the next task.
                        continue 'tasks;
                    }
                    Verdict::RequestRevision { feedback } if revision < max_revisions_per_task => {
                        revision += 1;
                        revision_feedback = Some(feedback);
                        // Falls through to the next iteration of the
                        // 'revisions loop with the feedback baked into
                        // the next Scratchpad. (Used to be an explicit
                        // `continue 'revisions;` — clippy 1.93 flags it
                        // as redundant since the match arm is the last
                        // statement in the loop body.)
                    }
                    Verdict::RequestRevision { feedback } => {
                        // Revision cap hit — force a Reject path. Emit a
                        // synthetic Reject verdict so observers know
                        // *why* we gave up.
                        let reason =
                            format!("too many revisions ({revision}); last feedback: {feedback}");
                        emit(
                            &tx,
                            OrchEvent::CriticVerdict {
                                task_id: task.id,
                                kind: VerdictKind::Reject,
                                reason: reason.clone(),
                            },
                        )
                        .await;
                        any_rejected = true;
                        reject_reason_this_round = Some(reason.clone());
                        rejection_reasons.push(format!("task {}: revision cap: {reason}", task.id));
                        break 'tasks;
                    }
                    Verdict::Reject { reason } => {
                        any_rejected = true;
                        reject_reason_this_round = Some(reason.clone());
                        rejection_reasons.push(format!("task {}: reject: {reason}", task.id));
                        break 'tasks;
                    }
                }
            }
        }

        // 5. Round outcome.
        if !any_rejected {
            // All tasks Approved — Completed.
            emit(
                &tx,
                OrchEvent::Final {
                    stop_reason: TriangleStopReason::Completed,
                    summary: build_summary(plan_round, &approved_artefacts, &rejection_reasons),
                },
            )
            .await;
            return;
        }

        // 6. Replan if we have budget for another round.
        //    `max_replans` counts total plan-rounds. With
        //    max_replans=N we allow plan_round = 0..N-1; after the
        //    N-th rejection we emit MaxReplansReached.
        let reason = reject_reason_this_round
            .clone()
            .unwrap_or_else(|| "unspecified rejection".to_string());

        if plan_round + 1 < max_replans {
            emit(
                &tx,
                OrchEvent::Replan {
                    reason,
                    prev_round: plan_round,
                },
            )
            .await;
            plan_round += 1;
            continue;
        }

        // 7. Replan cap reached.
        emit(
            &tx,
            OrchEvent::Final {
                stop_reason: TriangleStopReason::MaxReplansReached,
                summary: build_summary(plan_round, &approved_artefacts, &rejection_reasons),
            },
        )
        .await;
        return;
    }
}

// =====================================================================
// Helpers
// =====================================================================

#[derive(Debug, Clone)]
struct ApprovedArtefact {
    task_id: TaskId,
    artefact: Option<String>,
    approve_reason: String,
}

fn build_summary(last_round: u32, approved: &[ApprovedArtefact], rejections: &[String]) -> String {
    let mut s = String::with_capacity(256);
    s.push_str(&format!("plan_rounds_used={}; ", last_round + 1));
    s.push_str(&format!("approved={}; ", approved.len()));
    s.push_str(&format!("rejected={}", rejections.len()));
    for a in approved {
        s.push_str(&format!(
            "\n- approved task {}: {}",
            a.task_id, a.approve_reason
        ));
        if let Some(art) = &a.artefact {
            // Truncate to keep the summary bounded.
            let trimmed = art.trim();
            let preview = if trimmed.len() > 200 {
                format!("{}…", &trimmed[..200])
            } else {
                trimmed.to_string()
            };
            s.push_str(&format!("\n  artefact: {preview}"));
        }
    }
    for r in rejections {
        s.push_str(&format!("\n- {r}"));
    }
    s
}

async fn emit(tx: &mpsc::Sender<OrchEvent>, event: OrchEvent) {
    // If the receiver dropped, we silently stop emitting — the loop
    // continues to its natural termination so cleanup still runs.
    let _ = tx.send(event).await;
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::triangle::memory_view::InMemoryMemoryView;
    use crate::triangle::TriangleBudget;
    use std::sync::Arc;
    use tokio_stream::StreamExt;
    use xiaoguai_llm::mock::MockBackend;

    /// Smallest valid backend bundle — used by the budget-too-small
    /// test below. We never actually drive these because the runner
    /// returns before spawning anything.
    fn dummy_runner(parent_budget: u64) -> TriangleRunner {
        let planner_backend = Arc::new(MockBackend::with_response("{}"));
        let worker_backend = Arc::new(MockBackend::with_response("x"));
        let critic_backend = Arc::new(MockBackend::with_response("{}"));
        let planner = Arc::new(PlannerAgent::new(planner_backend, "p".into()));
        let worker = Arc::new(WorkerAgent::new(worker_backend, "w".into(), vec![]));
        let critic = Arc::new(CriticAgent::new(critic_backend, "c".into()));
        let memory = InMemoryMemoryView::new();
        TriangleRunner::new(
            planner,
            worker,
            critic,
            memory,
            TriangleBudget::DEFAULT,
            parent_budget,
            3,
            3,
        )
    }

    #[tokio::test]
    async fn budget_too_small_returns_planner_failed() {
        // 9 tokens × 10 % critic = 0 tokens — TriangleBudget::split rejects.
        let runner = dummy_runner(9);
        let mut stream = Box::pin(runner.stream(TriangleRequest {
            goal: "anything".into(),
            session_id: SessionId::new(),
        }));

        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }

        assert_eq!(events.len(), 1, "should emit exactly one Final event");
        match &events[0] {
            OrchEvent::Final {
                stop_reason: TriangleStopReason::PlannerFailed(msg),
                ..
            } => {
                assert!(
                    msg.contains("budget too small"),
                    "expected 'budget too small' in PlannerFailed msg, got: {msg}"
                );
            }
            other => panic!("expected Final {{ PlannerFailed }}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_id_round_trips_through_serde() {
        let s = SessionId::new();
        let json = serde_json::to_string(&s).unwrap();
        let back: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
