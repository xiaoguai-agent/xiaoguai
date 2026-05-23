//! `LlmRouter` resolves a request to one or more backends and walks the
//! fallback chain when the first one's *initial* call fails. Once the stream
//! has started, errors are propagated to the caller as-is.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use xiaoguai_llm::{
    ChatRequest, LlmBackend, LlmError, LlmRouter, Message, MockBackend, ResolveCtx, RouterConfig,
};
use xiaoguai_types::{ProviderId, TenantId};

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
        tenant_id: None,
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
async fn tenant_default_used_when_no_explicit() {
    let p_global = ProviderId::new();
    let p_tenant = ProviderId::new();
    let tenant = TenantId::from("ten_alpha".to_string());

    let mut cfg = RouterConfig::default();
    cfg.system_default_for_model
        .insert("qwen".into(), p_global.clone());
    cfg.tenant_default_for_model
        .entry(tenant.clone())
        .or_default()
        .insert("qwen".into(), p_tenant.clone());

    let router = router_with(
        vec![
            (
                p_global.clone(),
                Arc::new(MockBackend::with_response("global-resp")) as Arc<dyn LlmBackend>,
            ),
            (
                p_tenant.clone(),
                Arc::new(MockBackend::with_response("tenant-resp")) as Arc<dyn LlmBackend>,
            ),
        ],
        cfg,
    );
    let ctx = ResolveCtx {
        tenant_id: Some(&tenant),
        explicit_provider: None,
        ..Default::default()
    };
    let stream = router.chat_stream(ctx, make_req("qwen")).await.expect("ok");
    assert_eq!(collect(stream).await, "tenant-resp");
}

#[tokio::test]
async fn system_default_used_when_no_tenant_default() {
    let p_global = ProviderId::new();
    let tenant = TenantId::from("ten_alpha".to_string());

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
        tenant_id: Some(&tenant),
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
        tenant_id: None,
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
        tenant_id: None,
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
        tenant_id: None,
        explicit_provider: Some(&unknown),
        ..Default::default()
    };
    let err = match router.chat_stream(ctx, make_req("anything")).await {
        Ok(_) => panic!("expected NoProvider"),
        Err(e) => e,
    };
    assert!(matches!(err, LlmError::NoProvider { .. }));
}

/// v0.6.4: the `LlmBackend` impl on `LlmRouter` reads
/// `ChatRequest::tenant_id` and builds a `ResolveCtx` from it. Two
/// backends, two tenants each with a different default — the per-request
/// tenant on the `ChatRequest` decides which backend wins.
#[tokio::test]
async fn llm_backend_impl_routes_by_request_tenant_id() {
    let p_a = ProviderId::new();
    let p_b = ProviderId::new();
    let backends: Vec<(ProviderId, Arc<dyn LlmBackend>)> = vec![
        (p_a.clone(), Arc::new(MockBackend::with_response("from A"))),
        (p_b.clone(), Arc::new(MockBackend::with_response("from B"))),
    ];

    let mut alpha_table = HashMap::new();
    alpha_table.insert("default-model".to_string(), p_a.clone());
    let mut bravo_table = HashMap::new();
    bravo_table.insert("default-model".to_string(), p_b.clone());

    let mut tenant_defaults = HashMap::new();
    tenant_defaults.insert(TenantId::from("ten_alpha".to_string()), alpha_table);
    tenant_defaults.insert(TenantId::from("ten_bravo".to_string()), bravo_table);

    let cfg = RouterConfig {
        tenant_default_for_model: tenant_defaults,
        ..RouterConfig::default()
    };
    let router: Arc<dyn LlmBackend> = Arc::new(router_with(backends, cfg));

    // Alpha tenant → backend A.
    let mut req = make_req("default-model");
    req.tenant_id = Some("ten_alpha".into());
    let stream_a = router.chat_stream(req).await.expect("a ok");
    assert_eq!(collect(stream_a).await, "from A");

    // Bravo tenant → backend B (proves the impl actually reads the field).
    let mut req = make_req("default-model");
    req.tenant_id = Some("ten_bravo".into());
    let stream_b = router.chat_stream(req).await.expect("b ok");
    assert_eq!(collect(stream_b).await, "from B");
}

/// v0.6.4: when `ChatRequest::tenant_id` is `None`, the `LlmBackend` impl
/// falls back to system defaults — legacy v0.6.2 behaviour preserved.
#[tokio::test]
async fn llm_backend_impl_falls_back_to_system_default_without_tenant() {
    let p_sys = ProviderId::new();
    let p_tenant = ProviderId::new();
    let backends: Vec<(ProviderId, Arc<dyn LlmBackend>)> = vec![
        (
            p_sys.clone(),
            Arc::new(MockBackend::with_response("system")),
        ),
        (
            p_tenant.clone(),
            Arc::new(MockBackend::with_response("tenant")),
        ),
    ];

    let mut sys_table = HashMap::new();
    sys_table.insert("m".to_string(), p_sys.clone());
    let mut alpha_table = HashMap::new();
    alpha_table.insert("m".to_string(), p_tenant.clone());
    let mut tenant_defaults = HashMap::new();
    tenant_defaults.insert(TenantId::from("ten_alpha".to_string()), alpha_table);

    let cfg = RouterConfig {
        system_default_for_model: sys_table,
        tenant_default_for_model: tenant_defaults,
        ..RouterConfig::default()
    };
    let router: Arc<dyn LlmBackend> = Arc::new(router_with(backends, cfg));

    // No tenant on the request → system default wins, not the tenant_a one.
    let req = make_req("m");
    assert!(req.tenant_id.is_none());
    let stream = router.chat_stream(req).await.expect("ok");
    assert_eq!(collect(stream).await, "system");
}
