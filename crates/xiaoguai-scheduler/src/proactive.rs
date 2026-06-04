//! Proactive checker seam — the cheap-model gate in front of the
//! executor for [`crate::trigger::Trigger::Proactive`] jobs.
//!
//! The roadmap (§3 + §5.5) is explicit that proactive runs are the
//! *differentiating* feature and equally the most dangerous one: an
//! unchecked LLM-initiated push channel devolves into spam in a single
//! afternoon. The whole point of this trait is to keep the decision
//! "is now the right time to bother the user?" *outside* the scheduler
//! core, so v0.11.2 can swap in a real cheap-model call (Haiku /
//! 3B-local / equivalent) without touching `JobRunner`.
//!
//! Contract:
//!
//! * The runner calls [`ProactiveChecker::should_fire`] every time a
//!   proactive job comes due on its interval.
//! * Returning `Ok(Some(reason))` means "yes, run the executor and put
//!   `reason` in the push payload" — the reason is then visible in
//!   every push sink and in the audit row.
//! * Returning `Ok(None)` means "no, skip silently — don't run the
//!   executor, don't consume a budget slot, don't write a `JobRun`
//!   row, don't push anything". Only the audit row is suppressed for
//!   no-fire decisions: they would saturate the audit log under a fast
//!   check interval.
//! * Returning `Err(_)` means the checker itself failed (model
//!   unreachable, prompt too long, etc.). The runner logs and treats
//!   it as "skip this tick" — a flaky checker should not block the
//!   job's next interval.

use async_trait::async_trait;
use parking_lot::Mutex;
use thiserror::Error;

use crate::job::ScheduledJob;

#[derive(Debug, Error)]
pub enum ProactiveError {
    #[error("checker backend: {0}")]
    Backend(String),
    #[error("checker invalid: {0}")]
    Invalid(String),
}

/// Context handed to the checker on every tick. Kept narrow on
/// purpose — the checker should rely on the prompt, not rummage through
/// the full job row.
#[derive(Debug, Clone)]
pub struct ProactiveCtx {
    pub job_id: String,
}

impl ProactiveCtx {
    #[must_use]
    pub fn from_job(job: &ScheduledJob) -> Self {
        Self {
            job_id: job.id.clone(),
        }
    }
}

#[async_trait]
pub trait ProactiveChecker: Send + Sync {
    /// Decide whether the job should fire right now.
    ///
    /// `Some(reason)` ⇒ fire + put `reason` in the push payload.
    /// `None`         ⇒ skip this tick silently.
    async fn should_fire(
        &self,
        prompt: &str,
        ctx: ProactiveCtx,
    ) -> Result<Option<String>, ProactiveError>;
}

/// Always-skip checker. Production default until v0.11.2 wires a real
/// cheap-model backend — keeps proactive jobs inert by construction so
/// a user creating one before the model is configured doesn't get
/// surprise pushes.
#[derive(Debug, Default, Clone)]
pub struct NeverFireChecker;

#[async_trait]
impl ProactiveChecker for NeverFireChecker {
    async fn should_fire(
        &self,
        _prompt: &str,
        _ctx: ProactiveCtx,
    ) -> Result<Option<String>, ProactiveError> {
        Ok(None)
    }
}

/// Always-fire checker with a fixed reason. Useful for tests asserting
/// the budget / push-with-reason path.
#[derive(Debug, Clone)]
pub struct AlwaysFireChecker {
    reason: String,
}

impl AlwaysFireChecker {
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl ProactiveChecker for AlwaysFireChecker {
    async fn should_fire(
        &self,
        _prompt: &str,
        _ctx: ProactiveCtx,
    ) -> Result<Option<String>, ProactiveError> {
        Ok(Some(self.reason.clone()))
    }
}

/// Scripted checker for tests — a queue of decisions consumed in
/// order. Useful for "first tick says no, second says yes" scenarios.
#[derive(Debug, Default)]
pub struct ScriptedChecker {
    queue: Mutex<Vec<Result<Option<String>, ProactiveError>>>,
}

impl ScriptedChecker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push one decision onto the tail of the queue.
    pub fn enqueue(&self, decision: Result<Option<String>, ProactiveError>) {
        self.queue.lock().push(decision);
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        self.queue.lock().len()
    }
}

#[async_trait]
impl ProactiveChecker for ScriptedChecker {
    async fn should_fire(
        &self,
        _prompt: &str,
        _ctx: ProactiveCtx,
    ) -> Result<Option<String>, ProactiveError> {
        let mut g = self.queue.lock();
        if g.is_empty() {
            return Ok(None);
        }
        g.remove(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::Trigger;

    fn sample_job() -> ScheduledJob {
        ScheduledJob::new(
            "j1",
            "j1",
            Trigger::proactive("any news?", 60).unwrap(),
            serde_json::json!({}),
        )
    }

    #[tokio::test]
    async fn never_fire_returns_none() {
        let chk = NeverFireChecker;
        let got = chk
            .should_fire("p", ProactiveCtx::from_job(&sample_job()))
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn always_fire_returns_reason() {
        let chk = AlwaysFireChecker::new("inbox got 3 new emails");
        let got = chk
            .should_fire("p", ProactiveCtx::from_job(&sample_job()))
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("inbox got 3 new emails"));
    }

    #[tokio::test]
    async fn scripted_drains_in_order() {
        let chk = ScriptedChecker::new();
        chk.enqueue(Ok(None));
        chk.enqueue(Ok(Some("now".into())));
        chk.enqueue(Err(ProactiveError::Backend("boom".into())));

        let ctx = || ProactiveCtx::from_job(&sample_job());
        assert!(chk.should_fire("p", ctx()).await.unwrap().is_none());
        assert_eq!(
            chk.should_fire("p", ctx()).await.unwrap().as_deref(),
            Some("now")
        );
        assert!(chk.should_fire("p", ctx()).await.is_err());
        // Empty queue ⇒ default to None.
        assert!(chk.should_fire("p", ctx()).await.unwrap().is_none());
    }
}
