//! Wire `LlmRouter` with `MemoryUsageSink` and assert that each successful
//! `chat_stream` call produces exactly one `UsageRecord`.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use xiaoguai_llm::{
    ChatRequest, LlmBackend, LlmError, LlmRouter, MemoryUsageSink, Message, MockBackend,
    ResolveCtx, RouterConfig,
};
use xiaoguai_types::{ProviderId, SessionId, UserId};

fn make_req(model: &str) -> ChatRequest {
    ChatRequest::new(model, vec![Message::user("hi")])
}

async fn drain(mut s: xiaoguai_llm::ChatStream) -> String {
    let mut out = String::new();
    while let Some(c) = s.next().await {
        out.push_str(&c.expect("chunk").delta);
    }
    out
}

#[tokio::test]
async fn one_record_per_successful_call() {
    let prov = ProviderId::new();
    let sink = Arc::new(MemoryUsageSink::new());

    let mut backends: HashMap<ProviderId, Arc<dyn LlmBackend>> = HashMap::new();
    backends.insert(prov.clone(), Arc::new(MockBackend::with_response("done")));

    let router = LlmRouter::new(
        backends,
        RouterConfig {
            fallback_order: vec![prov.clone()],
            ..Default::default()
        },
    )
    .with_usage_sink(sink.clone());

    let user = UserId::from("usr_b".to_string());
    let session = SessionId::from("sess_c".to_string());
    let ctx = ResolveCtx {
        user_id: Some(&user),
        session_id: Some(&session),
        request_id: Some("req_42"),
        ..Default::default()
    };

    let stream = router.chat_stream(ctx, make_req("any")).await.expect("ok");
    let text = drain(stream).await;
    assert_eq!(text, "done");

    let records = sink.records();
    assert_eq!(records.len(), 1);
    let r = &records[0];
    assert_eq!(r.user_id.as_ref().map(AsRef::as_ref), Some("usr_b"));
    assert_eq!(r.session_id.as_ref().map(AsRef::as_ref), Some("sess_c"));
    assert_eq!(r.provider_id.as_str(), prov.as_str());
    assert_eq!(r.model, "any");
    assert_eq!(r.request_id.as_deref(), Some("req_42"));
}

#[tokio::test]
async fn no_record_when_resolution_fails() {
    let sink = Arc::new(MemoryUsageSink::new());
    let router =
        LlmRouter::new(HashMap::new(), RouterConfig::default()).with_usage_sink(sink.clone());

    let err = match router
        .chat_stream(ResolveCtx::default(), make_req("x"))
        .await
    {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(matches!(err, LlmError::NoProvider(_)));
    assert_eq!(sink.records().len(), 0);
}

#[tokio::test]
async fn no_record_when_all_backends_fail() {
    let prov = ProviderId::new();
    let sink = Arc::new(MemoryUsageSink::new());

    let mut backends: HashMap<ProviderId, Arc<dyn LlmBackend>> = HashMap::new();
    backends.insert(
        prov.clone(),
        Arc::new(MockBackend::failing(LlmError::Provider("nope".into()))),
    );

    let router = LlmRouter::new(
        backends,
        RouterConfig {
            fallback_order: vec![prov],
            ..Default::default()
        },
    )
    .with_usage_sink(sink.clone());

    let result = router
        .chat_stream(ResolveCtx::default(), make_req("any"))
        .await;
    assert!(result.is_err(), "expected NoProvider");
    assert_eq!(sink.records().len(), 0);
}

#[tokio::test]
async fn fallback_winner_is_recorded_not_failing_one() {
    let p_fail = ProviderId::new();
    let p_ok = ProviderId::new();
    let sink = Arc::new(MemoryUsageSink::new());

    let mut backends: HashMap<ProviderId, Arc<dyn LlmBackend>> = HashMap::new();
    backends.insert(
        p_fail.clone(),
        Arc::new(MockBackend::failing(LlmError::Provider("503".into()))),
    );
    backends.insert(p_ok.clone(), Arc::new(MockBackend::with_response("ok")));

    let router = LlmRouter::new(
        backends,
        RouterConfig {
            fallback_order: vec![p_fail.clone(), p_ok.clone()],
            ..Default::default()
        },
    )
    .with_usage_sink(sink.clone());

    let stream = router
        .chat_stream(ResolveCtx::default(), make_req("any"))
        .await
        .expect("ok");
    let _ = drain(stream).await;

    let records = sink.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].provider_id.as_str(), p_ok.as_str());
}
