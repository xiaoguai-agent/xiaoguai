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
use xiaoguai_storage::repositories::{LoopRow, LoopStatus, LoopStore, RepoError};
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

/// Inputs for [`LoopController::create`]. Budget fields fall back to the
/// defaults above.
#[derive(Debug)]
pub struct CreateLoopParams {
    pub session_id: String,
    pub prompt: String,
    pub interval_secs: Option<u32>,
    pub max_ticks: Option<u32>,
    pub ttl_secs: Option<u32>,
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

/// One tick's disposition — drives the bookkeeping in the driver loop.
#[derive(Debug)]
enum TickOutcome {
    /// Turn ran and its finalize completed.
    Success,
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
    /// Live driver cancel tokens, keyed by loop id.
    drivers: Mutex<HashMap<Uuid, CancellationToken>>,
}

/// See module docs. Construct with [`LoopController::new`], then
/// [`LoopController::replay_from_storage`] once at boot.
pub struct LoopController {
    inner: Arc<Inner>,
}

impl LoopController {
    #[must_use]
    pub fn new(store: Arc<dyn LoopStore>, state: AppState) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Inner {
                store,
                state,
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
            interval_secs,
            max_ticks,
            ttl_secs,
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

        let wait = sleep_duration(row.next_tick_at, Utc::now());
        tokio::select! {
            () = cancel.cancelled() => break, // store already terminalised by cancel()
            () = tokio::time::sleep(wait) => {}
        }
        if Utc::now() >= row.expires_at {
            exhaust(&inner, &row, "ttl").await;
            break;
        }

        match fire_tick(&inner, &row, &cancel).await {
            TickOutcome::Success => {
                row.ticks_run += 1;
                row.consecutive_failures = 0;
                row.last_error = None;
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

        row.next_tick_at = Utc::now() + backoff_delay(row.interval_secs, row.consecutive_failures);
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

/// Run one tick as a normal agent turn and wait for its finalize.
async fn fire_tick(inner: &Inner, row: &LoopRow, cancel: &CancellationToken) -> TickOutcome {
    let handle = match run_turn(
        &inner.state,
        TurnInput {
            session_id: row.session_id.clone(),
            content: row.prompt.clone(),
            model_override: None,
            loop_id: Some(row.id),
        },
    )
    .await
    {
        Ok(handle) => handle,
        Err(TurnError::TurnInFlight) => return TickOutcome::Skipped,
        Err(TurnError::SessionNotFound | TurnError::SessionNotActive) => {
            return TickOutcome::SessionGone
        }
        Err(e) => return TickOutcome::Failure(e.to_string()),
    };
    // Ticks have no SSE consumer — drop the receiver; the agent's event
    // channel sends fail fast and the run continues (LLD §3).
    drop(handle.events);
    tokio::select! {
        () = cancel.cancelled() => TickOutcome::LoopCancelled,
        completion = handle.completion => match completion {
            Ok(TurnCompletion::Completed) => TickOutcome::Success,
            Ok(TurnCompletion::Errored) => TickOutcome::Failure("agent run errored".into()),
            Ok(TurnCompletion::Panicked) => TickOutcome::Failure("agent task panicked".into()),
            Err(_) => TickOutcome::Failure("turn finalize never reported".into()),
        },
    }
}

/// Exponential failure backoff: `interval × 2^failures`. The exponent is
/// naturally bounded by [`MAX_CONSECUTIVE_FAILURES`] (the breaker fires
/// before it can grow further).
fn backoff_delay(interval_secs: u32, consecutive_failures: u32) -> Duration {
    let factor = 2u32.saturating_pow(consecutive_failures.min(MAX_CONSECUTIVE_FAILURES));
    Duration::seconds(i64::from(interval_secs.saturating_mul(factor)))
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
}
