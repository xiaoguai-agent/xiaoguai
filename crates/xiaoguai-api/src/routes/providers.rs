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
//! provider takes effect on the next server restart (same as the boot-time
//! Casbin merge). The endpoints persist immediately; the running router does
//! not hot-reload.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
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

const fn default_fallback_order() -> i32 {
    100
}

/// Build the provider-management router (state = the repository).
pub fn build_router(repo: Repo) -> Router {
    Router::new()
        .route("/v1/admin/providers", get(list).post(create))
        .route(
            "/v1/admin/providers/{id}",
            axum::routing::delete(delete_provider),
        )
        .with_state(repo)
}

async fn list(State(repo): State<Repo>) -> Response {
    match repo.list_global().await {
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
    if req.name.trim().is_empty() {
        return bad_request("name is required");
    }

    let now = Utc::now();
    let prov = LlmProvider {
        id: ProviderId::new(),
        tenant_id: None,
        name: req.name.trim().to_string(),
        kind,
        endpoint: trimmed_endpoint,
        models: req.models,
        default_for_models: req.default_for_models,
        fallback_order: req.fallback_order,
        api_key_env: req.api_key_env.filter(|k| !k.trim().is_empty()),
        api_key: req.api_key.filter(|k| !k.trim().is_empty()),
        created_at: now,
        updated_at: now,
        cost_per_1k_input_usd: None,
        cost_per_1k_output_usd: None,
    };

    match repo.create(None, &prov).await {
        Ok(()) => (StatusCode::CREATED, Json(ProviderView::from(prov))).into_response(),
        Err(e) => server_error(&e),
    }
}

async fn delete_provider(State(repo): State<Repo>, Path(id): Path<String>) -> Response {
    match repo.delete(None, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(&e),
    }
}

fn bad_request(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

fn server_error(e: &impl std::fmt::Display) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
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

    // A trivial in-memory repo so the router can be exercised without Postgres.
    #[derive(Default)]
    struct MemRepo {
        rows: std::sync::Mutex<Vec<LlmProvider>>,
    }

    #[async_trait::async_trait]
    impl LlmProviderRepository for MemRepo {
        async fn create(&self, _tenant: Option<&str>, prov: &LlmProvider) -> RepoResult<()> {
            self.rows.lock().unwrap().push(prov.clone());
            Ok(())
        }
        async fn find_by_id(
            &self,
            _tenant: Option<&str>,
            id: &str,
        ) -> RepoResult<Option<LlmProvider>> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .find(|p| p.id.as_str() == id)
                .cloned())
        }
        async fn list_global(&self) -> RepoResult<Vec<LlmProvider>> {
            Ok(self.rows.lock().unwrap().clone())
        }
        async fn list_for_tenant(&self, _tenant_id: &str) -> RepoResult<Vec<LlmProvider>> {
            self.list_global().await
        }
        async fn delete(&self, _tenant: Option<&str>, id: &str) -> RepoResult<()> {
            let mut g = self.rows.lock().unwrap();
            let before = g.len();
            g.retain(|p| p.id.as_str() != id);
            if g.len() == before {
                return Err(RepoError::NotFound);
            }
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
}
