//! /loop L1 controller integration tests (LLD-LOOP-001 §9 gates:
//! "controller tick tests; boot-replay test").
//!
//! Real `SQLite` for the loop store + session/message repos (so ticks
//! actually persist and the one-per-session unique index is exercised);
//! `MockBackend` for the agent. Intervals are sub-second so a tick fires
//! within the test's deadline. Every wait is bounded — a stuck driver
//! must fail the test, never wedge the runner (#243).

use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::loops::{CreateLoopError, CreateLoopParams, LoopController};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend, ToolCallSpec};
use xiaoguai_storage::repositories::{
    LoopRow, LoopStatus, LoopStore, MessageRepository, SessionRepository, SqliteLoopRepository,
    SqliteMessageRepository, SqliteSessionRepository,
};
use xiaoguai_types::{Session, SessionId, SessionStatus, UserId};

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};

/// Build an `AppState` over a fresh temp `SQLite` db, plus the loop store
/// (shared pool). Returns `(state, loop_store)`; the controller is built
/// by the caller from `state.clone()`.
async fn build_state() -> (
    AppState,
    Arc<dyn LoopStore>,
    sqlx::SqlitePool,
    tempfile::TempDir,
) {
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![ScriptStep::text(
        "tick reply",
    )]));
    build_state_with_backend(backend).await
}

async fn build_state_with_backend(
    backend: Arc<dyn LlmBackend>,
) -> (
    AppState,
    Arc<dyn LoopStore>,
    sqlx::SqlitePool,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.db");
    let pool = xiaoguai_storage::db::connect(path.to_str().unwrap(), 5)
        .await
        .expect("connect");
    xiaoguai_storage::db::migrate(&pool).await.expect("migrate");

    let sessions: Arc<dyn SessionRepository> = Arc::new(SqliteSessionRepository::new(pool.clone()));
    let messages: Arc<dyn MessageRepository> = Arc::new(SqliteMessageRepository::new(pool.clone()));
    let loop_store: Arc<dyn LoopStore> = Arc::new(SqliteLoopRepository::new(pool.clone()));

    let state = AppState {
        sessions,
        messages,
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock-model"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth: None,
        audit: None,
        audit_verifier: None,
        audit_chain_exporter: None,
        mcp_publish_enabled: false,
        mcp_supervisor: None,
        today: None,
        eval: None,
        webhook_pusher: None,
        nl_job_compiler: None,
        job_upserter: None,
        session_forker: None,
        usage_reader: None,
        webhook_token_validator: None,
        webhook_token_admin: None,
        scheduler_jobs_reader: None,
        hotl_policy_store: None,
        hotl_enforcer: None,
        hotl_decision_store: None,
        hotl_audit: None,
        outcome_writer: None,
        outcomes_reader: None,
        skill_packs: None,
        memory_store: None,
        workspace_repository: None,
        skill_proposals: None,
        tenant_settings: None,
        skill_author_gate: None,
        skill_audit: None,
        skills_dir: std::path::PathBuf::new(),
        personas: None,
        watchers: None,
        loops: None,
        teams: None,
        incidents: None,
        team_audit: None,
        decision_registry: Arc::new(xiaoguai_api::hotl::decision_registry::DecisionRegistry::new()),
    };
    (state, loop_store, pool, dir)
}

async fn create_session(state: &AppState, id: &str) {
    let now = Utc::now();
    state
        .sessions
        .create(&Session {
            id: SessionId::from(id.to_string()),
            user_id: UserId::from("usr_a".to_string()),
            title: None,
            created_at: now,
            updated_at: now,
            model: "mock-model".to_string(),
            status: SessionStatus::Active,
            parent_session_id: None,
            forked_from_message_id: None,
        })
        .await
        .expect("create session");
}

/// Poll `cond` until true or the deadline; panic with `what` on timeout.
async fn wait_until<F, Fut>(what: &str, mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..600 {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test]
async fn create_arms_loop_and_ticks_persist() {
    let (state, store, _pool, _dir) = build_state().await;
    create_session(&state, "sess_loop").await;
    let ctrl = LoopController::new(store.clone(), state.clone(), None);

    let row = ctrl
        .create(CreateLoopParams {
            session_id: "sess_loop".to_string(),
            prompt: "watch the build".to_string(),
            interval_secs: Some(1),
            max_ticks: Some(50),
            ttl_secs: Some(3600),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: None,
            created_by: Some("usr_a".to_string()),
        })
        .await
        .expect("create loop");
    assert_eq!(row.status, LoopStatus::Active);

    // First tick fires ~1s after creation; wait for ticks_run to advance.
    let id = row.id;
    wait_until("first tick recorded", || {
        let store = store.clone();
        async move {
            store
                .get(id)
                .await
                .unwrap()
                .is_some_and(|r| r.ticks_run >= 1)
        }
    })
    .await;

    // The tick ran as a real turn → its reply is persisted in the session.
    let msgs = state
        .messages
        .list_by_session("sess_loop", 100, 0)
        .await
        .expect("list messages");
    let has_reply = msgs.iter().any(|m| {
        m.content.iter().any(
            |b| matches!(b, xiaoguai_types::ContentBlock::Text { text } if text == "tick reply"),
        )
    });
    assert!(
        has_reply,
        "tick turn's reply must be persisted to the session"
    );

    // Cancel stops the driver and terminalises the row.
    ctrl.cancel(id, "usr_a").await.expect("cancel");
    let got = store.get(id).await.unwrap().expect("row");
    assert_eq!(got.status, LoopStatus::Cancelled);
}

/// A backend that calls one tool on the first model turn, then stops with
/// a text reply — drives a loop tick's agent to invoke `loop_done` /
/// `loop_pause`.
fn call_tool_then_stop(tool: &str, args: &str) -> Arc<dyn LlmBackend> {
    Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![ToolCallSpec {
            id: "call-1".to_string(),
            name: tool.to_string(),
            arguments_json: args.to_string(),
        }]),
        ScriptStep::text("loop wrap-up summary"),
    ]))
}

#[tokio::test]
async fn loop_done_tool_terminalises_as_done() {
    let backend = call_tool_then_stop("loop_done", r#"{"reason":"CI went green"}"#);
    let (state, store, _pool, _dir) = build_state_with_backend(backend).await;
    create_session(&state, "sess_done").await;
    let ctrl = LoopController::new(store.clone(), state.clone(), None);

    let row = ctrl
        .create(CreateLoopParams {
            session_id: "sess_done".to_string(),
            prompt: "watch CI".to_string(),
            interval_secs: Some(1),
            max_ticks: Some(50),
            ttl_secs: Some(3600),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: None,
            created_by: None,
        })
        .await
        .expect("create loop");
    let id = row.id;

    wait_until("loop terminalises as done", || {
        let store = store.clone();
        async move {
            store
                .get(id)
                .await
                .unwrap()
                .is_some_and(|r| r.status == LoopStatus::Done)
        }
    })
    .await;

    let got = store.get(id).await.unwrap().expect("row");
    assert_eq!(got.status, LoopStatus::Done);
    assert_eq!(got.ticks_run, 1, "the done tick still counts");
    // The done loop frees the session slot (no longer live).
    ctrl.create(CreateLoopParams {
        session_id: "sess_done".to_string(),
        prompt: "new one".to_string(),
        interval_secs: Some(3600),
        max_ticks: Some(1),
        ttl_secs: Some(3600),
        dynamic_pacing: false,
        min_interval_secs: None,
        max_interval_secs: None,
        max_total_tokens: None,
        created_by: None,
    })
    .await
    .expect("done loop freed the slot");
}

#[tokio::test]
async fn loop_pause_tool_moves_to_paused_and_keeps_slot() {
    let backend = call_tool_then_stop("loop_pause", r#"{"reason":"waiting on a human"}"#);
    let (state, store, _pool, _dir) = build_state_with_backend(backend).await;
    create_session(&state, "sess_pause").await;
    let ctrl = LoopController::new(store.clone(), state.clone(), None);

    let row = ctrl
        .create(CreateLoopParams {
            session_id: "sess_pause".to_string(),
            prompt: "poll until ready".to_string(),
            interval_secs: Some(1),
            max_ticks: Some(50),
            ttl_secs: Some(3600),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: None,
            created_by: None,
        })
        .await
        .expect("create loop");
    let id = row.id;

    wait_until("loop moves to paused", || {
        let store = store.clone();
        async move {
            store
                .get(id)
                .await
                .unwrap()
                .is_some_and(|r| r.status == LoopStatus::Paused)
        }
    })
    .await;

    // Paused holds the slot — a new loop on the same session is refused…
    let err = ctrl
        .create(CreateLoopParams {
            session_id: "sess_pause".to_string(),
            prompt: "second".to_string(),
            interval_secs: Some(3600),
            max_ticks: Some(1),
            ttl_secs: Some(3600),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: None,
            created_by: None,
        })
        .await
        .expect_err("paused loop still holds the slot");
    assert!(matches!(err, CreateLoopError::AlreadyExists { existing } if existing == id));

    // …and an operator can resume it: paused → active, driver re-armed.
    let resumed = ctrl.resume(id, "usr_a").await.expect("resume paused");
    assert_eq!(resumed.status, LoopStatus::Active);
    assert_eq!(
        store.get(id).await.unwrap().unwrap().status,
        LoopStatus::Active
    );
    // Resuming an already-active loop is rejected.
    let err = ctrl.resume(id, "usr_a").await.expect_err("not paused");
    assert!(matches!(err, xiaoguai_api::ResumeLoopError::NotPaused(_)));

    // Now it can be cancelled.
    ctrl.cancel(id, "usr_a").await.expect("cancel");
    assert_eq!(
        store.get(id).await.unwrap().unwrap().status,
        LoopStatus::Cancelled
    );
}

#[tokio::test]
async fn failing_tick_increments_consecutive_failures() {
    // A backend whose call errors → run_turn launches, the agent run
    // errors, completion = Errored → the driver records a Failure: ticks_run
    // and consecutive_failures both advance and last_error is persisted.
    // (The full 5-failure breaker walk is left to the backoff unit test +
    // the failure branch — exercising it end-to-end would burn ~30s of
    // real backoff and risk the runner, #243.)
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::failing(
        xiaoguai_llm::LlmError::Provider("boom".into()),
    ));
    let (state, store, _pool, _dir) = build_state_with_backend(backend).await;
    create_session(&state, "sess_fail").await;
    let ctrl = LoopController::new(store.clone(), state.clone(), None);

    let row = ctrl
        .create(CreateLoopParams {
            session_id: "sess_fail".to_string(),
            prompt: "will fail".to_string(),
            interval_secs: Some(1),
            max_ticks: Some(50),
            ttl_secs: Some(3600),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: None,
            created_by: None,
        })
        .await
        .expect("create loop");
    let id = row.id;

    wait_until("first failure recorded", || {
        let store = store.clone();
        async move {
            store
                .get(id)
                .await
                .unwrap()
                .is_some_and(|r| r.consecutive_failures >= 1)
        }
    })
    .await;

    let got = store.get(id).await.unwrap().expect("row");
    assert!(got.ticks_run >= 1);
    assert!(got.consecutive_failures >= 1);
    assert!(got.last_error.is_some(), "failure must persist last_error");
    // A failing tick is NOT terminal (the breaker only fires at 5).
    assert_eq!(got.status, LoopStatus::Active);

    ctrl.cancel(id, "usr_a").await.expect("cancel");
}

#[tokio::test]
async fn cancel_mid_tick_does_not_count_as_failure() {
    // Point-4 invariant: a cancel landing while a tick's turn is in flight
    // must terminalise the loop as `cancelled` WITHOUT tripping the failure
    // path (the dropped completion sender must not be read as an error).
    use std::time::Duration as StdDuration2;
    use tokio::sync::Semaphore;

    // Backend that parks the tick's turn until the test releases it.
    #[derive(Debug)]
    struct Blocking {
        gate: Arc<Semaphore>,
    }
    #[async_trait::async_trait]
    impl LlmBackend for Blocking {
        async fn chat_stream(
            &self,
            _req: xiaoguai_llm::ChatRequest,
        ) -> Result<xiaoguai_llm::ChatStream, xiaoguai_llm::LlmError> {
            let permit = self.gate.acquire().await.expect("gate");
            permit.forget();
            Ok(Box::pin(futures::stream::iter(vec![Ok(
                xiaoguai_llm::ChatChunk {
                    finish_reason: Some(xiaoguai_llm::FinishReason::Stop),
                    done: true,
                    ..Default::default()
                },
            )])))
        }
        fn name(&self) -> &'static str {
            "blocking"
        }
    }

    let gate = Arc::new(Semaphore::new(0));
    let backend: Arc<dyn LlmBackend> = Arc::new(Blocking { gate: gate.clone() });
    let (state, store, _pool, _dir) = build_state_with_backend(backend).await;
    create_session(&state, "sess_cancel").await;
    let ctrl = LoopController::new(store.clone(), state.clone(), None);

    let row = ctrl
        .create(CreateLoopParams {
            session_id: "sess_cancel".to_string(),
            prompt: "park".to_string(),
            interval_secs: Some(1),
            max_ticks: Some(50),
            ttl_secs: Some(3600),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: None,
            created_by: None,
        })
        .await
        .expect("create loop");
    let id = row.id;

    // Wait until the tick's turn is in flight (the per-session turn lock is
    // held), i.e. the driver is parked inside fire_tick on the gate.
    wait_until("tick turn in flight", || {
        let cancels = state.cancels.clone();
        async move { cancels.is_active("sess_cancel") }
    })
    .await;

    // Cancel while the tick is parked. The driver's select! must take the
    // LoopCancelled branch, not the dropped-completion Failure branch.
    ctrl.cancel(id, "usr_a").await.expect("cancel");
    // Release the gate so the parked turn can finish and unwind cleanly.
    gate.add_permits(8);

    // Give the driver a moment to observe the cancel and exit.
    tokio::time::sleep(StdDuration2::from_millis(50)).await;
    let got = store.get(id).await.unwrap().expect("row");
    assert_eq!(got.status, LoopStatus::Cancelled);
    assert_eq!(
        got.consecutive_failures, 0,
        "a cancel must never be counted as a tick failure"
    );
}

#[tokio::test]
async fn one_live_loop_per_session() {
    let (state, store, _pool, _dir) = build_state().await;
    create_session(&state, "sess_dup").await;
    let ctrl = LoopController::new(store, state.clone(), None);

    let first = ctrl
        .create(CreateLoopParams {
            session_id: "sess_dup".to_string(),
            prompt: "first".to_string(),
            interval_secs: Some(3600),
            max_ticks: Some(50),
            ttl_secs: Some(86_400),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: None,
            created_by: None,
        })
        .await
        .expect("first loop");

    let err = ctrl
        .create(CreateLoopParams {
            session_id: "sess_dup".to_string(),
            prompt: "second".to_string(),
            interval_secs: Some(3600),
            max_ticks: Some(50),
            ttl_secs: Some(86_400),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: None,
            created_by: None,
        })
        .await
        .expect_err("second loop refused");
    match err {
        CreateLoopError::AlreadyExists { existing } => assert_eq!(existing, first.id),
        other => panic!("expected AlreadyExists, got {other:?}"),
    }

    // After cancelling the first, a new loop can be created.
    ctrl.cancel(first.id, "usr_a").await.expect("cancel");
    ctrl.create(CreateLoopParams {
        session_id: "sess_dup".to_string(),
        prompt: "replacement".to_string(),
        interval_secs: Some(3600),
        max_ticks: Some(50),
        ttl_secs: Some(86_400),
        dynamic_pacing: false,
        min_interval_secs: None,
        max_interval_secs: None,
        max_total_tokens: None,
        created_by: None,
    })
    .await
    .expect("replacement loop");
}

#[tokio::test]
async fn create_on_unknown_session_is_not_found() {
    let (state, store, _pool, _dir) = build_state().await;
    let ctrl = LoopController::new(store, state.clone(), None);
    let err = ctrl
        .create(CreateLoopParams {
            session_id: "sess_missing".to_string(),
            prompt: "x".to_string(),
            interval_secs: Some(60),
            max_ticks: Some(1),
            ttl_secs: Some(60),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: None,
            created_by: None,
        })
        .await
        .expect_err("unknown session");
    assert!(matches!(err, CreateLoopError::SessionNotFound));
}

#[tokio::test]
async fn boot_replay_rearms_active_and_expires_stale() {
    let (state, store, _pool, _dir) = build_state().await;
    create_session(&state, "sess_armed").await;
    create_session(&state, "sess_stale").await;
    let now = Utc::now();

    // An active loop due to tick now (next_tick_at in the past).
    let armed = LoopRow {
        id: Uuid::new_v4(),
        session_id: "sess_armed".to_string(),
        prompt: "rearm me".to_string(),
        pacing_kind: xiaoguai_storage::repositories::PacingKind::Fixed,
        interval_secs: 1,
        min_interval_secs: 10,
        max_interval_secs: 3600,
        max_ticks: 50,
        ttl_secs: 3600,
        max_total_tokens: 500_000,
        status: LoopStatus::Active,
        created_by: "usr_a".to_string(),
        created_at: now - ChronoDuration::seconds(10),
        expires_at: now + ChronoDuration::seconds(3600),
        next_tick_at: now - ChronoDuration::seconds(1),
        ticks_run: 0,
        consecutive_failures: 0,
        last_error: None,
    };
    // An active loop whose ttl already lapsed → replay must terminalise it.
    let lapsed = LoopRow {
        id: Uuid::new_v4(),
        session_id: "sess_stale".to_string(),
        expires_at: now - ChronoDuration::seconds(1),
        ..armed.clone()
    };
    store.insert(&armed).await.expect("insert armed");
    store.insert(&lapsed).await.expect("insert lapsed");

    let ctrl = LoopController::new(store.clone(), state.clone(), None);
    let (rearmed, expired) = ctrl.replay_from_storage().await.expect("replay");
    assert_eq!(rearmed, 1, "one active unexpired loop re-armed");
    assert_eq!(expired, 1, "one ttl-lapsed loop expired at boot");

    // The stale loop is terminal immediately.
    let lapsed_got = store.get(lapsed.id).await.unwrap().expect("lapsed row");
    assert_eq!(lapsed_got.status, LoopStatus::BudgetExhausted);

    // The re-armed loop ticks.
    let armed_id = armed.id;
    wait_until("re-armed loop ticks", || {
        let store = store.clone();
        async move {
            store
                .get(armed_id)
                .await
                .unwrap()
                .is_some_and(|r| r.ticks_run >= 1)
        }
    })
    .await;

    ctrl.cancel(armed_id, "usr_a").await.expect("cancel");
}

// ── L3: dynamic pacing + token budget ───────────────────────────────────────

#[tokio::test]
async fn dynamic_pacing_uses_clamped_loop_next_tick_delay() {
    // The agent calls loop_next_tick(900); the loop's max bound is 60, so the
    // next tick is scheduled ~60s out (not 900). We assert the persisted
    // next_tick_at lands in the clamped window, proving the agent's request
    // was honoured-but-clamped rather than the fixed interval being used.
    let backend = call_tool_then_stop("loop_next_tick", r#"{"delay_seconds": 900}"#);
    let (state, store, _pool, _dir) = build_state_with_backend(backend).await;
    create_session(&state, "sess_dyn").await;
    let ctrl = LoopController::new(store.clone(), state.clone(), None);

    let row = ctrl
        .create(CreateLoopParams {
            session_id: "sess_dyn".to_string(),
            prompt: "poll the deploy".to_string(),
            interval_secs: Some(1),
            max_ticks: Some(50),
            ttl_secs: Some(3600),
            dynamic_pacing: true,
            min_interval_secs: Some(5),
            max_interval_secs: Some(60),
            max_total_tokens: None,
            created_by: None,
        })
        .await
        .expect("create loop");
    let id = row.id;

    // Wait for the first tick to run and re-schedule.
    wait_until("first dynamic tick recorded", || {
        let store = store.clone();
        async move {
            store
                .get(id)
                .await
                .unwrap()
                .is_some_and(|r| r.ticks_run >= 1)
        }
    })
    .await;

    let got = store.get(id).await.unwrap().expect("row");
    let secs_out = (got.next_tick_at - Utc::now()).num_seconds();
    // Clamped to max=60 (requested 900). Allow a small scheduling margin.
    assert!(
        (50..=61).contains(&secs_out),
        "next tick should be ~60s out (clamped from 900), got {secs_out}s"
    );
    ctrl.cancel(id, "usr_a").await.expect("cancel");
}

#[tokio::test]
async fn token_budget_exhaustion_stops_the_loop() {
    use xiaoguai_storage::repositories::{
        SqliteTokenUsageRepository, TokenUsageEntry, TokenUsageRepository,
    };
    let (state, store, pool, _dir) = build_state().await;
    create_session(&state, "sess_budget").await;

    // Pre-seed token_usage so the session is already over its 100-token
    // budget before the first tick — the gate must trip immediately.
    let token_repo = SqliteTokenUsageRepository::new(pool.clone());
    token_repo
        .record_batch(&[TokenUsageEntry {
            ts: Utc::now() + ChronoDuration::seconds(1),
            user_id: Some("usr_a".into()),
            session_id: Some("sess_budget".into()),
            provider_id: "p".into(),
            model: "m".into(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: Some(500),
            request_id: None,
        }])
        .await
        .expect("seed usage");

    let token_usage: Arc<dyn TokenUsageRepository> = Arc::new(token_repo);
    let ctrl = LoopController::new(store.clone(), state.clone(), Some(token_usage));

    let row = ctrl
        .create(CreateLoopParams {
            session_id: "sess_budget".to_string(),
            prompt: "burn tokens".to_string(),
            interval_secs: Some(1),
            max_ticks: Some(50),
            ttl_secs: Some(3600),
            dynamic_pacing: false,
            min_interval_secs: None,
            max_interval_secs: None,
            max_total_tokens: Some(100),
            created_by: None,
        })
        .await
        .expect("create loop");
    let id = row.id;

    // The budget gate runs before the first sleep, so the loop terminalises
    // as budget_exhausted almost immediately.
    wait_until("loop budget-exhausts", || {
        let store = store.clone();
        async move {
            store
                .get(id)
                .await
                .unwrap()
                .is_some_and(|r| r.status == LoopStatus::BudgetExhausted)
        }
    })
    .await;
    assert_eq!(
        store.get(id).await.unwrap().unwrap().status,
        LoopStatus::BudgetExhausted
    );
}

// ── REST surface ────────────────────────────────────────────────────────────

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.expect("read body");
    serde_json::from_slice(&bytes).expect("json")
}

#[tokio::test]
async fn rest_returns_503_when_unwired() {
    let (state, _store, _pool, _dir) = build_state().await;
    // state.loops stays None → routes return 503.
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/loops")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn rest_create_list_get_cancel_round_trip() {
    let (mut state, store, _pool, _dir) = build_state().await;
    create_session(&state, "sess_rest").await;
    let ctrl = LoopController::new(store, state.clone(), None);
    state.loops = Some(ctrl);
    let app = router(state);

    // Create.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/loops")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "session_id": "sess_rest",
                        "prompt": "poll the rollout",
                        "interval_secs": 3600,
                        "max_ticks": 10,
                        "ttl_secs": 7200
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp.into_body()).await;
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["status"], "active");
    assert_eq!(created["interval_secs"], 3600);

    // Duplicate on the same session → 409.
    let dup = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/loops")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"session_id": "sess_rest", "prompt": "again"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);

    // List shows it.
    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/loops")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let rows = body_json(list.into_body()).await;
    assert_eq!(rows.as_array().unwrap().len(), 1);

    // Cancel → 200 + terminal status.
    let cancel = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/v1/loops/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancel.status(), StatusCode::OK);
    assert_eq!(body_json(cancel.into_body()).await["status"], "cancelled");

    // Second cancel → 409 (already terminal).
    let cancel2 = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/v1/loops/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancel2.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn rest_create_on_unknown_session_is_404() {
    let (mut state, store, _pool, _dir) = build_state().await;
    let ctrl = LoopController::new(store, state.clone(), None);
    state.loops = Some(ctrl);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/loops")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"session_id": "nope", "prompt": "x"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
