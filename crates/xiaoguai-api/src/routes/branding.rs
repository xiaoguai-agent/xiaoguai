//! White-label branding — the assistant's display name shown across the chat
//! UI. A self-contained router whose state is the [`SettingsRepository`], merged
//! into the app like `providers` (it sits OUTSIDE the `/v1` RBAC layer, so the
//! caller re-applies the owner auth gate). DEC-033 single owner: one row.
//!
//! Stored as a JSON blob under the `branding` settings key (migration 0040) so
//! the shape can grow (accent colour, tagline, avatar) without another
//! migration. Unset / empty `assistant_name` → the UI falls back to its
//! built-in default name ("Xiaoguai" / "小怪").

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use xiaoguai_storage::repositories::SettingsRepository;

use crate::error::ApiError;

type Repo = Arc<dyn SettingsRepository>;

/// Settings-store key the branding JSON blob lives under.
const BRANDING_KEY: &str = "branding";
/// Upper bound on the display name — keeps a pasted essay out of the header.
const MAX_NAME_LEN: usize = 64;

/// White-label branding the owner can set. An empty `assistant_name` means
/// "use the UI's built-in default" — the frontend substitutes its own string.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrandingSettings {
    #[serde(default)]
    pub assistant_name: String,
}

/// Build the branding router (state = the settings repository).
pub fn build_router(repo: Repo) -> Router {
    Router::new()
        .route("/v1/branding", get(get_branding).put(put_branding))
        .with_state(repo)
}

/// `GET /v1/branding` — current branding, or the empty default when unset.
async fn get_branding(State(repo): State<Repo>) -> Response {
    match repo.get(BRANDING_KEY).await {
        // A corrupt blob must not 500 the whole UI — degrade to the default so
        // the chat still renders (just with the built-in name).
        Ok(Some(json)) => Json(serde_json::from_str::<BrandingSettings>(&json).unwrap_or_default())
            .into_response(),
        Ok(None) => Json(BrandingSettings::default()).into_response(),
        Err(e) => server_error(&e),
    }
}

/// `PUT /v1/branding` — set the branding (owner-gated by the merge-time auth
/// layer). Trims the name and bounds its length; echoes the stored value back.
async fn put_branding(
    State(repo): State<Repo>,
    Json(mut body): Json<BrandingSettings>,
) -> Response {
    body.assistant_name = body.assistant_name.trim().to_string();
    if body.assistant_name.chars().count() > MAX_NAME_LEN {
        return bad_request(&format!(
            "assistant_name too long (max {MAX_NAME_LEN} chars)"
        ));
    }
    let json = match serde_json::to_string(&body) {
        Ok(j) => j,
        Err(e) => return server_error(&e),
    };
    match repo.set(BRANDING_KEY, &json).await {
        Ok(()) => Json(body).into_response(),
        Err(e) => server_error(&e),
    }
}

fn bad_request(msg: &str) -> Response {
    ApiError::BadRequest(msg.to_string()).into_response()
}

fn server_error(e: &impl std::fmt::Display) -> Response {
    // SEC-07: log the real cause server-side, return a generic body.
    ApiError::Internal(anyhow::anyhow!("branding: {e}")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use xiaoguai_storage::repositories::error::RepoResult;

    /// In-memory `SettingsRepository` so the handlers test without a DB.
    #[derive(Default)]
    struct MemSettings {
        store: Mutex<std::collections::HashMap<String, String>>,
    }

    #[async_trait]
    impl SettingsRepository for MemSettings {
        async fn get(&self, key: &str) -> RepoResult<Option<String>> {
            Ok(self.store.lock().unwrap().get(key).cloned())
        }
        async fn set(&self, key: &str, value: &str) -> RepoResult<()> {
            self.store
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn get_unset_returns_empty_default() {
        let repo: Repo = Arc::new(MemSettings::default());
        let body = to_branding(get_branding(State(repo)).await).await;
        assert_eq!(body, BrandingSettings::default());
        assert!(body.assistant_name.is_empty());
    }

    #[tokio::test]
    async fn put_then_get_round_trips_trimmed() {
        let repo: Repo = Arc::new(MemSettings::default());
        let put = put_branding(
            State(repo.clone()),
            Json(BrandingSettings {
                assistant_name: "  Acme 助手  ".to_string(),
            }),
        )
        .await;
        assert_eq!(
            to_branding(put).await.assistant_name,
            "Acme 助手",
            "trimmed"
        );

        let got = to_branding(get_branding(State(repo)).await).await;
        assert_eq!(got.assistant_name, "Acme 助手", "persisted");
    }

    #[tokio::test]
    async fn put_rejects_overlong_name() {
        let repo: Repo = Arc::new(MemSettings::default());
        let long = "x".repeat(MAX_NAME_LEN + 1);
        let resp = put_branding(
            State(repo),
            Json(BrandingSettings {
                assistant_name: long,
            }),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    /// Decode a handler `Response` body into `BrandingSettings`.
    async fn to_branding(resp: Response) -> BrandingSettings {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}
