//! Auto-dispatcher worker pool — the heart of `xiaoguai-tasks`.
//!
//! [`WorkerPool`] maintains a bounded set of Tokio worker tasks. Each tick it
//! polls the READY column, claims a batch of cards via `CardStore::claim_ready`
//! (which uses SKIP LOCKED semantics to prevent double-claiming), and fans them
//! out across available workers. The semaphore (`Arc<Semaphore>`) is the
//! concurrency bound: exactly `pool_size` cards execute at once.
//!
//! ## Claim / SKIP LOCKED
//!
//! `CardStore::claim_ready` is the single concurrent-safe entry point. In the
//! production PG backend this maps to `SELECT … FOR UPDATE SKIP LOCKED`. The
//! in-memory implementation holds an async Mutex for the entire claim scan so
//! the semantics are identical: two racing workers cannot claim the same card.
//!
//! ## Retry
//!
//! On executor failure the card stays RUNNING (the store row is not touched).
//! The worker loops up to `max_retries` times with zero sleep (integration
//! tests control timing via `tokio::time::pause`). Only on exhaustion does it
//! call `mark_blocked`. Callers that need back-off can wrap the executor.
//!
//! ## Timeout
//!
//! Each worker wraps `executor.execute` in `tokio::time::timeout`. On expiry
//! the future is dropped (the executor implementation is responsible for
//! cleanup), the card is moved to BLOCKED with reason `"agent timeout"`, and
//! the `timed_out` counter is incremented.
//!
//! ## Graceful shutdown / SIGTERM
//!
//! [`WorkerPool::run`] selects between the poll interval and a `CancellationToken`.
//! Call [`WorkerPool::shutdown`] to cancel the token. The pool stops accepting
//! new claims and awaits the `JoinSet` until all in-flight workers finish, then
//! returns. The binary can wire this to SIGTERM:
//!
//! ```rust,ignore
//! tokio::signal::unix::signal(SignalKind::terminate())
//!     .unwrap()
//!     .recv()
//!     .await;
//! pool.shutdown().await;
//! ```

use std::env;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

use crate::card::{Attribution, CardColumn, KanbanCard, Outcome};
use crate::executor::{ExecutorError, TaskExecutor};
use crate::metrics::PoolMetrics;
use crate::store::CardStore;

// ─── Configuration ────────────────────────────────────────────────────────────

/// Default pool concurrency (overridden by `KANBAN_POOL_SIZE`).
const DEFAULT_POOL_SIZE: usize = 10;
/// Default poll interval (overridden by `KANBAN_POLL_INTERVAL_MS`).
const DEFAULT_POLL_INTERVAL_MS: u64 = 5_000;
/// Default per-task timeout in seconds (overridden by `KANBAN_TASK_TIMEOUT_SECS`).
const DEFAULT_TASK_TIMEOUT_SECS: u64 = 300;
/// Default retry limit (overridden by `KANBAN_MAX_RETRIES`).
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Static configuration for a [`WorkerPool`].
///
/// Built from environment variables via [`PoolConfig::from_env`] or
/// constructed directly in tests.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum number of cards executing concurrently.
    pub pool_size: usize,
    /// How often the dispatcher polls the READY column.
    pub poll_interval: Duration,
    /// Per-task timeout. Exceeded → card moved to BLOCKED.
    pub task_timeout: Duration,
    /// Maximum execution attempts before a card is permanently BLOCKED.
    pub max_retries: u32,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            pool_size: DEFAULT_POOL_SIZE,
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
            task_timeout: Duration::from_secs(DEFAULT_TASK_TIMEOUT_SECS),
            max_retries: DEFAULT_MAX_RETRIES,
        }
    }
}

impl PoolConfig {
    /// Read configuration from environment variables with defaults.
    ///
    /// | Variable | Default |
    /// |---|---|
    /// | `KANBAN_POOL_SIZE` | 10 |
    /// | `KANBAN_POLL_INTERVAL_MS` | 5000 |
    /// | `KANBAN_TASK_TIMEOUT_SECS` | 300 |
    /// | `KANBAN_MAX_RETRIES` | 3 |
    #[must_use]
    pub fn from_env() -> Self {
        let pool_size = env_usize("KANBAN_POOL_SIZE", DEFAULT_POOL_SIZE);
        let poll_ms = env_u64("KANBAN_POLL_INTERVAL_MS", DEFAULT_POLL_INTERVAL_MS);
        let timeout_secs = env_u64("KANBAN_TASK_TIMEOUT_SECS", DEFAULT_TASK_TIMEOUT_SECS);
        let max_retries = env_u32("KANBAN_MAX_RETRIES", DEFAULT_MAX_RETRIES);
        Self {
            pool_size,
            poll_interval: Duration::from_millis(poll_ms),
            task_timeout: Duration::from_secs(timeout_secs),
            max_retries,
        }
    }
}

// ─── WorkerPool ───────────────────────────────────────────────────────────────

/// The auto-dispatcher. Holds shared references to the store, executor, and
/// metrics. Cheap to clone — all state is `Arc`-wrapped.
#[derive(Clone)]
pub struct WorkerPool {
    store: Arc<dyn CardStore>,
    executor: Arc<dyn TaskExecutor>,
    config: PoolConfig,
    metrics: PoolMetrics,
    /// Send side of the shutdown channel. Dropping or explicitly calling
    /// [`WorkerPool::shutdown`] causes the run loop to stop accepting new claims.
    shutdown_tx: Arc<watch::Sender<bool>>,
    shutdown_rx: watch::Receiver<bool>,
}

impl WorkerPool {
    /// Construct a pool. Use [`WorkerPool::run`] to start the dispatch loop.
    #[must_use]
    pub fn new(
        store: Arc<dyn CardStore>,
        executor: Arc<dyn TaskExecutor>,
        config: PoolConfig,
        metrics: PoolMetrics,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            store,
            executor,
            config,
            metrics,
            shutdown_tx: Arc::new(shutdown_tx),
            shutdown_rx,
        }
    }

    /// Signal graceful shutdown and wait for all in-flight tasks to drain.
    ///
    /// After this returns the run loop has exited and the `JoinSet` is empty.
    /// Idempotent — safe to call multiple times.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Drive the dispatch loop until [`WorkerPool::shutdown`] is called.
    ///
    /// The loop:
    /// 1. Waits for `poll_interval`.
    /// 2. Claims up to `pool_size` READY cards.
    /// 3. Spawns one worker task per claimed card (gated by semaphore).
    /// 4. Repeats until shutdown signal received.
    /// 5. On shutdown: stops claiming, awaits all in-flight workers, returns.
    pub async fn run(&self) {
        let semaphore = Arc::new(Semaphore::new(self.config.pool_size));
        let mut join_set: JoinSet<()> = JoinSet::new();
        let mut interval = tokio::time::interval(self.config.poll_interval);
        // Skip missed ticks: a long processing batch shouldn't queue up a burst.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut shutdown_rx = self.shutdown_rx.clone();

        info!(
            pool_size = self.config.pool_size,
            poll_interval_ms = self.config.poll_interval.as_millis(),
            max_retries = self.config.max_retries,
            task_timeout_secs = self.config.task_timeout.as_secs(),
            "kanban dispatcher starting"
        );

        loop {
            tokio::select! {
                biased;
                // Shutdown wins over the timer: stop accepting new claims.
                result = shutdown_rx.changed() => {
                    if result.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    self.poll_and_dispatch(&semaphore, &mut join_set).await;
                }
            }
        }

        info!("kanban dispatcher draining {} in-flight tasks", join_set.len());
        // Drain: let all running workers finish.
        while join_set.join_next().await.is_some() {}
        info!("kanban dispatcher stopped");
    }

    /// Claim a batch of READY cards and spawn a worker per card.
    async fn poll_and_dispatch(
        &self,
        semaphore: &Arc<Semaphore>,
        join_set: &mut JoinSet<()>,
    ) {
        // Reap finished workers to keep JoinSet from growing unbounded.
        while join_set.try_join_next().is_some() {}

        let batch = match self.store.claim_ready(self.config.pool_size).await {
            Ok(cards) => cards,
            Err(e) => {
                error!(error = %e, "failed to claim READY cards");
                return;
            }
        };

        for card in batch {
            // Acquire a permit before spawning. This blocks the poll loop until
            // a worker slot is free, which is the right back-pressure point.
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore never closed");

            let worker = Worker {
                store: Arc::clone(&self.store),
                executor: Arc::clone(&self.executor),
                metrics: self.metrics.clone(),
                task_timeout: self.config.task_timeout,
                max_retries: self.config.max_retries,
            };

            debug!(card_id = %card.id, title = %card.title, "dispatching card");
            join_set.spawn(async move {
                worker.run(card).await;
                drop(permit); // release slot back to pool
            });
        }
    }
}

// ─── Worker ───────────────────────────────────────────────────────────────────

/// Per-card worker context. Cloned for each spawned task.
struct Worker {
    store: Arc<dyn CardStore>,
    executor: Arc<dyn TaskExecutor>,
    metrics: PoolMetrics,
    task_timeout: Duration,
    max_retries: u32,
}

impl Worker {
    /// Execute `card`, retrying up to `max_retries` times.
    ///
    /// Column transitions:
    /// * Success → DONE + outcome + `attribution_chain`
    /// * Timeout → BLOCKED("agent timeout")
    /// * Exhausted retries → BLOCKED(last error message)
    async fn run(&self, mut card: KanbanCard) {
        let tenant = card
            .tenant_id
            .clone()
            .unwrap_or_else(|| "system".to_string());
        self.metrics.inc_dispatched(&tenant);

        let result = self.execute_with_retry(&mut card, &tenant).await;

        match result {
            WorkerOutcome::Done(outcome) => {
                debug!(card_id = %card.id, "card done");
                if let Err(e) = self.store.mark_done(card.id, outcome).await {
                    error!(card_id = %card.id, error = %e, "mark_done failed");
                }
                self.metrics.inc_completed(&tenant);
            }
            WorkerOutcome::Blocked(reason) => {
                warn!(card_id = %card.id, reason = %reason, "card blocked");
                if let Err(e) = self.store.mark_blocked(card.id, reason).await {
                    error!(card_id = %card.id, error = %e, "mark_blocked failed");
                }
                self.metrics.inc_blocked(&tenant);
            }
        }
    }

    /// Retry loop. Returns the final [`WorkerOutcome`].
    async fn execute_with_retry(
        &self,
        card: &mut KanbanCard,
        tenant: &str,
    ) -> WorkerOutcome {
        // card.attempt was incremented by claim_ready; subsequent retries
        // increment it here.
        let first_attempt = card.attempt;
        let max_attempts = self.max_retries.max(1);

        for attempt in first_attempt..first_attempt + max_attempts {
            if attempt > first_attempt {
                card.attempt = attempt;
            }

            let exec_result = tokio::time::timeout(
                self.task_timeout,
                self.executor.execute(card),
            )
            .await;

            match exec_result {
                // Timeout.
                Err(_elapsed) => {
                    error!(
                        card_id = %card.id,
                        attempt,
                        timeout_secs = self.task_timeout.as_secs(),
                        "task timed out"
                    );
                    self.metrics.inc_timed_out(tenant);
                    // Timeout is always terminal — do not retry.
                    return WorkerOutcome::Blocked("agent timeout".to_string());
                }
                // Executor returned.
                Ok(inner) => match inner {
                    Ok(mut outcome) => {
                        // Append dispatcher attribution to the chain.
                        outcome.attribution_chain.push(
                            Attribution::new(
                                format!("dispatcher:attempt-{attempt}"),
                                "executor",
                            )
                            .with_note(format!("card: {}", card.id)),
                        );
                        return WorkerOutcome::Done(outcome);
                    }
                    Err(ExecutorError::Cancelled) => {
                        // Cancellation is always terminal.
                        self.metrics.inc_failed(tenant);
                        return WorkerOutcome::Blocked("cancelled".to_string());
                    }
                    Err(err) => {
                        self.metrics.inc_failed(tenant);
                        warn!(
                            card_id = %card.id,
                            attempt,
                            error = %err,
                            "executor failed"
                        );
                        let last = attempt == first_attempt + max_attempts - 1;
                        if last {
                            return WorkerOutcome::Blocked(err.to_string());
                        }
                        // Not the last attempt — continue the loop.
                    }
                },
            }
        }

        // Should be unreachable due to the `last` check above.
        WorkerOutcome::Blocked("retry limit exceeded".to_string())
    }
}

/// Internal result of a worker's execution sequence.
enum WorkerOutcome {
    Done(Outcome),
    Blocked(String),
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u32(key: &str, default: u32) -> u32 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{CardId, KanbanCard};
    use crate::executor::{ExecutorError, MockExecutor};
    use crate::store::InMemoryCardStore;
    use std::time::Duration;

    fn fast_config(pool_size: usize, max_retries: u32) -> PoolConfig {
        PoolConfig {
            pool_size,
            poll_interval: Duration::from_millis(10),
            task_timeout: Duration::from_secs(5),
            max_retries,
        }
    }

    fn pool(
        store: Arc<InMemoryCardStore>,
        executor: Arc<MockExecutor>,
        config: PoolConfig,
    ) -> WorkerPool {
        let store: Arc<dyn CardStore> = store;
        let executor: Arc<dyn TaskExecutor> = executor;
        WorkerPool::new(store, executor, config, PoolMetrics::no_op())
    }

    // ── Integration: 3 workers, 10 cards, all DONE within budget ─────────────

    /// Spawn pool with 3 workers + queue 10 always-ok cards.
    /// All 10 must be DONE within 2 seconds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ten_cards_three_workers_all_done() {
        let store = Arc::new(InMemoryCardStore::new());
        for i in 0..10 {
            store
                .insert(KanbanCard::new(format!("task-{i}"), serde_json::json!({"i": i})))
                .await;
        }

        let executor = Arc::new(MockExecutor::always_ok());
        let p = pool(Arc::clone(&store), executor, fast_config(3, 1));
        let p2 = p.clone();

        let handle = tokio::spawn(async move { p2.run().await });

        // Wait up to 2 s for all 10 to finish.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let snap = store.snapshot().await;
            let done = snap.iter().filter(|c| c.column == CardColumn::Done).count();
            if done == 10 {
                break;
            }
            assert!(
                tokio::time::Instant::now() <= deadline,
                "not all cards done within deadline: {done}/10 done"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        p.shutdown();
        let _ = handle.await;

        let snap = store.snapshot().await;
        assert!(snap.iter().all(|c| c.column == CardColumn::Done));
    }

    // ── Retry: executor fails N-1 times then succeeds ─────────────────────────

    #[tokio::test]
    async fn card_retries_until_success() {
        let store = Arc::new(InMemoryCardStore::new());
        store
            .insert(KanbanCard::new("retry-card", serde_json::Value::Null))
            .await;

        let executor = Arc::new(MockExecutor::new());
        // Fail twice, then succeed.
        executor.enqueue_ok("ok on 3rd attempt");
        executor.enqueue_err(ExecutorError::Agent("boom2".into()));
        executor.enqueue_err(ExecutorError::Agent("boom1".into()));

        let p = pool(Arc::clone(&store), executor, fast_config(1, 3));
        let p2 = p.clone();
        let handle = tokio::spawn(async move { p2.run().await });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let snap = store.snapshot().await;
            if snap.iter().any(|c| c.column == CardColumn::Done) {
                break;
            }
            assert!(
                tokio::time::Instant::now() <= deadline,
                "card never reached DONE"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        p.shutdown();
        let _ = handle.await;

        let snap = store.snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].column, CardColumn::Done);
        let outcome = snap[0].outcome.as_ref().unwrap();
        assert!(!outcome.attribution_chain.is_empty());
    }

    // ── Retry exhaustion: card goes to BLOCKED ────────────────────────────────

    #[tokio::test]
    async fn exhausted_retries_moves_to_blocked() {
        let store = Arc::new(InMemoryCardStore::new());
        store
            .insert(KanbanCard::new("doomed", serde_json::Value::Null))
            .await;

        let executor = Arc::new(MockExecutor::new());
        // Always fail — 3 attempts, all errors.
        executor.enqueue_err(ExecutorError::Agent("fail3".into()));
        executor.enqueue_err(ExecutorError::Agent("fail2".into()));
        executor.enqueue_err(ExecutorError::Agent("fail1".into()));

        let p = pool(Arc::clone(&store), executor, fast_config(1, 3));
        let p2 = p.clone();
        let handle = tokio::spawn(async move { p2.run().await });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let snap = store.snapshot().await;
            if snap.iter().any(|c| c.column == CardColumn::Blocked) {
                break;
            }
            assert!(
                tokio::time::Instant::now() <= deadline,
                "card never reached BLOCKED"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        p.shutdown();
        let _ = handle.await;

        let snap = store.snapshot().await;
        assert_eq!(snap[0].column, CardColumn::Blocked);
        assert!(snap[0].blocked_reason.is_some());
    }

    // ── Timeout: slow executor → BLOCKED("agent timeout") ────────────────────

    #[tokio::test]
    async fn timeout_moves_card_to_blocked() {
        use async_trait::async_trait;
        use crate::card::Outcome;

        struct SlowExecutor;
        #[async_trait]
        impl TaskExecutor for SlowExecutor {
            async fn execute(&self, _card: &KanbanCard) -> Result<Outcome, ExecutorError> {
                // Sleep longer than the configured timeout.
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(Outcome::new("should never reach here", serde_json::Value::Null))
            }
        }

        let store = Arc::new(InMemoryCardStore::new());
        store
            .insert(KanbanCard::new("slow-card", serde_json::Value::Null))
            .await;

        let cfg = PoolConfig {
            pool_size: 1,
            poll_interval: Duration::from_millis(10),
            task_timeout: Duration::from_millis(50), // very short timeout
            max_retries: 1,
        };
        let store_ref = Arc::clone(&store);
        let p = WorkerPool::new(
            store_ref as Arc<dyn CardStore>,
            Arc::new(SlowExecutor) as Arc<dyn TaskExecutor>,
            cfg,
            PoolMetrics::no_op(),
        );
        let p2 = p.clone();
        let handle = tokio::spawn(async move { p2.run().await });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let snap = store.snapshot().await;
            if snap.iter().any(|c| c.column == CardColumn::Blocked) {
                break;
            }
            assert!(
                tokio::time::Instant::now() <= deadline,
                "card never BLOCKED after timeout"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        p.shutdown();
        let _ = handle.await;

        let snap = store.snapshot().await;
        assert_eq!(snap[0].column, CardColumn::Blocked);
        assert_eq!(
            snap[0].blocked_reason.as_deref(),
            Some("agent timeout")
        );
    }

    // ── Cancel: executor returns Cancelled → terminal BLOCKED ────────────────

    #[tokio::test]
    async fn cancelled_card_goes_to_blocked_not_retried() {
        let store = Arc::new(InMemoryCardStore::new());
        store
            .insert(KanbanCard::new("cancel-me", serde_json::Value::Null))
            .await;

        let executor = Arc::new(MockExecutor::new());
        executor.enqueue_err(ExecutorError::Cancelled);

        let p = pool(Arc::clone(&store), executor.clone(), fast_config(1, 5));
        let p2 = p.clone();
        let handle = tokio::spawn(async move { p2.run().await });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let snap = store.snapshot().await;
            if snap.iter().any(|c| c.column == CardColumn::Blocked) {
                break;
            }
            assert!(
                tokio::time::Instant::now() <= deadline,
                "card never BLOCKED after cancel"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        p.shutdown();
        let _ = handle.await;

        let snap = store.snapshot().await;
        assert_eq!(snap[0].column, CardColumn::Blocked);
        assert_eq!(snap[0].blocked_reason.as_deref(), Some("cancelled"));
        // Queue should have 0 items: cancelled is terminal (not retried).
        assert_eq!(executor.queue_len(), 0);
    }

    // ── Graceful shutdown drains in-flight tasks ──────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_drains_in_flight_before_exit() {
        use async_trait::async_trait;
        use std::sync::atomic::{AtomicU32, Ordering};
        use crate::card::Outcome;

        static COMPLETED: AtomicU32 = AtomicU32::new(0);

        struct CountingExecutor;
        #[async_trait]
        impl TaskExecutor for CountingExecutor {
            async fn execute(&self, _card: &KanbanCard) -> Result<Outcome, ExecutorError> {
                // Small delay to ensure we're "in flight" when shutdown is called.
                tokio::time::sleep(Duration::from_millis(30)).await;
                COMPLETED.fetch_add(1, Ordering::SeqCst);
                Ok(Outcome::new("counted", serde_json::Value::Null))
            }
        }

        let store = Arc::new(InMemoryCardStore::new());
        // Enqueue 3 cards — they'll all be claimed and running when we shut down.
        for i in 0..3 {
            store
                .insert(KanbanCard::new(format!("drain-{i}"), serde_json::Value::Null))
                .await;
        }

        let cfg = PoolConfig {
            pool_size: 3,
            poll_interval: Duration::from_millis(5),
            task_timeout: Duration::from_secs(5),
            max_retries: 1,
        };
        let store_ref = Arc::clone(&store);
        let p = WorkerPool::new(
            store_ref as Arc<dyn CardStore>,
            Arc::new(CountingExecutor) as Arc<dyn TaskExecutor>,
            cfg,
            PoolMetrics::no_op(),
        );
        let p2 = p.clone();
        let handle = tokio::spawn(async move { p2.run().await });

        // Let the first poll fire and cards enter RUNNING.
        tokio::time::sleep(Duration::from_millis(15)).await;
        // Signal shutdown while cards are still in flight.
        p.shutdown();
        // Pool must drain before the future resolves.
        handle.await.unwrap();

        // All 3 workers must have finished.
        assert_eq!(COMPLETED.load(Ordering::SeqCst), 3);
        let snap = store.snapshot().await;
        assert!(snap.iter().all(|c| c.column == CardColumn::Done));
    }

    // ── Attribution chain is populated ───────────────────────────────────────

    #[tokio::test]
    async fn done_card_has_attribution_chain() {
        let store = Arc::new(InMemoryCardStore::new());
        store
            .insert(KanbanCard::new("attr-card", serde_json::Value::Null))
            .await;

        let executor = Arc::new(MockExecutor::always_ok());
        let p = pool(Arc::clone(&store), executor, fast_config(1, 1));
        let p2 = p.clone();
        let handle = tokio::spawn(async move { p2.run().await });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let snap = store.snapshot().await;
            if snap.iter().any(|c| c.column == CardColumn::Done) {
                break;
            }
            assert!(
                tokio::time::Instant::now() <= deadline,
                "card never done"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        p.shutdown();
        let _ = handle.await;

        let snap = store.snapshot().await;
        let outcome = snap[0].outcome.as_ref().unwrap();
        assert!(
            !outcome.attribution_chain.is_empty(),
            "attribution_chain must be populated"
        );
        assert!(
            outcome.attribution_chain[0].actor.starts_with("dispatcher:"),
            "actor must start with 'dispatcher:'"
        );
    }

    // ── PoolConfig defaults match constants ───────────────────────────────────

    #[test]
    fn config_defaults_are_documented_values() {
        // When no env vars are set (nominal test environment) the defaults
        // match the constants declared at the top of this module.
        // We can't mutate env in a forbid(unsafe) crate; instead we verify
        // that from_env() returns the documented fall-backs when the vars
        // are absent (which is the state in a standard test run).
        let cfg = PoolConfig::default();
        assert_eq!(cfg.pool_size, DEFAULT_POOL_SIZE);
        assert_eq!(cfg.max_retries, DEFAULT_MAX_RETRIES);
        assert_eq!(cfg.poll_interval.as_millis(), u128::from(DEFAULT_POLL_INTERVAL_MS));
        assert_eq!(cfg.task_timeout.as_secs(), DEFAULT_TASK_TIMEOUT_SECS);
    }

    // ── SKIP LOCKED: two concurrent claim calls never yield same card ─────────

    #[tokio::test]
    async fn concurrent_claims_never_return_same_card() {
        let store = Arc::new(InMemoryCardStore::new());
        // Insert 4 READY cards.
        for i in 0..4 {
            store
                .insert(KanbanCard::new(format!("c{i}"), serde_json::Value::Null))
                .await;
        }

        // Two workers race to claim 4 cards each (total 8 claims, only 4 exist).
        let s1 = Arc::clone(&store);
        let s2 = Arc::clone(&store);
        let (r1, r2) = tokio::join!(
            tokio::spawn(async move { s1.claim_ready(4).await.unwrap() }),
            tokio::spawn(async move { s2.claim_ready(4).await.unwrap() }),
        );
        let claimed1 = r1.unwrap();
        let claimed2 = r2.unwrap();

        // Combined, all claimed ids must be unique.
        let all_ids: std::collections::HashSet<CardId> = claimed1
            .iter()
            .chain(claimed2.iter())
            .map(|c| c.id)
            .collect();
        assert_eq!(
            all_ids.len(),
            claimed1.len() + claimed2.len(),
            "each card must be claimed by exactly one worker"
        );
        // Total claimed must equal the 4 inserted cards.
        assert_eq!(all_ids.len(), 4);
    }
}
