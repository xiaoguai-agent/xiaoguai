//! `ExecutiveRunner` — parallel member fan-out + lead synthesis.
//!
//! T4 of `docs/plans/2026-06-10-executive-orchestration.md` §2.1
//! (capability-upgrade second wave). Gives a team a real execution
//! model: **goal in → members run in parallel → lead synthesizes one
//! answer out**.
//!
//! ## Run shape
//!
//! ```text
//! RunStarted { members }
//! per member (CONCURRENT, join_all):
//!     MemberStarted { id }
//!     runner.run_member(member, goal)
//!     MemberCompleted { id, ok }          // Err(..) or ok:false => failed
//! if ≥1 member succeeded:
//!     SynthesisStarted { ok_members }
//!     runner.run_synthesis(lead, goal, survivors)   // original member order
//!     Final { ok: true, text, failed_members }
//! else:
//!     Final { ok: false, text: brief reason, failed_members: all }
//! ```
//!
//! ## Failure semantics
//!
//! - A member returning `Err(..)` **or** `Ok(MemberOutcome { ok: false, .. })`
//!   counts as failed but does NOT abort the run — synthesis receives only
//!   the survivors, **in original member order** (not completion order).
//! - If **all** members failed, synthesis is skipped and `Final { ok: false }`
//!   lists every member in `failed_members`.
//! - If `run_synthesis` itself errors, the run ends with
//!   `Final { ok: false, text: <synthesis error message>, .. }` — but
//!   `failed_members` stays **member failures only** (the lead is NOT
//!   appended; consumers distinguish the two cases by `ok == false` with
//!   a non-member-failure text).
//!
//! ## Isolation
//!
//! Members share nothing: each `run_member` call is independent (no shared
//! scratchpad — quarantine by construction, mirroring the triangle pattern's
//! §4.5 invariant). This module is LLM-free; the real agent-backed
//! `MemberRunner` lives in the API layer (T4.2).

use std::sync::Arc;

use async_trait::async_trait;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream};
use uuid::Uuid;

use crate::error::OrchestratorError;

// =====================================================================
// Constants
// =====================================================================

/// Default cap on team size, enforced at construction. Plan §2.1.
pub const DEFAULT_MAX_MEMBERS: usize = 8;

/// Channel capacity for the event stream. A capped-size team emits at
/// most `2 × members + 3` events, comfortably under 64; if the consumer
/// is slow the sender awaits backpressure. Mirrors `triangle.rs`.
const EVENT_CHANNEL_CAPACITY: usize = 64;

// =====================================================================
// Public types — specs, outcomes, events, config errors
// =====================================================================

/// One team member as seen by the runner. `name` is opaque to the
/// orchestrator — the `MemberRunner` impl maps it onto a persona.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberSpec {
    pub id: Uuid,
    pub name: String,
}

/// Result of one member's full agent turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberOutcome {
    pub id: Uuid,
    /// `false` marks a soft failure (the turn ran but did not produce a
    /// usable answer); treated the same as `Err` by the executive loop.
    pub ok: bool,
    pub text: String,
    pub iterations: u32,
}

/// Backend contract: how to run one member persona and how to run the
/// lead's synthesis turn. Implemented agent-backed in the API layer;
/// mocked in this crate's tests. This crate stays LLM-free.
#[async_trait]
pub trait MemberRunner: Send + Sync {
    /// Run one member persona against the goal; full agent turn, tools
    /// allowed.
    ///
    /// # Errors
    ///
    /// Returns an [`OrchestratorError`] when the member turn could not
    /// run at all (hard failure). The executive loop treats this the
    /// same as a soft `ok: false` outcome — the run continues.
    async fn run_member(
        &self,
        member: &MemberSpec,
        goal: &str,
    ) -> Result<MemberOutcome, OrchestratorError>;

    /// Run the lead synthesis turn over the members' outcomes.
    ///
    /// # Errors
    ///
    /// Returns an [`OrchestratorError`] when synthesis fails; the run
    /// ends with `Final { ok: false }` carrying the error message.
    async fn run_synthesis(
        &self,
        lead: &MemberSpec,
        goal: &str,
        outcomes: &[MemberOutcome],
    ) -> Result<String, OrchestratorError>;
}

/// One event from the executive run. The API layer SSE-encodes these
/// directly — serde tagging mirrors `xiaoguai-agent::AgentEvent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecEvent {
    RunStarted {
        members: usize,
    },
    MemberStarted {
        id: Uuid,
    },
    MemberCompleted {
        id: Uuid,
        ok: bool,
    },
    SynthesisStarted {
        ok_members: usize,
    },
    Final {
        ok: bool,
        text: String,
        /// Member failures only (see module docs — a synthesis error
        /// does NOT add the lead here).
        failed_members: Vec<Uuid>,
    },
}

/// Construction-time validation failures (plan §2.1: cap + lead
/// membership are enforced before any agent spawns). A dedicated enum
/// (vs [`OrchestratorError`]) keeps config errors distinguishable from
/// runtime ones, mirroring the crate's local-error convention
/// (`triangle::budget::BudgetError` precedent).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExecutiveConfigError {
    /// More members than the cap allows.
    #[error("too many members: {count} > max {max}")]
    TooManyMembers { count: usize, max: usize },

    /// The designated lead is not one of the members.
    #[error("lead {lead} is not a member of the team")]
    LeadNotMember { lead: Uuid },
}

// =====================================================================
// ExecutiveRunner
// =====================================================================

/// Stateless across runs — holds only configuration. `stream()`
/// consumes the runner and returns a fresh event stream backed by a
/// tokio task, mirroring [`super::triangle::TriangleRunner::stream`].
pub struct ExecutiveRunner<R: MemberRunner + 'static> {
    runner: Arc<R>,
    lead: MemberSpec,
    members: Vec<MemberSpec>,
}

// Manual impl — `R` need not be `Debug`; the runner field is elided.
impl<R: MemberRunner + 'static> std::fmt::Debug for ExecutiveRunner<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutiveRunner")
            .field("lead", &self.lead)
            .field("members", &self.members)
            .finish_non_exhaustive()
    }
}

impl<R: MemberRunner + 'static> ExecutiveRunner<R> {
    /// Construct with the default member cap ([`DEFAULT_MAX_MEMBERS`]).
    ///
    /// # Errors
    ///
    /// [`ExecutiveConfigError::TooManyMembers`] when `members` exceeds
    /// the cap; [`ExecutiveConfigError::LeadNotMember`] when `lead.id`
    /// is not present in `members`.
    pub fn new(
        runner: Arc<R>,
        lead: MemberSpec,
        members: Vec<MemberSpec>,
    ) -> Result<Self, ExecutiveConfigError> {
        Self::with_max_members(runner, lead, members, DEFAULT_MAX_MEMBERS)
    }

    /// Construct with an explicit member cap.
    ///
    /// # Errors
    ///
    /// Same as [`Self::new`], with `max_members` as the cap.
    pub fn with_max_members(
        runner: Arc<R>,
        lead: MemberSpec,
        members: Vec<MemberSpec>,
        max_members: usize,
    ) -> Result<Self, ExecutiveConfigError> {
        if members.len() > max_members {
            return Err(ExecutiveConfigError::TooManyMembers {
                count: members.len(),
                max: max_members,
            });
        }
        if !members.iter().any(|m| m.id == lead.id) {
            return Err(ExecutiveConfigError::LeadNotMember { lead: lead.id });
        }
        Ok(Self {
            runner,
            lead,
            members,
        })
    }

    /// Drive the run and return a stream of [`ExecEvent`]s. The stream
    /// completes after a terminal `Final` event. Members run
    /// concurrently (`join_all`); see the module docs for the exact
    /// event/failure semantics.
    pub fn stream(self, goal: String) -> impl Stream<Item = ExecEvent> + Send + 'static {
        let (tx, rx) = mpsc::channel::<ExecEvent>(EVENT_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            run_executive(self.runner, self.lead, self.members, goal, tx).await;
        });

        ReceiverStream::new(rx)
    }
}

// =====================================================================
// Run body
// =====================================================================

async fn run_executive<R: MemberRunner>(
    runner: Arc<R>,
    lead: MemberSpec,
    members: Vec<MemberSpec>,
    goal: String,
    tx: mpsc::Sender<ExecEvent>,
) {
    emit(
        &tx,
        ExecEvent::RunStarted {
            members: members.len(),
        },
    )
    .await;

    // Fan out: one future per member, all driven concurrently by
    // join_all. Each per-member block sends its own Started/Completed
    // events through a channel clone, so completion events arrive in
    // completion order while join_all's result vector preserves the
    // original member order (the order synthesis must see).
    let member_futures = members.iter().map(|member| {
        let runner = Arc::clone(&runner);
        let tx = tx.clone();
        let goal = goal.as_str();
        async move {
            emit(&tx, ExecEvent::MemberStarted { id: member.id }).await;
            let outcome = match runner.run_member(member, goal).await {
                Ok(outcome) => outcome,
                // Hard failure → synthesized soft-failure outcome so the
                // run continues; the error message is preserved as text
                // for audit/debugging downstream.
                Err(e) => MemberOutcome {
                    id: member.id,
                    ok: false,
                    text: e.to_string(),
                    iterations: 0,
                },
            };
            emit(
                &tx,
                ExecEvent::MemberCompleted {
                    id: member.id,
                    ok: outcome.ok,
                },
            )
            .await;
            outcome
        }
    });
    let outcomes: Vec<MemberOutcome> = join_all(member_futures).await;

    let (survivors, failed): (Vec<MemberOutcome>, Vec<MemberOutcome>) =
        outcomes.into_iter().partition(|o| o.ok);
    let failed_members: Vec<Uuid> = failed.iter().map(|o| o.id).collect();

    // All failed → skip synthesis entirely.
    if survivors.is_empty() {
        emit(
            &tx,
            ExecEvent::Final {
                ok: false,
                text: "all members failed; nothing to synthesize".to_string(),
                failed_members,
            },
        )
        .await;
        return;
    }

    emit(
        &tx,
        ExecEvent::SynthesisStarted {
            ok_members: survivors.len(),
        },
    )
    .await;

    // Synthesis over survivors only, original member order (join_all
    // + stable partition preserve it).
    let final_event = match runner.run_synthesis(&lead, &goal, &survivors).await {
        Ok(text) => ExecEvent::Final {
            ok: true,
            text,
            failed_members,
        },
        // Synthesis error: ok:false with the error message as text;
        // failed_members stays member-failures-only (module docs).
        Err(e) => ExecEvent::Final {
            ok: false,
            text: e.to_string(),
            failed_members,
        },
    };
    emit(&tx, final_event).await;
}

/// If the receiver dropped, silently stop emitting — the run continues
/// to its natural termination so cleanup still happens (mirrors
/// `triangle.rs::emit`).
async fn emit(tx: &mpsc::Sender<ExecEvent>, event: ExecEvent) {
    let _ = tx.send(event).await;
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use parking_lot::Mutex;
    use tokio_stream::StreamExt;
    use uuid::Uuid;

    use crate::error::OrchestratorError;

    /// Per-member scripted behavior for the mock runner.
    #[derive(Debug, Clone)]
    enum Behavior {
        /// `Ok(MemberOutcome { ok: true, .. })` after `delay_ms`.
        Succeed { delay_ms: u64, text: &'static str },
        /// `Ok(MemberOutcome { ok: false, .. })` after `delay_ms`.
        Fail { delay_ms: u64, text: &'static str },
        /// `Err(OrchestratorError::WorkerFailed)` after `delay_ms`.
        Error { delay_ms: u64, msg: &'static str },
    }

    /// Mock `MemberRunner` — scripted per-member results + delays, and
    /// a recording of every `run_synthesis` input for assertions.
    struct MockRunner {
        behaviors: HashMap<Uuid, Behavior>,
        synthesis: Result<&'static str, &'static str>,
        synthesis_inputs: Mutex<Vec<Vec<MemberOutcome>>>,
    }

    impl MockRunner {
        fn new(
            behaviors: HashMap<Uuid, Behavior>,
            synthesis: Result<&'static str, &'static str>,
        ) -> Self {
            Self {
                behaviors,
                synthesis,
                synthesis_inputs: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl MemberRunner for MockRunner {
        async fn run_member(
            &self,
            member: &MemberSpec,
            _goal: &str,
        ) -> Result<MemberOutcome, OrchestratorError> {
            let behavior = self
                .behaviors
                .get(&member.id)
                .unwrap_or_else(|| panic!("no scripted behavior for member {}", member.id))
                .clone();
            match behavior {
                Behavior::Succeed { delay_ms, text } => {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    Ok(MemberOutcome {
                        id: member.id,
                        ok: true,
                        text: text.to_string(),
                        iterations: 1,
                    })
                }
                Behavior::Fail { delay_ms, text } => {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    Ok(MemberOutcome {
                        id: member.id,
                        ok: false,
                        text: text.to_string(),
                        iterations: 1,
                    })
                }
                Behavior::Error { delay_ms, msg } => {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    Err(OrchestratorError::WorkerFailed(msg.to_string()))
                }
            }
        }

        async fn run_synthesis(
            &self,
            _lead: &MemberSpec,
            _goal: &str,
            outcomes: &[MemberOutcome],
        ) -> Result<String, OrchestratorError> {
            self.synthesis_inputs.lock().push(outcomes.to_vec());
            match self.synthesis {
                Ok(text) => Ok(text.to_string()),
                Err(msg) => Err(OrchestratorError::Internal(msg.to_string())),
            }
        }
    }

    fn member(name: &str) -> MemberSpec {
        MemberSpec {
            id: Uuid::new_v4(),
            name: name.to_string(),
        }
    }

    async fn collect(runner: ExecutiveRunner<MockRunner>, goal: &str) -> Vec<ExecEvent> {
        let mut stream = Box::pin(runner.stream(goal.to_string()));
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }
        events
    }

    fn position_of(events: &[ExecEvent], pred: impl Fn(&ExecEvent) -> bool) -> usize {
        events
            .iter()
            .position(pred)
            .unwrap_or_else(|| panic!("expected event not found in {events:?}"))
    }

    // (1) Happy path: RunStarted first, 3 Started + 3 Completed (each
    //     Started before its Completed), SynthesisStarted, Final ok
    //     with the synthesized text; synthesis got all 3 outcomes in
    //     original member order.
    #[tokio::test]
    async fn happy_path_three_members_event_order() {
        let (a, b, c) = (member("a"), member("b"), member("c"));
        let behaviors = HashMap::from([
            (
                a.id,
                Behavior::Succeed {
                    delay_ms: 0,
                    text: "ra",
                },
            ),
            (
                b.id,
                Behavior::Succeed {
                    delay_ms: 0,
                    text: "rb",
                },
            ),
            (
                c.id,
                Behavior::Succeed {
                    delay_ms: 0,
                    text: "rc",
                },
            ),
        ]);
        let mock = Arc::new(MockRunner::new(behaviors, Ok("synth")));
        let runner = ExecutiveRunner::new(
            mock.clone(),
            a.clone(),
            vec![a.clone(), b.clone(), c.clone()],
        )
        .expect("valid config");

        let events = collect(runner, "goal").await;

        assert_eq!(events[0], ExecEvent::RunStarted { members: 3 });
        for m in [&a, &b, &c] {
            let started = position_of(
                &events,
                |e| matches!(e, ExecEvent::MemberStarted { id } if *id == m.id),
            );
            let completed = position_of(
                &events,
                |e| matches!(e, ExecEvent::MemberCompleted { id, ok: true } if *id == m.id),
            );
            assert!(
                started < completed,
                "member {} started after completed",
                m.name
            );
        }
        let synth = position_of(&events, |e| {
            matches!(e, ExecEvent::SynthesisStarted { ok_members: 3 })
        });
        let last_completed = events
            .iter()
            .rposition(|e| matches!(e, ExecEvent::MemberCompleted { .. }))
            .unwrap();
        assert!(
            synth > last_completed,
            "synthesis must start after all members"
        );
        assert_eq!(
            events.last().unwrap(),
            &ExecEvent::Final {
                ok: true,
                text: "synth".to_string(),
                failed_members: vec![],
            }
        );

        let inputs = mock.synthesis_inputs.lock();
        assert_eq!(inputs.len(), 1);
        let ids: Vec<Uuid> = inputs[0].iter().map(|o| o.id).collect();
        assert_eq!(
            ids,
            vec![a.id, b.id, c.id],
            "original member order preserved"
        );
    }

    // (2) Genuine concurrency: staggered delays complete out of start
    //     order and total wall-clock ≈ max(delays), not the sum.
    #[tokio::test(start_paused = true)]
    async fn members_run_concurrently() {
        let (a, b, c) = (member("a"), member("b"), member("c"));
        let behaviors = HashMap::from([
            (
                a.id,
                Behavior::Succeed {
                    delay_ms: 30,
                    text: "ra",
                },
            ),
            (
                b.id,
                Behavior::Succeed {
                    delay_ms: 10,
                    text: "rb",
                },
            ),
            (
                c.id,
                Behavior::Succeed {
                    delay_ms: 20,
                    text: "rc",
                },
            ),
        ]);
        let mock = Arc::new(MockRunner::new(behaviors, Ok("synth")));
        let runner = ExecutiveRunner::new(mock, a.clone(), vec![a.clone(), b.clone(), c.clone()])
            .expect("valid config");

        let started_at = tokio::time::Instant::now();
        let events = collect(runner, "goal").await;
        let elapsed = started_at.elapsed();

        // Concurrent: bounded by max(30, 10, 20)ms, far below the
        // 60 ms serial sum (paused clock => deterministic).
        assert!(
            elapsed < Duration::from_millis(60),
            "expected concurrent execution, took {elapsed:?}"
        );

        // Completion order follows the delays (b, c, a), not the start order.
        let completed_ids: Vec<Uuid> = events
            .iter()
            .filter_map(|e| match e {
                ExecEvent::MemberCompleted { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(completed_ids, vec![b.id, c.id, a.id]);
    }

    // (3) Partial failure: synthesis receives only the survivors in
    //     original member order; Final ok with the failed member listed.
    #[tokio::test]
    async fn partial_failure_synthesizes_survivors_in_order() {
        let (a, b, c) = (member("a"), member("b"), member("c"));
        let behaviors = HashMap::from([
            (
                a.id,
                Behavior::Succeed {
                    delay_ms: 0,
                    text: "ra",
                },
            ),
            (
                b.id,
                Behavior::Fail {
                    delay_ms: 0,
                    text: "boom",
                },
            ),
            (
                c.id,
                Behavior::Succeed {
                    delay_ms: 0,
                    text: "rc",
                },
            ),
        ]);
        let mock = Arc::new(MockRunner::new(behaviors, Ok("synth")));
        let runner = ExecutiveRunner::new(
            mock.clone(),
            a.clone(),
            vec![a.clone(), b.clone(), c.clone()],
        )
        .expect("valid config");

        let events = collect(runner, "goal").await;

        position_of(
            &events,
            |e| matches!(e, ExecEvent::MemberCompleted { id, ok: false } if *id == b.id),
        );
        position_of(&events, |e| {
            matches!(e, ExecEvent::SynthesisStarted { ok_members: 2 })
        });
        assert_eq!(
            events.last().unwrap(),
            &ExecEvent::Final {
                ok: true,
                text: "synth".to_string(),
                failed_members: vec![b.id],
            }
        );

        let inputs = mock.synthesis_inputs.lock();
        let ids: Vec<Uuid> = inputs[0].iter().map(|o| o.id).collect();
        assert_eq!(ids, vec![a.id, c.id], "survivors only, original order");
    }

    // (4) All members fail (mix of Err and ok:false): no synthesis,
    //     Final ok:false with every member listed as failed.
    #[tokio::test]
    async fn all_fail_skips_synthesis() {
        let (a, b) = (member("a"), member("b"));
        let behaviors = HashMap::from([
            (
                a.id,
                Behavior::Error {
                    delay_ms: 0,
                    msg: "agent died",
                },
            ),
            (
                b.id,
                Behavior::Fail {
                    delay_ms: 0,
                    text: "no answer",
                },
            ),
        ]);
        let mock = Arc::new(MockRunner::new(behaviors, Ok("never")));
        let runner = ExecutiveRunner::new(mock.clone(), a.clone(), vec![a.clone(), b.clone()])
            .expect("valid config");

        let events = collect(runner, "goal").await;

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ExecEvent::SynthesisStarted { .. })),
            "no synthesis when all members failed"
        );
        match events.last().unwrap() {
            ExecEvent::Final {
                ok: false,
                text,
                failed_members,
            } => {
                assert!(!text.is_empty(), "Final carries a brief reason");
                assert_eq!(failed_members, &vec![a.id, b.id]);
            }
            other => panic!("expected Final {{ ok: false }}, got {other:?}"),
        }
        assert!(
            mock.synthesis_inputs.lock().is_empty(),
            "run_synthesis must not be called"
        );
    }

    // (5) Member cap: 9 members with the default cap (8) → constructor error.
    #[tokio::test]
    async fn member_cap_rejected_at_construction() {
        let members: Vec<MemberSpec> = (0..9).map(|i| member(&format!("m{i}"))).collect();
        let lead = members[0].clone();
        let mock = Arc::new(MockRunner::new(HashMap::new(), Ok("never")));

        let err = ExecutiveRunner::new(mock, lead, members).expect_err("cap must reject");
        assert!(
            matches!(
                err,
                ExecutiveConfigError::TooManyMembers { count: 9, max: 8 }
            ),
            "got {err:?}"
        );
    }

    // (6) Lead not in members → constructor error.
    #[tokio::test]
    async fn lead_not_in_members_rejected_at_construction() {
        let (a, b) = (member("a"), member("b"));
        let outsider = member("outsider");
        let mock = Arc::new(MockRunner::new(HashMap::new(), Ok("never")));

        let err = ExecutiveRunner::new(mock, outsider.clone(), vec![a, b])
            .expect_err("lead must be a member");
        assert!(
            matches!(&err, ExecutiveConfigError::LeadNotMember { lead } if *lead == outsider.id),
            "got {err:?}"
        );
    }

    // (7) Synthesis error: Final ok:false carrying the error text;
    //     failed_members stays member-failures-only (empty here).
    #[tokio::test]
    async fn synthesis_error_yields_final_not_ok() {
        let (a, b) = (member("a"), member("b"));
        let behaviors = HashMap::from([
            (
                a.id,
                Behavior::Succeed {
                    delay_ms: 0,
                    text: "ra",
                },
            ),
            (
                b.id,
                Behavior::Succeed {
                    delay_ms: 0,
                    text: "rb",
                },
            ),
        ]);
        let mock = Arc::new(MockRunner::new(behaviors, Err("synth boom")));
        let runner = ExecutiveRunner::new(mock, a.clone(), vec![a.clone(), b.clone()])
            .expect("valid config");

        let events = collect(runner, "goal").await;

        position_of(&events, |e| {
            matches!(e, ExecEvent::SynthesisStarted { ok_members: 2 })
        });
        match events.last().unwrap() {
            ExecEvent::Final {
                ok: false,
                text,
                failed_members,
            } => {
                assert!(
                    text.contains("synth boom"),
                    "Final text must carry the synthesis error, got: {text}"
                );
                assert!(
                    failed_members.is_empty(),
                    "failed_members is member failures only"
                );
            }
            other => panic!("expected Final {{ ok: false }}, got {other:?}"),
        }
    }
}
