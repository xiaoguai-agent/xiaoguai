//! `LlmRouter` resolves a request to one or more backends and walks the
//! fallback chain when the first one's *initial* call fails. Once the stream
//! has started, errors are propagated to the caller as-is.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use xiaoguai_llm::{
    ChatRequest, LlmBackend, LlmError, LlmRouter, Message, MockBackend, ResolveCtx, RouterConfig,
};
use xiaoguai_types::ProviderId;

fn make_req(model: &str) -> ChatRequest {
    ChatRequest::new(model, vec![Message::user("hi")])
}

async fn collect(mut s: xiaoguai_llm::ChatStream) -> String {
    let mut out = String::new();
    while let Some(c) = s.next().await {
        let c = c.expect("chunk");
        out.push_str(&c.delta);
    }
    out
}

fn router_with(backends: Vec<(ProviderId, Arc<dyn LlmBackend>)>, cfg: RouterConfig) -> LlmRouter {
    let map: HashMap<ProviderId, Arc<dyn LlmBackend>> = backends.into_iter().collect();
    LlmRouter::new(map, cfg)
}

#[tokio::test]
async fn empty_model_uses_default_model() {
    let p = ProviderId::new();
    let backends: Vec<(ProviderId, Arc<dyn LlmBackend>)> =
        vec![(p.clone(), Arc::new(MockBackend::with_response("defaulted")))];
    let mut cfg = RouterConfig::default();
    cfg.fallback_order.push(p);
    cfg.default_model = Some("MiniMax-M2".into());
    let router = router_with(backends, cfg);

    // Empty model -> router substitutes default_model, then routes via fallback.
    let stream = router
        .chat_stream(ResolveCtx::default(), make_req(""))
        .await
        .expect("ok");
    assert_eq!(collect(stream).await, "defaulted");
}

#[tokio::test]
async fn empty_model_without_default_errors() {
    let p = ProviderId::new();
    let backends: Vec<(ProviderId, Arc<dyn LlmBackend>)> =
        vec![(p.clone(), Arc::new(MockBackend::with_response("x")))];
    let mut cfg = RouterConfig::default();
    cfg.fallback_order.push(p); // no default_model configured
    let router = router_with(backends, cfg);

    // `ChatStream` isn't `Debug`, so match rather than `.expect_err()`.
    let res = router
        .chat_stream(ResolveCtx::default(), make_req("  "))
        .await;
    assert!(matches!(res, Err(LlmError::NoProvider(_))));
}

#[tokio::test]
async fn explicit_provider_wins() {
    let p_a = ProviderId::new();
    let p_b = ProviderId::new();
    let backends: Vec<(ProviderId, Arc<dyn LlmBackend>)> = vec![
        (p_a.clone(), Arc::new(MockBackend::with_response("A wins"))),
        (p_b.clone(), Arc::new(MockBackend::with_response("B wins"))),
    ];
    let cfg = RouterConfig::default();
    let router = router_with(backends, cfg);

    let ctx = ResolveCtx {
        explicit_provider: Some(&p_b),
        ..Default::default()
    };
    let stream = router
        .chat_stream(ctx, make_req("anything"))
        .await
        .expect("ok");
    assert_eq!(collect(stream).await, "B wins");
}

#[tokio::test]
async fn system_default_used_when_no_explicit() {
    let p_global = ProviderId::new();

    let mut cfg = RouterConfig::default();
    cfg.system_default_for_model
        .insert("qwen".into(), p_global.clone());

    let router = router_with(
        vec![(
            p_global,
            Arc::new(MockBackend::with_response("global-resp")) as Arc<dyn LlmBackend>,
        )],
        cfg,
    );
    let ctx = ResolveCtx {
        explicit_provider: None,
        ..Default::default()
    };
    let stream = router.chat_stream(ctx, make_req("qwen")).await.expect("ok");
    assert_eq!(collect(stream).await, "global-resp");
}

#[tokio::test]
async fn fallback_chain_walks_to_next_on_failure() {
    let p_a = ProviderId::new();
    let p_b = ProviderId::new();
    let p_c = ProviderId::new();

    let cfg = RouterConfig {
        fallback_order: vec![p_a.clone(), p_b.clone(), p_c.clone()],
        ..RouterConfig::default()
    };

    let router = router_with(
        vec![
            (
                p_a,
                Arc::new(MockBackend::failing(LlmError::Provider("503".into())))
                    as Arc<dyn LlmBackend>,
            ),
            (
                p_b,
                Arc::new(MockBackend::failing(LlmError::Network(
                    "conn refused".into(),
                ))) as Arc<dyn LlmBackend>,
            ),
            (
                p_c,
                Arc::new(MockBackend::with_response("third time lucky")) as Arc<dyn LlmBackend>,
            ),
        ],
        cfg,
    );
    let ctx = ResolveCtx {
        explicit_provider: None,
        ..Default::default()
    };
    let stream = router
        .chat_stream(ctx, make_req("anything"))
        .await
        .expect("ok");
    assert_eq!(collect(stream).await, "third time lucky");
}

#[tokio::test]
async fn no_provider_when_all_fail() {
    let p_a = ProviderId::new();
    let cfg = RouterConfig {
        fallback_order: vec![p_a.clone()],
        ..RouterConfig::default()
    };

    let router = router_with(
        vec![(
            p_a,
            Arc::new(MockBackend::failing(LlmError::Provider("dead".into())))
                as Arc<dyn LlmBackend>,
        )],
        cfg,
    );
    let ctx = ResolveCtx {
        explicit_provider: None,
        ..Default::default()
    };
    let err = match router.chat_stream(ctx, make_req("anything")).await {
        Ok(_) => panic!("expected NoProvider"),
        Err(e) => e,
    };
    assert!(matches!(err, LlmError::NoProvider { .. }));
}

#[tokio::test]
async fn explicit_provider_missing_is_error() {
    let cfg = RouterConfig::default();
    let router = router_with(vec![], cfg);
    let unknown = ProviderId::from("prov_does_not_exist".to_string());
    let ctx = ResolveCtx {
        explicit_provider: Some(&unknown),
        ..Default::default()
    };
    let err = match router.chat_stream(ctx, make_req("anything")).await {
        Ok(_) => panic!("expected NoProvider"),
        Err(e) => e,
    };
    assert!(matches!(err, LlmError::NoProvider { .. }));
}

/// The `LlmBackend` impl on `LlmRouter` resolves system defaults +
/// fallback (single-owner model — no per-request tenant routing).
#[tokio::test]
async fn llm_backend_impl_uses_system_default() {
    let p_sys = ProviderId::new();
    let backends: Vec<(ProviderId, Arc<dyn LlmBackend>)> = vec![(
        p_sys.clone(),
        Arc::new(MockBackend::with_response("system")),
    )];

    let mut sys_table = HashMap::new();
    sys_table.insert("m".to_string(), p_sys.clone());

    let cfg = RouterConfig {
        system_default_for_model: sys_table,
        ..RouterConfig::default()
    };
    let router: Arc<dyn LlmBackend> = Arc::new(router_with(backends, cfg));

    let req = make_req("m");
    let stream = router.chat_stream(req).await.expect("ok");
    assert_eq!(collect(stream).await, "system");
}
