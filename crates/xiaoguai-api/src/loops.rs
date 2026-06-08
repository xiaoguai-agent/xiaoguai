//! `LoopController` — drives /loop session-scoped recurring agent turns
//! (DEC-039 / LLD-LOOP-001 §4).
//!
//! Mirrors the `HotL` decision-registry shape: per-row spawned companion
//! tasks (one driver per active loop) + boot replay that re-arms unexpired
//! rows from the `loops` table, so loops survive a server restart with the
//! same semantics as `HotL` escalation replay.
//!
//! Every tick is a normal agent turn through [`crate::turn::run_turn`]:
//! the per-session turn lock means a tick can never interleave with an
//! operator's in-flight message — a colliding tick is **skipped, not
//! queued** (LLD §4). Failure semantics (LLD §3, review H2): a tick whose
//! turn errors backs off exponentially (`interval × 2^failures`); after
//! [`MAX_CONSECUTIVE_FAILURES`] the loop terminalises as `failed` with a
//! final session message naming the last error. A tick against a gone
//! session terminalises the loop as `cancelled` (`reason: session_gone`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use xiaoguai_llm::Message as LlmMessage;
use xiaoguai_storage::repositories::{
    LoopRow, LoopStatus, LoopStore, PacingKind, RepoError, TokenUsageRepository,
};
use xiaoguai_types::{SessionId, SessionStatus};

use crate::convert::llm_to_domain;
use crate::state::AppState;
use crate::turn::{run_turn, TurnCompletion, TurnError, TurnInput};

/// After this many consecutive tick failures the loop auto-terminalises
/// as `failed` (mirrors the `LlmRouter` breaker precedent, LLD §3).
pub const MAX_CONSECUTIVE_FAILURES: u32 = 5;

/// Budget defaults (LLD §3) — conservative on purpose.
pub const DEFAULT_INTERVAL_SECS: u32 = 300;
pub const DEFAULT_MAX_TICKS: u32 = 50;
pub const DEFAULT_TTL_SECS: u32 = 86_400;
/// Dynamic-pacing clamp window defaults (L3 Part B).
pub const DEFAULT_MIN_INTERVAL_SECS: u32 = 10;
pub const DEFAULT_MAX_INTERVAL_SECS: u32 = 3600;
/// Token budget default (L3 Part C, LLD §3): 500k tokens / loop.
pub const DEFAULT_MAX_TOTAL_TOKENS: u64 = 500_000;

/// Inputs for [`LoopController::create`]. Budget fields fall back to the
/// defaults above.
#[derive(Debug)]
pub struct CreateLoopParams {
    pub session_id: String,
    pub prompt: String,
    pub interval_secs: Option<u32>,
    pub max_ticks: Option<u32>,
    pub ttl_secs: Option<u32>,
    /// L3 Part B: when `true`, the agent paces the loop via `loop_next_tick`
    /// (clamped to `[min_interval_secs, max_interval_secs]`); otherwise the
    /// fixed `interval_secs` is used. Default `false`.
    pub dynamic_pacing: bool,
    pub min_interval_secs: Option<u32>,
    pub max_interval_secs: Option<u32>,
    /// L3 Part C: stop once the session burns this many tokens since
    /// loop-start. `Some(0)` / `None` → the 500k default; explicit `0`
    /// after clamping means unlimited.
    pub max_total_tokens: Option<u64>,
    /// Audit actor; falls back to the session owner.
    pub created_by: Option<String>,
}

#[derive(Debug, Error)]
pub enum CreateLoopError {
    #[error("session not found")]
    SessionNotFound,
    #[error("session is not active")]
    SessionNotActive,
    /// v1 one-per-session constraint: cancel the existing loop first.
    #[error("session already has a live loop ({existing})")]
    AlreadyExists { existing: Uuid },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error(transparent)]
    Repo(#[from] RepoError),
}

#[derive(Debug, Error)]
pub enum CancelLoopError {
    #[error("loop not found")]
    NotFound,
    #[error("loop is already terminal ({0})")]
    AlreadyTerminal(String),
    #[error(transparent)]
    Repo(#[from] RepoError),
}

#[derive(Debug, Error)]
pub enum ResumeLoopError {
    #[error("loop not found")]
    NotFound,
    /// Only a `paused` loop can resume (active loops are already running;
    /// terminal loops are immutable).
    #[error("loop is not paused (status: {0})")]
    NotPaused(String),
    #[error(transparent)]
    Repo(#[from] RepoError),
}

/// One tick's disposition — drives the bookkeeping in the driver loop.
#[derive(Debug)]
enum TickOutcome {
    /// Turn ran and its finalize completed.
    Success,
    /// The agent called `loop_done` — goal met, terminalise as `done`.
    Done(String),
    /// The agent called `loop_pause` — stop ticking, keep the loop paused.
    Paused(String),
    /// Turn failed to start (HOTL deny, repo error) or errored/panicked
    /// at runtime.
    Failure(String),
    /// A turn was already in flight on the session — skipped, not queued.
    Skipped,
    /// The session is archived or deleted — terminalise the loop.
    SessionGone,
    /// The loop was cancelled while the tick was waiting/running.
    LoopCancelled,
}

struct Inner {
    store: Arc<dyn LoopStore>,
    /// State clone for `run_turn` + audit + final messages. Captured
    /// before the controller is inserted into the served `AppState`, so
    /// its own `loops` field is `None` — the controller never re-enters
    /// itself.
    state: AppState,
    /// Token-usage source for the L3 `max_total_tokens` budget. `None`
    /// disables the token gate (the budget is simply not enforced) — used
    /// by tests that don't wire a usage ledger.
    token_usage: Option<Arc<dyn TokenUsageRepository>>,
    /// Live driver cancel tokens, keyed by loop id.
    drivers: Mutex<HashMap<Uuid, CancellationToken>>,
}

impl Inner {
    /// Sum the session's tokens since `since`. Returns 0 (no enforcement)
    /// when no usage ledger is wired.
    async fn store_tokens_since(
        &self,
        session_id: &str,
        since: DateTime<Utc>,
    ) -> Result<u64, RepoError> {
        match &self.token_usage {
            Some(repo) => {
                let total = repo.session_total_since(session_id, since).await?;
                Ok(u64::try_from(total.max(0)).unwrap_or(u64::MAX))
            }
            None => Ok(0),
        }
    }
}

/// See module docs. Construct with [`LoopController::new`], then
/// [`LoopController::replay_from_storage`] once at boot.
pub struct LoopController {
    inner: Arc<Inner>,
}

impl LoopController {
    /// Construct with an optional token-usage ledger for the L3 budget
    /// (`None` disables the `max_total_tokens` gate).
    #[must_use]
    pub fn new(
        store: Arc<dyn LoopStore>,
        state: AppState,
        token_usage: Option<Arc<dyn TokenUsageRepository>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Inner {
                store,
                state,
                token_usage,
                drivers: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Re-arm every `active` loop after a restart (LLD §4 — same
    /// boot-replay semantics as `HotL` escalation replay). Rows whose ttl
    /// expired while the server was down are terminalised as
    /// `budget_exhausted` instead of armed.
    ///
    /// Returns `(armed, expired)` counts for the boot log.
    ///
    /// # Errors
    /// Returns an error when the initial `list_active` scan fails; per-row
    /// failures are logged and skipped (one bad row must not block boot).
    pub async fn replay_from_storage(&self) -> Result<(usize, usize), RepoError> {
        let rows = self.inner.store.list_active().await?;
        let now = Utc::now();
        let (mut armed, mut expired) = (0usize, 0usize);
        for row in rows {
            if row.expires_at <= now {
                expired += 1;
                exhaust(&self.inner, &row, "ttl").await;
            } else {
                armed += 1;
                self.arm(row);
            }
        }
        tracing::info!(armed, expired, "loop controller replay complete");
        Ok((armed, expired))
    }

    /// Create + persist + arm a new loop. The first tick fires one
    /// interval after creation (the operator just talked to the session —
    /// an immediate tick would collide with their in-flight turn).
    ///
    /// # Errors
    /// See [`CreateLoopError`].
    pub async fn create(&self, params: CreateLoopParams) -> Result<LoopRow, CreateLoopError> {
        let prompt = params.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(CreateLoopError::InvalidArgument(
                "prompt must be non-empty".into(),
            ));
        }
        let interval_secs = params.interval_secs.unwrap_or(DEFAULT_INTERVAL_SECS);
        let max_ticks = params.max_ticks.unwrap_or(DEFAULT_MAX_TICKS);
        let ttl_secs = params.ttl_secs.unwrap_or(DEFAULT_TTL_SECS);
        if interval_secs == 0 || max_ticks == 0 || ttl_secs == 0 {
            return Err(CreateLoopError::InvalidArgument(
                "interval_secs, max_ticks and ttl_secs must be >= 1".into(),
            ));
        }
        // Dynamic-pacing bounds (L3 Part B). Default the window around the
        // tick interval; validate min ≤ interval ≤ max so the clamp is sane.
        let min_interval_secs = params
            .min_interval_secs
            .unwrap_or(DEFAULT_MIN_INTERVAL_SECS);
        let max_interval_secs = params
            .max_interval_secs
            .unwrap_or(DEFAULT_MAX_INTERVAL_SECS);
        if min_interval_secs == 0 || min_interval_secs > max_interval_secs {
            return Err(CreateLoopError::InvalidArgument(
                "min_interval_secs must be >= 1 and <= max_interval_secs".into(),
            ));
        }
        let pacing_kind = if params.dynamic_pacing {
            PacingKind::Dynamic
        } else {
            PacingKind::Fixed
        };
        let max_total_tokens = params.max_total_tokens.unwrap_or(DEFAULT_MAX_TOTAL_TOKENS);

        let session = self
            .inner
            .state
            .sessions
            .find_by_id(&params.session_id)
            .await
            .map_err(CreateLoopError::Repo)?
            .ok_or(CreateLoopError::SessionNotFound)?;
        if !matches!(session.status, SessionStatus::Active) {
            return Err(CreateLoopError::SessionNotActive);
        }

        let now = Utc::now();
        let row = LoopRow {
            id: Uuid::new_v4(),
            session_id: params.session_id,
            prompt,
            pacing_kind,
            interval_secs,
            min_interval_secs,
            max_interval_secs,
            max_ticks,
            ttl_secs,
            max_total_tokens,
            status: LoopStatus::Active,
            created_by: params
                .created_by
                .unwrap_or_else(|| session.user_id.to_string()),
            created_at: now,
            expires_at: now + Duration::seconds(i64::from(ttl_secs)),
            next_tick_at: now + Duration::seconds(i64::from(interval_secs)),
            ticks_run: 0,
            consecutive_failures: 0,
            last_error: None,
        };

        match self.inner.store.insert(&row).await {
            Ok(()) => {}
            Err(RepoError::DuplicateKey(_)) => {
                // One-per-session (v1). Surface the holder's id so the
                // operator can cancel it (teaching error).
                let existing = match self.inner.store.find_live_by_session(&row.session_id).await {
                    Ok(Some(l)) => l.id,
                    Ok(None) => Uuid::nil(), // raced away between insert + lookup
                    Err(e) => {
                        tracing::warn!(session_id = %row.session_id, ?e, "find_live_by_session failed resolving the existing-loop id");
                        Uuid::nil()
                    }
                };
                return Err(CreateLoopError::AlreadyExists { existing });
            }
            Err(e) => return Err(CreateLoopError::Repo(e)),
        }

        append_loop_audit(
            &self.inner.state,
            &row.created_by,
            "loop.create",
            row.id,
            serde_json::json!({
                "session_id": row.session_id,
                "interval_secs": interval_secs,
                "max_ticks": max_ticks,
                "ttl_secs": ttl_secs,
            }),
        )
        .await;

        self.arm(row.clone());
        Ok(row)
    }

    /// All loops, newest first (terminal rows included).
    ///
    /// # Errors
    /// Propagates store failures.
    pub async fn list(&self) -> Result<Vec<LoopRow>, RepoError> {
        self.inner.store.list().await
    }

    /// # Errors
    /// Propagates store failures.
    pub async fn get(&self, id: Uuid) -> Result<Option<LoopRow>, RepoError> {
        self.inner.store.get(id).await
    }

    /// Cancel a live loop: terminalise the row, stop its driver, audit.
    ///
    /// # Errors
    /// See [`CancelLoopError`].
    pub async fn cancel(&self, id: Uuid, cancelled_by: &str) -> Result<LoopRow, CancelLoopError> {
        let row = self
            .inner
            .store
            .get(id)
            .await
            .map_err(CancelLoopError::Repo)?
            .ok_or(CancelLoopError::NotFound)?;
        let moved = self
            .inner
            .store
            .terminalise(id, LoopStatus::Cancelled, Some("operator cancel"))
            .await
            .map_err(CancelLoopError::Repo)?;
        if !moved {
            // Already terminal (double cancel, or a driver raced an
            // auto-terminalise between our get and terminalise). Re-fetch so
            // the reported status is the row's ACTUAL terminal state, not the
            // possibly-stale `active` we read above.
            let current = self
                .inner
                .store
                .get(id)
                .await
                .map_err(CancelLoopError::Repo)?
                .map_or(LoopStatus::Cancelled, |r| r.status);
            return Err(CancelLoopError::AlreadyTerminal(
                current.as_str().to_string(),
            ));
        }
        if let Some(token) = self.inner.drivers.lock().remove(&id) {
            token.cancel();
        }
        append_loop_audit(
            &self.inner.state,
            cancelled_by,
            "loop.cancel",
            id,
            serde_json::json!({ "session_id": row.session_id, "ticks_run": row.ticks_run }),
        )
        .await;
        self.inner
            .store
            .get(id)
            .await?
            .ok_or(CancelLoopError::NotFound)
    }

    /// Resume a `paused` loop: move it back to `active`, re-arm its driver
    /// with a fresh next-tick one interval out, and audit. The agent's
    /// `loop_pause` is undone by an operator here (LLD-LOOP-001 §3 — the
    /// missing half of the pause/resume pair).
    ///
    /// # Errors
    /// See [`ResumeLoopError`].
    pub async fn resume(&self, id: Uuid, resumed_by: &str) -> Result<LoopRow, ResumeLoopError> {
        let row = self
            .inner
            .store
            .get(id)
            .await
            .map_err(ResumeLoopError::Repo)?
            .ok_or(ResumeLoopError::NotFound)?;
        if row.status != LoopStatus::Paused {
            return Err(ResumeLoopError::NotPaused(row.status.as_str().to_string()));
        }
        let next_tick_at = Utc::now() + Duration::seconds(i64::from(row.interval_secs));
        let moved = self
            .inner
            .store
            .resume(id, next_tick_at)
            .await
            .map_err(ResumeLoopError::Repo)?;
        if !moved {
            // Raced (cancelled between our get and resume). Report the
            // actual current status.
            let current = self
                .inner
                .store
                .get(id)
                .await
                .map_err(ResumeLoopError::Repo)?
                .map_or_else(
                    || "cancelled".to_string(),
                    |r| r.status.as_str().to_string(),
                );
            return Err(ResumeLoopError::NotPaused(current));
        }
        append_loop_audit(
            &self.inner.state,
            resumed_by,
            "loop.resume",
            id,
            serde_json::json!({ "session_id": row.session_id, "ticks_run": row.ticks_run }),
        )
        .await;
        // Re-arm the driver against the freshened row.
        let mut resumed = row;
        resumed.status = LoopStatus::Active;
        resumed.next_tick_at = next_tick_at;
        resumed.consecutive_failures = 0;
        resumed.last_error = None;
        let armed_id = resumed.id;
        self.arm(resumed);
        self.inner
            .store
            .get(armed_id)
            .await?
            .ok_or(ResumeLoopError::NotFound)
    }

    /// Spawn the per-loop driver task (no-op when one is already running).
    fn arm(&self, row: LoopRow) {
        let token = CancellationToken::new();
        {
            let mut drivers = self.inner.drivers.lock();
            if drivers.contains_key(&row.id) {
                tracing::warn!(loop_id = %row.id, "driver already armed — skipping");
                return;
            }
            drivers.insert(row.id, token.clone());
        }
        let inner = Arc::clone(&self.inner);
        tokio::spawn(drive(inner, row, token));
    }
}

impl std::fmt::Debug for LoopController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopController")
            .field("drivers", &self.inner.drivers.lock().len())
            .finish_non_exhaustive()
    }
}

/// Removes the driver's `drivers` map entry on drop — so a panic anywhere
/// in the driver body still frees the slot (otherwise a stale entry would
/// block re-arming until a restart).
struct DriverSlot {
    inner: Arc<Inner>,
    id: Uuid,
}

impl Drop for DriverSlot {
    fn drop(&mut self) {
        self.inner.drivers.lock().remove(&self.id);
    }
}

/// The per-loop companion task: sleep → tick → bookkeeping, until a
/// budget trips, the failure breaker fires, the session goes away, or the
/// loop is cancelled.
async fn drive(inner: Arc<Inner>, mut row: LoopRow, cancel: CancellationToken) {
    // Frees the `drivers` entry on every exit path, panics included.
    let _slot = DriverSlot {
        inner: Arc::clone(&inner),
        id: row.id,
    };
    loop {
        // Budget gates (LLD §5 — blunt backstops, checked before sleeping
        // so an already-exhausted row never fires another tick).
        if row.ticks_run >= row.max_ticks {
            exhaust(&inner, &row, "max_ticks").await;
            break;
        }
        if Utc::now() >= row.expires_at {
            exhaust(&inner, &row, "ttl").await;
            break;
        }
        // Token budget (L3 Part C): stop once the session has burned
        // max_total_tokens since loop-start. `0` = unlimited. A query error
        // is logged and the tick proceeds (better to slightly overshoot than
        // to wedge the loop on a transient DB blip).
        if row.max_total_tokens > 0 {
            match inner
                .store_tokens_since(&row.session_id, row.created_at)
                .await
            {
                Ok(spent) if spent >= row.max_total_tokens => {
                    exhaust_tokens(&inner, &row, spent).await;
                    break;
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(loop_id = %row.id, ?err, "token budget check failed — proceeding");
                }
            }
        }

        let wait = sleep_duration(row.next_tick_at, Utc::now());
        tokio::select! {
            () = cancel.cancelled() => break, // store already terminalised by cancel()
            () = tokio::time::sleep(wait) => {}
        }
        if Utc::now() >= row.expires_at {
            exhaust(&inner, &row, "ttl").await;
            break;
        }

        let (outcome, requested_delay) = fire_tick(&inner, &row, &cancel).await;
        match outcome {
            TickOutcome::Success => {
                row.ticks_run += 1;
                row.consecutive_failures = 0;
                row.last_error = None;
            }
            TickOutcome::Done(reason) => {
                // The agent called `loop_done` — goal met (LLD §3). The tick
                // itself ran (and posted its own final summary as part of the
                // turn), so persist the tick count first, then terminalise.
                row.ticks_run += 1;
                row.consecutive_failures = 0;
                persist_tick_count(&inner, &row).await;
                let detail = if reason.is_empty() {
                    "loop_done".to_string()
                } else {
                    format!("loop_done: {reason}")
                };
                terminalise_with_audit(
                    &inner,
                    &row,
                    LoopStatus::Done,
                    &detail,
                    serde_json::json!({ "reason": reason, "ticks_run": row.ticks_run }),
                )
                .await;
                break;
            }
            TickOutcome::Paused(reason) => {
                // The agent called `loop_pause` — stop ticking, keep the row
                // (an operator resumes or cancels it). `pause` is the only
                // active→paused (non-terminal) transition.
                row.ticks_run += 1;
                row.consecutive_failures = 0;
                persist_tick_count(&inner, &row).await;
                pause_with_audit(&inner, &row, &reason).await;
                break;
            }
            TickOutcome::Failure(err) => {
                row.ticks_run += 1;
                row.consecutive_failures += 1;
                tracing::warn!(loop_id = %row.id, failures = row.consecutive_failures, %err, "loop tick failed");
                row.last_error = Some(err);
            }
            TickOutcome::Skipped => {
                // A turn (operator message or a still-running previous
                // tick) holds the session — skip, not queue (LLD §4).
                tracing::debug!(loop_id = %row.id, "tick skipped: turn in flight");
            }
            TickOutcome::SessionGone => {
                terminalise_with_audit(
                    &inner,
                    &row,
                    LoopStatus::Cancelled,
                    "session_gone",
                    serde_json::json!({ "reason": "session_gone" }),
                )
                .await;
                break;
            }
            TickOutcome::LoopCancelled => break,
        }

        if row.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            fail(&inner, &row).await;
            break;
        }

        // Next-tick delay: a failing tick always uses exponential backoff.
        // A successful dynamic-pacing tick uses the agent's requested delay
        // (clamped to the loop's window); otherwise the fixed interval.
        row.next_tick_at = Utc::now() + next_tick_delay(&row, requested_delay);
        match inner
            .store
            .record_tick(
                row.id,
                row.next_tick_at,
                row.ticks_run,
                row.consecutive_failures,
                row.last_error.as_deref(),
            )
            .await
        {
            Ok(true) => {}
            // No longer active — cancelled/exhausted while the tick ran.
            Ok(false) => break,
            Err(err) => {
                // Storage failing: stop this driver; the row stays
                // `active` in the DB so boot replay re-arms it.
                tracing::error!(loop_id = %row.id, ?err, "loop bookkeeping write failed — parking driver");
                break;
            }
        }
    }
    // `_slot` drops here (or on any panic above), freeing the map entry.
}

/// Run one tick as a normal agent turn and wait for its finalize. Returns
/// the tick disposition and, for a successful dynamic-pacing tick, the
/// agent's requested next-tick delay (seconds) from `loop_next_tick`.
async fn fire_tick(
    inner: &Inner,
    row: &LoopRow,
    cancel: &CancellationToken,
) -> (TickOutcome, Option<f64>) {
    let handle = match run_turn(
        &inner.state,
        TurnInput {
            session_id: row.session_id.clone(),
            content: row.prompt.clone(),
            model_override: None,
            loop_id: Some(row.id),
            loop_dynamic_pacing: row.pacing_kind == PacingKind::Dynamic,
        },
    )
    .await
    {
        Ok(handle) => handle,
        Err(TurnError::TurnInFlight) => return (TickOutcome::Skipped, None),
        Err(TurnError::SessionNotFound | TurnError::SessionNotActive) => {
            return (TickOutcome::SessionGone, None)
        }
        Err(e) => return (TickOutcome::Failure(e.to_string()), None),
    };
    // Ticks have no SSE consumer — drop the receiver; the agent's event
    // channel sends fail fast and the run continues (LLD §3).
    drop(handle.events);
    let loop_intent = handle.loop_intent.clone();
    let completed = tokio::select! {
        () = cancel.cancelled() => return (TickOutcome::LoopCancelled, None),
        completion = handle.completion => completion,
    };
    match completed {
        Ok(TurnCompletion::Completed) => {
            // The turn finished — did the agent end the loop or request a
            // cadence via a tool? The intent is recorded during the run, so
            // it is set by the time completion fires.
            let state = loop_intent
                .map(|sink| sink.lock().clone())
                .unwrap_or_default();
            match state.terminal {
                Some(intent) => match intent.kind {
                    crate::loop_tools::LoopToolKind::Done => {
                        (TickOutcome::Done(intent.reason), None)
                    }
                    crate::loop_tools::LoopToolKind::Pause => {
                        (TickOutcome::Paused(intent.reason), None)
                    }
                },
                None => (TickOutcome::Success, state.next_delay_secs),
            }
        }
        Ok(TurnCompletion::Errored | TurnCompletion::Panicked) | Err(_) => {
            // A failed/panicked run is in an inconsistent state — ignore any
            // tool intent it recorded (the next tick re-converges). Log it so
            // a `loop_done` lost to a post-call error is diagnosable.
            if loop_intent.is_some_and(|sink| sink.lock().terminal.is_some()) {
                tracing::debug!(loop_id = %row.id, "loop tool intent dropped: tick did not complete cleanly");
            }
            let why = match completed {
                Ok(TurnCompletion::Errored) => "agent run errored",
                Ok(TurnCompletion::Panicked) => "agent task panicked",
                _ => "turn finalize never reported",
            };
            (TickOutcome::Failure(why.into()), None)
        }
    }
}

/// Exponential failure backoff: `interval × 2^failures`. The exponent is
/// naturally bounded by [`MAX_CONSECUTIVE_FAILURES`] (the breaker fires
/// before it can grow further).
fn backoff_delay(interval_secs: u32, consecutive_failures: u32) -> Duration {
    let factor = 2u32.saturating_pow(consecutive_failures.min(MAX_CONSECUTIVE_FAILURES));
    Duration::seconds(i64::from(interval_secs.saturating_mul(factor)))
}

/// Decide the wait before the loop's next tick.
///
/// - A failing tick (`consecutive_failures > 0`) always uses exponential
///   backoff — `requested_delay` is ignored (the agent's cadence request is
///   moot when the tick didn't succeed).
/// - A successful DYNAMIC-pacing tick uses the agent's `requested_delay`
///   clamped to `[min_interval_secs, max_interval_secs]`; absent a request,
///   it falls back to the fixed `interval_secs`.
/// - A FIXED-pacing tick always uses `interval_secs`.
fn next_tick_delay(row: &LoopRow, requested_delay: Option<f64>) -> Duration {
    if row.consecutive_failures > 0 {
        return backoff_delay(row.interval_secs, row.consecutive_failures);
    }
    match (row.pacing_kind, requested_delay) {
        (PacingKind::Dynamic, Some(secs)) => {
            // Clamp to the window; non-finite / negative → the min bound.
            let secs = if secs.is_finite() && secs >= 0.0 {
                secs as u32
            } else {
                row.min_interval_secs
            };
            let clamped = secs.clamp(row.min_interval_secs, row.max_interval_secs);
            Duration::seconds(i64::from(clamped))
        }
        _ => Duration::seconds(i64::from(row.interval_secs)),
    }
}

fn sleep_duration(next_tick_at: DateTime<Utc>, now: DateTime<Utc>) -> StdDuration {
    (next_tick_at - now).to_std().unwrap_or(StdDuration::ZERO)
}

/// Budget trip → terminal `budget_exhausted` + audit + teaching message
/// (LLD §5: "a stop, never a silent trim").
async fn exhaust(inner: &Inner, row: &LoopRow, which: &str) {
    let detail = match which {
        "max_ticks" => format!(
            "max_ticks budget ({}) exhausted after {} ticks",
            row.max_ticks, row.ticks_run
        ),
        _ => format!("ttl budget ({}s) expired", row.ttl_secs),
    };
    let moved = terminalise_with_audit(
        inner,
        row,
        LoopStatus::BudgetExhausted,
        &detail,
        serde_json::json!({ "budget": which, "ticks_run": row.ticks_run }),
    )
    .await;
    if moved {
        post_final_message(
            inner,
            row,
            &format!(
                "[loop {}] stopped: {detail}. Create a new loop with a higher budget to continue.",
                short_id(row.id)
            ),
        )
        .await;
    }
}

/// Token-budget trip (L3 Part C) → terminal `budget_exhausted` + audit +
/// teaching message. `spent` is the summed session tokens since loop-start.
async fn exhaust_tokens(inner: &Inner, row: &LoopRow, spent: u64) {
    let detail = format!(
        "max_total_tokens budget ({}) exhausted — {spent} tokens used in {} ticks",
        row.max_total_tokens, row.ticks_run
    );
    let moved = terminalise_with_audit(
        inner,
        row,
        LoopStatus::BudgetExhausted,
        &detail,
        serde_json::json!({
            "budget": "max_total_tokens",
            "tokens_spent": spent,
            "ticks_run": row.ticks_run,
        }),
    )
    .await;
    if moved {
        post_final_message(
            inner,
            row,
            &format!(
                "[loop {}] stopped: {detail}. Create a new loop with a higher \
                 max_total_tokens to continue.",
                short_id(row.id)
            ),
        )
        .await;
    }
}

/// Failure breaker → terminal `failed` + audit + final message naming the
/// last error (LLD §3).
async fn fail(inner: &Inner, row: &LoopRow) {
    let last = row.last_error.as_deref().unwrap_or("unknown error");
    let moved = terminalise_with_audit(
        inner,
        row,
        LoopStatus::Failed,
        last,
        serde_json::json!({
            "consecutive_failures": row.consecutive_failures,
            "ticks_run": row.ticks_run,
        }),
    )
    .await;
    if moved {
        post_final_message(
            inner,
            row,
            &format!(
                "[loop {}] failed after {} consecutive tick failures; last error: {last}. \
                 Create a new loop to retry.",
                short_id(row.id),
                row.consecutive_failures
            ),
        )
        .await;
    }
}

/// Move the row to `status` and audit the transition. Returns whether the
/// row actually moved — `false` means something else (operator cancel)
/// already terminalised it, and that path already audited.
async fn terminalise_with_audit(
    inner: &Inner,
    row: &LoopRow,
    status: LoopStatus,
    reason: &str,
    details: serde_json::Value,
) -> bool {
    let moved = match inner.store.terminalise(row.id, status, Some(reason)).await {
        Ok(moved) => moved,
        Err(err) => {
            tracing::error!(loop_id = %row.id, ?err, "loop terminalise failed");
            return false;
        }
    };
    if moved {
        let action = match status {
            LoopStatus::BudgetExhausted => "loop.budget_exhausted",
            LoopStatus::Failed => "loop.failed",
            LoopStatus::Done => "loop.done",
            _ => "loop.cancel",
        };
        let mut details = details;
        if let Some(obj) = details.as_object_mut() {
            obj.insert(
                "session_id".into(),
                serde_json::json!(row.session_id.clone()),
            );
        }
        append_loop_audit(&inner.state, &row.created_by, action, row.id, details).await;
    }
    moved
}

/// Persist the in-memory tick count to the row before a Done/Pause
/// transition (those transitions only update `status`, so without this the
/// `ticks_run` increment for the final tick would be lost). Best-effort:
/// the row is still `active` here, so `record_tick`'s guard matches; a
/// failure just leaves the persisted count one behind and is logged.
///
/// Non-atomicity note: this is a separate write from the following
/// terminalise/pause. A crash in the (sub-millisecond) window between them
/// leaves the row `active` with the bumped count and a past `next_tick_at`,
/// so boot replay re-arms it and the agent fires one extra tick — which
/// self-heals (it re-evaluates and calls `loop_done` again) and is bounded
/// by `max_ticks`/`ttl`. Not worth a transaction for L2a.
async fn persist_tick_count(inner: &Inner, row: &LoopRow) {
    if let Err(err) = inner
        .store
        .record_tick(
            row.id,
            row.next_tick_at,
            row.ticks_run,
            row.consecutive_failures,
            row.last_error.as_deref(),
        )
        .await
    {
        tracing::warn!(loop_id = %row.id, ?err, "failed to persist final tick count");
    }
}

/// The agent called `loop_pause`: move the row active→paused, audit, and
/// post an operator-facing note. `pause` returns `false` if a cancel raced
/// in first (then we don't audit — that path already did).
async fn pause_with_audit(inner: &Inner, row: &LoopRow, reason: &str) {
    let moved = match inner.store.pause(row.id, Some(reason)).await {
        Ok(moved) => moved,
        Err(err) => {
            tracing::error!(loop_id = %row.id, ?err, "loop pause failed");
            return;
        }
    };
    if !moved {
        return;
    }
    append_loop_audit(
        &inner.state,
        &row.created_by,
        "loop.pause",
        row.id,
        serde_json::json!({
            "session_id": row.session_id.clone(),
            "reason": reason,
            "ticks_run": row.ticks_run,
        }),
    )
    .await;
    let sid = short_id(row.id);
    let note = if reason.is_empty() {
        format!(
            "[loop {sid}] paused by the agent; it will not tick until resumed. \
             Resume with `xiaoguai loop resume {sid}` or cancel with \
             `xiaoguai loop cancel {sid}`."
        )
    } else {
        format!(
            "[loop {sid}] paused: {reason}. It will not tick until resumed. \
             Resume with `xiaoguai loop resume {sid}` or cancel with \
             `xiaoguai loop cancel {sid}`."
        )
    };
    post_final_message(inner, row, &note).await;
}

/// Best-effort final message into the loop's session so the operator is
/// told why the loop stopped (teaching-error convention). Failures are
/// logged, never fatal.
async fn post_final_message(inner: &Inner, row: &LoopRow, text: &str) {
    let session_id = SessionId::from(row.session_id.clone());
    let domain = llm_to_domain(&session_id, &LlmMessage::assistant(text));
    if let Err(err) = inner.state.messages.append(&domain).await {
        tracing::warn!(loop_id = %row.id, ?err, "failed to post loop final message");
    }
}

/// Best-effort append to the HMAC audit chain — same sink as
/// `hotl.decision` / `agent.run` (LLD §7).
async fn append_loop_audit(
    state: &AppState,
    actor: &str,
    action: &str,
    loop_id: Uuid,
    details: serde_json::Value,
) {
    let Some(sink) = &state.hotl_audit else {
        return;
    };
    let entry = xiaoguai_audit::AuditEntry {
        ts: Utc::now(),
        tenant_id: xiaoguai_audit::OWNER_TENANT_ID.to_string(),
        actor: actor.to_string(),
        action: action.to_string(),
        resource: Some(format!("loop:{loop_id}")),
        details,
    };
    if let Err(err) = sink.append(entry).await {
        tracing::warn!(%err, %action, "loop audit append failed");
    }
}

fn short_id(id: Uuid) -> String {
    id.to_string().chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_per_failure() {
        assert_eq!(backoff_delay(300, 0), Duration::seconds(300));
        assert_eq!(backoff_delay(300, 1), Duration::seconds(600));
        assert_eq!(backoff_delay(300, 4), Duration::seconds(4800));
        // Exponent is clamped — no overflow even with absurd counters.
        assert_eq!(backoff_delay(300, 99), Duration::seconds(300 * 32));
        // Multiplication saturates instead of wrapping.
        assert_eq!(
            backoff_delay(u32::MAX, 5),
            Duration::seconds(i64::from(u32::MAX))
        );
    }

    #[test]
    fn sleep_duration_clamps_past_deadlines_to_zero() {
        let now = Utc::now();
        assert_eq!(
            sleep_duration(now - Duration::seconds(10), now),
            StdDuration::ZERO
        );
        assert_eq!(
            sleep_duration(now + Duration::seconds(2), now),
            StdDuration::from_secs(2)
        );
    }

    #[test]
    fn short_id_is_eight_chars() {
        assert_eq!(short_id(Uuid::nil()), "00000000");
    }

    fn row_with_pacing(kind: PacingKind, failures: u32) -> LoopRow {
        let now = Utc::now();
        LoopRow {
            id: Uuid::nil(),
            session_id: "s".into(),
            prompt: "p".into(),
            pacing_kind: kind,
            interval_secs: 300,
            min_interval_secs: 10,
            max_interval_secs: 600,
            max_ticks: 50,
            ttl_secs: 86_400,
            max_total_tokens: 500_000,
            status: LoopStatus::Active,
            created_by: "u".into(),
            created_at: now,
            expires_at: now,
            next_tick_at: now,
            ticks_run: 0,
            consecutive_failures: failures,
            last_error: None,
        }
    }

    #[test]
    fn next_tick_delay_fixed_ignores_requested() {
        let row = row_with_pacing(PacingKind::Fixed, 0);
        // Fixed pacing always uses interval_secs, even if a delay slips in.
        assert_eq!(next_tick_delay(&row, Some(42.0)), Duration::seconds(300));
        assert_eq!(next_tick_delay(&row, None), Duration::seconds(300));
    }

    #[test]
    fn next_tick_delay_dynamic_clamps_to_window() {
        let row = row_with_pacing(PacingKind::Dynamic, 0);
        // Within bounds → honoured.
        assert_eq!(next_tick_delay(&row, Some(120.0)), Duration::seconds(120));
        // Below min → min; above max → max.
        assert_eq!(next_tick_delay(&row, Some(1.0)), Duration::seconds(10));
        assert_eq!(
            next_tick_delay(&row, Some(99_999.0)),
            Duration::seconds(600)
        );
        // Non-finite / negative → min bound.
        assert_eq!(next_tick_delay(&row, Some(-5.0)), Duration::seconds(10));
        assert_eq!(next_tick_delay(&row, Some(f64::NAN)), Duration::seconds(10));
        // No request → fixed interval fallback.
        assert_eq!(next_tick_delay(&row, None), Duration::seconds(300));
    }

    #[test]
    fn next_tick_delay_failing_tick_always_backs_off() {
        // A failing dynamic tick ignores any requested delay and uses backoff.
        let row = row_with_pacing(PacingKind::Dynamic, 2);
        assert_eq!(
            next_tick_delay(&row, Some(15.0)),
            backoff_delay(300, 2) // interval × 2^2
        );
    }
}
