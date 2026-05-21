//! End-to-end: router-level circuit breaker behaviour.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use parking_lot::Mutex;
use xiaoguai_llm::{
    BreakerConfig, BreakerState, Breakers, ChatRequest, Clock, LlmBackend, LlmError, LlmRouter,
    Message, MockBackend, ResolveCtx, Role, RouterConfig,
};
use xiaoguai_types::ProviderId;

#[derive(Debug, Clone)]
struct FakeClock(Arc<Mutex<Instant>>);
impl FakeClock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }
    #[allow(dead_code)]
    fn advance(&self, d: Duration) {
        *self.0.lock() += d;
    }
}
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        *self.0.lock()
    }
}

fn make_req() -> ChatRequest {
    ChatRequest {
        model: "any".into(),
        messages: vec![Message {
            role: Role::User,
            content: "hi".into(),
        }],
        temperature: None,
        max_tokens: None,
    }
}

async fn drain(mut s: xiaoguai_llm::ChatStream) -> String {
    let mut out = String::new();
    while let Some(c) = s.next().await {
        out.push_str(&c.expect("chunk").delta);
    }
    out
}

#[tokio::test]
async fn router_skips_open_breaker_and_uses_healthy_fallback() {
    let clock = FakeClock::new();
    let breakers = Breakers::with_clock(
        BreakerConfig {
            failure_threshold: 3,
            failure_window: Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
        },
        Arc::new(clock.clone()),
    );
    let p_bad = ProviderId::new();
    let p_good = ProviderId::new();

    // Force-open the bad provider's breaker.
    breakers.record_failure(&p_bad);
    breakers.record_failure(&p_bad);
    breakers.record_failure(&p_bad);
    assert!(matches!(breakers.state(&p_bad), BreakerState::Open { .. }));

    let mut backends: HashMap<ProviderId, Arc<dyn LlmBackend>> = HashMap::new();
    backends.insert(
        p_bad.clone(),
        // Would yield text if it were tried, so this catches "router didn't skip".
        Arc::new(MockBackend::with_response("BAD SHOULD NOT APPEAR")),
    );
    backends.insert(p_good.clone(), Arc::new(MockBackend::with_response("good")));

    let router = LlmRouter::new(
        backends,
        RouterConfig {
            fallback_order: vec![p_bad, p_good.clone()],
            ..Default::default()
        },
    )
    .with_breakers(breakers);

    let stream = router
        .chat_stream(ResolveCtx::default(), make_req())
        .await
        .expect("ok");
    assert_eq!(drain(stream).await, "good");
}

#[tokio::test]
async fn first_call_failure_records_breaker_state() {
    let clock = FakeClock::new();
    let breakers = Breakers::with_clock(
        BreakerConfig {
            failure_threshold: 2,
            failure_window: Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
        },
        Arc::new(clock.clone()),
    );
    let p = ProviderId::new();
    let mut backends: HashMap<ProviderId, Arc<dyn LlmBackend>> = HashMap::new();
    backends.insert(
        p.clone(),
        Arc::new(MockBackend::failing(LlmError::Provider("503".into()))),
    );

    let router = LlmRouter::new(
        backends,
        RouterConfig {
            fallback_order: vec![p.clone()],
            ..Default::default()
        },
    )
    .with_breakers(breakers.clone());

    // First call → fails → 1 failure recorded → still closed (threshold=2).
    let _ = router.chat_stream(ResolveCtx::default(), make_req()).await;
    assert!(matches!(breakers.state(&p), BreakerState::Closed));

    // Second call → fails again → threshold reached → Open.
    let _ = router.chat_stream(ResolveCtx::default(), make_req()).await;
    assert!(matches!(breakers.state(&p), BreakerState::Open { .. }));
}

#[tokio::test]
async fn success_resets_breaker() {
    let clock = FakeClock::new();
    let breakers = Breakers::with_clock(
        BreakerConfig {
            failure_threshold: 5,
            failure_window: Duration::from_secs(60),
            cooldown: Duration::from_secs(30),
        },
        Arc::new(clock.clone()),
    );
    let p = ProviderId::new();
    breakers.record_failure(&p);
    breakers.record_failure(&p);
    breakers.record_failure(&p);
    // 3 failures recorded but still closed (threshold=5).

    let mut backends: HashMap<ProviderId, Arc<dyn LlmBackend>> = HashMap::new();
    backends.insert(p.clone(), Arc::new(MockBackend::with_response("ok")));

    let router = LlmRouter::new(
        backends,
        RouterConfig {
            fallback_order: vec![p.clone()],
            ..Default::default()
        },
    )
    .with_breakers(breakers.clone());

    let stream = router
        .chat_stream(ResolveCtx::default(), make_req())
        .await
        .expect("ok");
    let _ = drain(stream).await;

    // After success, breaker should be reset. Now drive 4 more failures with
    // raw API — should still be closed (clean window).
    breakers.record_failure(&p);
    breakers.record_failure(&p);
    breakers.record_failure(&p);
    breakers.record_failure(&p);
    assert!(matches!(breakers.state(&p), BreakerState::Closed));
}
