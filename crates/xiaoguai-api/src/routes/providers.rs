//! Web-UI LLM provider management: list / create / delete.
//!
//! Self-contained router whose state is the [`LlmProviderRepository`], merged
//! into the app like `mcp_serve` — it sits OUTSIDE the `/v1` RBAC layer, so
//! operators who enable auth should also protect `/v1/admin/providers` at their
//! reverse proxy (same boundary as `/v1/mcp/serve`). The target deployment is
//! single-tenant / personal with `auth.required: false`.
//!
//! A provider points at a local model URL (`ollama` / `openai_compat` with a
//! `http://host:port` endpoint) or a hosted API (`minimax`, `openai_compat` for
//! Zhipu / `OpenAI` / `DeepSeek`, etc.). A create request may carry `api_key`
//! (stored in the DB, used directly) or `api_key_env` (env-var name). The
//! stored key is NEVER returned — list items expose only `has_api_key`.
//!
//! NOTE: the `LlmRouter` is built once at boot, so a newly created/deleted
//! provider takes effect on the next server restart. The endpoints persist
//! immediately; the running router does not hot-reload.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use xiaoguai_llm::ModelProbe;
use xiaoguai_storage::repositories::error::RepoError;
use xiaoguai_storage::repositories::LlmProviderRepository;
use xiaoguai_types::{LlmProvider, ProviderId, ProviderKind};

type Repo = Arc<dyn LlmProviderRepository>;

/// Comma-joined list of accepted `kind` values, for error messages.
const KINDS: &str =
    "ollama, openai_compat, anthropic, gemini, bedrock, azure_openai, mistral, groq, minimax";

/// A provider as returned to clients — note the absence of the secret
/// `api_key`; it's projected to `has_api_key`.
#[derive(Serialize)]
struct ProviderView {
    id: String,
    name: String,
    kind: String,
    endpoint: String,
    models: Vec<String>,
    default_for_models: Vec<String>,
    /// Probe-confirmed models, or `null` when never probed. The chat model
    /// picker prefers this over `models` so it can offer only models that
    /// actually connect. Populated by `POST /v1/admin/providers/{id}/probe`.
    verified_models: Option<Vec<String>>,
    fallback_order: i32,
    api_key_env: Option<String>,
    has_api_key: bool,
}

impl From<LlmProvider> for ProviderView {
    fn from(p: LlmProvider) -> Self {
        Self {
            id: p.id.as_str().to_string(),
            name: p.name,
            kind: p.kind.as_str().to_string(),
            endpoint: p.endpoint,
            models: p.models,
            default_for_models: p.default_for_models,
            verified_models: p.verified_models,
            fallback_order: p.fallback_order,
            api_key_env: p.api_key_env,
            has_api_key: p.api_key.as_deref().is_some_and(|k| !k.is_empty()),
        }
    }
}

#[derive(Deserialize)]
struct CreateProviderRequest {
    name: String,
    /// One of [`KINDS`].
    kind: String,
    /// Base URL — a local server (`http://localhost:11434`) or a hosted API
    /// (`https://api.minimax.io`). May be empty only for `bedrock` (region).
    endpoint: String,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    default_for_models: Vec<String>,
    #[serde(default = "default_fallback_order")]
    fallback_order: i32,
    /// API key stored directly (hosted APIs). Omit for local / unauthenticated.
    #[serde(default)]
    api_key: Option<String>,
    /// Alternative: name of an env var holding the key.
    #[serde(default)]
    api_key_env: Option<String>,
}

/// Body for `PUT /v1/admin/providers/{id}` — every field optional; only the
/// provided fields change. `api_key` sets/replaces the stored key when present
/// and non-empty (omit it or send empty to KEEP the existing key — so editing
/// the endpoint/models never wipes the stored secret).
#[derive(Deserialize)]
struct UpdateProviderRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    models: Option<Vec<String>>,
    #[serde(default)]
    default_for_models: Option<Vec<String>>,
    #[serde(default)]
    fallback_order: Option<i32>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
}

const fn default_fallback_order() -> i32 {
    100
}

/// Build the provider-management router (state = the repository).
pub fn build_router(repo: Repo) -> Router {
    Router::new()
        .route("/v1/admin/providers", get(list).post(create))
        .route(
            "/v1/admin/providers/{id}",
            axum::routing::put(update).delete(delete_provider),
        )
        .route("/v1/admin/providers/{id}/probe", axum::routing::post(probe))
        .with_state(repo)
}

async fn list(State(repo): State<Repo>) -> Response {
    match repo.list().await {
        Ok(rows) => {
            Json(rows.into_iter().map(ProviderView::from).collect::<Vec<_>>()).into_response()
        }
        Err(e) => server_error(&e),
    }
}

async fn create(State(repo): State<Repo>, Json(req): Json<CreateProviderRequest>) -> Response {
    let Some(kind) = ProviderKind::parse(&req.kind) else {
        return bad_request(&format!(
            "unknown provider kind '{}'; one of: {KINDS}",
            req.kind
        ));
    };
    let trimmed_endpoint = req.endpoint.trim().to_string();
    if trimmed_endpoint.is_empty() && kind != ProviderKind::Bedrock {
        return bad_request("endpoint is required (a local URL or a hosted API base URL)");
    }
    // The backend POSTs the stored API key to this endpoint, so reject anything
    // that isn't an http(s) URL (blocks `file://`/`gopher://`-style schemes).
    // Bedrock's "endpoint" is an AWS region, not a URL — skip the scheme check.
    if !trimmed_endpoint.is_empty()
        && kind != ProviderKind::Bedrock
        && !(trimmed_endpoint.starts_with("http://") || trimmed_endpoint.starts_with("https://"))
    {
        return bad_request("endpoint must be an http(s) URL");
    }
    if req.name.trim().is_empty() {
        return bad_request("name is required");
    }

    let now = Utc::now();
    let prov = LlmProvider {
        id: ProviderId::new(),
        name: req.name.trim().to_string(),
        kind,
        endpoint: trimmed_endpoint,
        models: req.models,
        default_for_models: req.default_for_models,
        verified_models: None,
        fallback_order: req.fallback_order,
        api_key_env: req.api_key_env.filter(|k| !k.trim().is_empty()),
        api_key: req.api_key.filter(|k| !k.trim().is_empty()),
        created_at: now,
        updated_at: now,
        cost_per_1k_input_usd: None,
        cost_per_1k_output_usd: None,
    };

    match repo.create(&prov).await {
        Ok(()) => (StatusCode::CREATED, Json(ProviderView::from(prov))).into_response(),
        // A duplicate name is a client error, not a 500 — and don't echo the raw
        // DB message back to the caller.
        Err(RepoError::DuplicateKey(_)) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": format!("a provider named '{}' already exists", prov.name) })),
        )
            .into_response(),
        Err(e) => server_error(&e),
    }
}

/// `PUT /v1/admin/providers/{id}` — edit an existing provider: paste a
/// `MiniMax` API key onto the seeded row, or switch its endpoint to the China
/// base URL (`https://api.minimaxi.com`). Only the fields present in the body
/// change; an omitted/empty `api_key` keeps the stored secret. Like create, the
/// running `LlmRouter` only picks up the change after a server restart.
async fn update(
    State(repo): State<Repo>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProviderRequest>,
) -> Response {
    let Some(mut prov) = (match repo.find_by_id(&id).await {
        Ok(p) => p,
        Err(e) => return server_error(&e),
    }) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "provider not found" })),
        )
            .into_response();
    };

    // Track edits that change what a probe would measure (endpoint / model list
    // / key). Any of these invalidates a prior `verified_models` result, so we
    // clear it below — otherwise the chat picker would keep offering a stale set
    // (hiding a just-added model, or still offering a removed/now-401 one).
    let mut connectivity_changed = false;

    if let Some(name) = req.name {
        if name.trim().is_empty() {
            return bad_request("name must not be empty");
        }
        prov.name = name.trim().to_string();
    }
    if let Some(endpoint) = req.endpoint {
        let e = endpoint.trim().to_string();
        if e.is_empty() && prov.kind != ProviderKind::Bedrock {
            return bad_request("endpoint is required (a local URL or a hosted API base URL)");
        }
        if !e.is_empty()
            && prov.kind != ProviderKind::Bedrock
            && !(e.starts_with("http://") || e.starts_with("https://"))
        {
            return bad_request("endpoint must be an http(s) URL");
        }
        if e != prov.endpoint {
            connectivity_changed = true;
        }
        prov.endpoint = e;
    }
    if let Some(models) = req.models {
        if models != prov.models {
            connectivity_changed = true;
        }
        prov.models = models;
    }
    if let Some(dfm) = req.default_for_models {
        prov.default_for_models = dfm;
    }
    if let Some(fo) = req.fallback_order {
        prov.fallback_order = fo;
    }
    // api_key: set only when a non-empty value is sent; omit/empty = keep.
    if let Some(key) = req.api_key {
        if !key.trim().is_empty() {
            prov.api_key = Some(key.trim().to_string());
            connectivity_changed = true;
        }
    }
    // A changed endpoint/models/key makes any prior probe result stale — drop it
    // so the picker falls back to advertised models until the next probe.
    if connectivity_changed {
        prov.verified_models = None;
    }
    if let Some(env) = req.api_key_env {
        prov.api_key_env = Some(env.trim().to_string()).filter(|s| !s.is_empty());
    }
    prov.updated_at = Utc::now();

    match repo.update(&prov).await {
        Ok(()) => (StatusCode::OK, Json(ProviderView::from(prov))).into_response(),
        Err(RepoError::DuplicateKey(_)) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": format!("a provider named '{}' already exists", prov.name) })),
        )
            .into_response(),
        Err(e) => server_error(&e),
    }
}

/// Response body for `POST /v1/admin/providers/{id}/probe`.
#[derive(Serialize)]
struct ProbeResponse {
    /// One entry per advertised model, in declared order.
    results: Vec<ModelProbe>,
    /// The subset of `results` that connected — persisted to the provider's
    /// `verified_models` and what the chat picker will offer.
    verified: Vec<String>,
}

/// `POST /v1/admin/providers/{id}/probe` — live connectivity check. Fires a
/// minimal chat request at each model this provider advertises (straight at the
/// provider, not the fallback chain) and persists the set that responded to
/// `verified_models`, so the chat model picker can offer only models that
/// actually connect. Returns the per-model results (incl. failure reasons).
///
/// This issues real (tiny) LLM calls, so it's an explicit operator action, not
/// something run on every save.
async fn probe(State(repo): State<Repo>, Path(id): Path<String>) -> Response {
    let Some(prov) = (match repo.find_by_id(&id).await {
        Ok(p) => p,
        Err(e) => return server_error(&e),
    }) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "provider not found" })),
        )
            .into_response();
    };

    let results = xiaoguai_llm::probe_provider(&prov).await;
    let verified: Vec<String> = results
        .iter()
        .filter(|r| r.ok)
        .map(|r| r.model.clone())
        .collect();

    // Narrow write: persist ONLY the verified set — never re-write api_key (a
    // full update() would re-conceal the revealed key and could NULL it out if
    // it currently can't be decrypted).
    if let Err(e) = repo.update_verified_models(&id, &verified).await {
        return server_error(&e);
    }

    Json(ProbeResponse { results, verified }).into_response()
}

async fn delete_provider(State(repo): State<Repo>, Path(id): Path<String>) -> Response {
    match repo.delete(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

fn bad_request(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

fn server_error(e: &impl std::fmt::Display) -> Response {
    // SEC-07: log the real error server-side but return a generic message so
    // DB/backend internals (table names, SQL fragments, paths) never reach the
    // client. Mirrors the centralised `ApiError` 5xx mapping.
    tracing::error!(error = %e, "provider endpoint internal error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal error" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    use xiaoguai_storage::repositories::error::{RepoError, RepoResult};

    // A trivial in-memory repo so the router can be exercised without a database.
    #[derive(Default)]
    struct MemRepo {
        rows: std::sync::Mutex<Vec<LlmProvider>>,
    }

    #[async_trait::async_trait]
    impl LlmProviderRepository for MemRepo {
        async fn create(&self, prov: &LlmProvider) -> RepoResult<()> {
            self.rows.lock().unwrap().push(prov.clone());
            Ok(())
        }
        async fn find_by_id(&self, id: &str) -> RepoResult<Option<LlmProvider>> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .find(|p| p.id.as_str() == id)
                .cloned())
        }
        async fn list(&self) -> RepoResult<Vec<LlmProvider>> {
            Ok(self.rows.lock().unwrap().clone())
        }
        async fn delete(&self, id: &str) -> RepoResult<()> {
            let mut g = self.rows.lock().unwrap();
            let before = g.len();
            g.retain(|p| p.id.as_str() != id);
            if g.len() == before {
                return Err(RepoError::NotFound);
            }
            Ok(())
        }
        async fn update(&self, prov: &LlmProvider) -> RepoResult<()> {
            let mut g = self.rows.lock().unwrap();
            let Some(slot) = g.iter_mut().find(|p| p.id == prov.id) else {
                return Err(RepoError::NotFound);
            };
            *slot = prov.clone();
            Ok(())
        }
        async fn update_verified_models(&self, id: &str, verified: &[String]) -> RepoResult<()> {
            let mut g = self.rows.lock().unwrap();
            let Some(slot) = g.iter_mut().find(|p| p.id.as_str() == id) else {
                return Err(RepoError::NotFound);
            };
            slot.verified_models = Some(verified.to_vec());
            Ok(())
        }
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    #[tokio::test]
    async fn create_then_list_hides_key_but_flags_has_api_key() {
        let repo: Repo = Arc::new(MemRepo::default());
        let app = build_router(repo);

        let create = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"MiniMax","kind":"minimax","endpoint":"https://api.minimax.io","models":["MiniMax-M2"],"api_key":"secret-xyz"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);

        let list = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let v = body_json(list).await;
        assert_eq!(v[0]["name"], "MiniMax");
        assert_eq!(v[0]["kind"], "minimax");
        assert_eq!(v[0]["has_api_key"], true);
        // The secret must never be serialised.
        assert!(v[0].get("api_key").is_none());
        assert!(!v.to_string().contains("secret-xyz"));
    }

    #[tokio::test]
    async fn probe_persists_verified_models_and_reports_per_model() {
        let repo: Repo = Arc::new(MemRepo::default());
        let app = build_router(repo);

        // Point at an unroutable port so every model probe fails fast — this
        // exercises the route + persistence without reaching the network.
        let create = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Unreachable","kind":"openai_compat","endpoint":"http://127.0.0.1:1","models":["m-a","m-b"],"api_key":"k"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let id = body_json(create).await["id"].as_str().unwrap().to_string();

        let probe = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/admin/providers/{id}/probe"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(probe.status(), StatusCode::OK);
        let v = body_json(probe).await;
        // One result per advertised model, all failed, none verified.
        assert_eq!(v["results"].as_array().unwrap().len(), 2);
        assert_eq!(v["results"][0]["ok"], false);
        assert!(v["results"][0]["error"].is_string());
        assert_eq!(v["verified"].as_array().unwrap().len(), 0);

        // The (empty) verified set is persisted and surfaced on the list view.
        let list = app
            .oneshot(
                Request::builder()
                    .uri("/v1/admin/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let lv = body_json(list).await;
        assert_eq!(lv[0]["verified_models"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn probe_unknown_provider_is_404() {
        let repo: Repo = Arc::new(MemRepo::default());
        let app = build_router(repo);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/providers/nope/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unknown_kind_is_rejected() {
        let repo: Repo = Arc::new(MemRepo::default());
        let app = build_router(repo);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"x","kind":"vertexai","endpoint":"http://x"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn local_url_provider_needs_no_key() {
        let repo: Repo = Arc::new(MemRepo::default());
        let app = build_router(repo);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"local ollama","kind":"ollama","endpoint":"http://localhost:11434","models":["qwen2.5-coder"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let v = body_json(resp).await;
        assert_eq!(v["has_api_key"], false);
    }

    #[tokio::test]
    async fn update_sets_key_and_switches_endpoint() {
        let repo: Repo = Arc::new(MemRepo::default());
        let app = build_router(repo);

        // Seed a keyless minimax provider (mirrors the bundled seed row).
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"minimax","kind":"minimax","endpoint":"https://api.minimax.io","models":["MiniMax-M2"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let id = body_json(created).await["id"].as_str().unwrap().to_string();

        // Paste a key + switch to the China domestic endpoint via PUT.
        let updated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/admin/providers/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"endpoint":"https://api.minimaxi.com","api_key":"sk-cn-123"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        let v = body_json(updated).await;
        assert_eq!(v["endpoint"], "https://api.minimaxi.com");
        assert_eq!(v["has_api_key"], true);
        assert!(
            !v.to_string().contains("sk-cn-123"),
            "key must stay server-side"
        );

        // A later edit that omits api_key must KEEP the stored key.
        let models_only = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/admin/providers/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"models":["MiniMax-M2","MiniMax-M2.1"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(models_only.status(), StatusCode::OK);
        assert_eq!(body_json(models_only).await["has_api_key"], true);
    }

    #[tokio::test]
    async fn update_unknown_id_is_404() {
        let repo: Repo = Arc::new(MemRepo::default());
        let app = build_router(repo);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/admin/providers/does-not-exist")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"api_key":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
