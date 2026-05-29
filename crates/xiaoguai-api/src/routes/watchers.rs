//! REST handlers for `/v1/watchers/*` (sprint-10b S10b-5).
//!
//! These three endpoints back `XiaoguaiClient.listSessionWatchers /
//! pauseWatcher / resumeWatcher` in `frontend/shared/src/index.ts`. The
//! frontend client falls back to `[]` on 404/503; this mount lets the
//! `<WatchIndicator>` UI render a real 200 + empty-array steady state.
//!
//! All routes return 503 when `AppState.watchers` is `None`; production
//! wires a [`crate::watchers::StaticWatcherIntrospector`] at minimum.
//!
//! ## Routes
//!
//! | Method | Path                                  | Notes                            |
//! |--------|---------------------------------------|----------------------------------|
//! | GET    | `/v1/watchers?session_id=<id>`        | List watchers for a session      |
//! | POST   | `/v1/watchers/:id/pause`              | Idempotent pause                 |
//! | POST   | `/v1/watchers/:id/resume`             | Resume from paused / error       |

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::state::AppState;
use crate::watchers::WatcherError;

#[derive(Debug, Deserialize)]
pub struct ListWatchersQuery {
    pub session_id: String,
}

fn watchers_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error": "watcher introspector not configured"})),
    )
        .into_response()
}

fn map_err(e: WatcherError) -> Response {
    match e {
        WatcherError::NotFound(id) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("watcher not found: {id}")})),
        )
            .into_response(),
        WatcherError::Backend(msg) => {
            tracing::error!(error = %msg, "watcher backend error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

/// `GET /v1/watchers?session_id=<id>` — returns the watcher list as a
/// bare JSON array (matches the TS `Promise<WatcherInfo[]>` return type).
pub async fn list_watchers(
    State(state): State<AppState>,
    Query(q): Query<ListWatchersQuery>,
) -> Response {
    let Some(intro) = state.watchers.clone() else {
        return watchers_unavailable();
    };
    match intro.list_for_session(&q.session_id).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => map_err(e),
    }
}

/// `POST /v1/watchers/:id/pause` — idempotent pause.
pub async fn pause_watcher(
    State(state): State<AppState>,
    Path(watcher_id): Path<String>,
) -> Response {
    let Some(intro) = state.watchers.clone() else {
        return watchers_unavailable();
    };
    match intro.pause(&watcher_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

/// `POST /v1/watchers/:id/resume` — resume from paused / error state.
pub async fn resume_watcher(
    State(state): State<AppState>,
    Path(watcher_id): Path<String>,
) -> Response {
    let Some(intro) = state.watchers.clone() else {
        return watchers_unavailable();
    };
    match intro.resume(&watcher_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}
