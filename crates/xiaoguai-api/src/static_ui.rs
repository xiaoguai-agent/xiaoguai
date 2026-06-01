//! Optional static web-UI serving.
//!
//! When `server.static_dir` is configured (and exists), `xiaoguai-core`
//! serves the bundled single-page apps from it:
//!   - **chat-ui** at `/` (the router fallback) — the end-user chat surface.
//!   - **admin-ui** at `/admin/` — the operator console.
//!
//! API routes (`/v1/*`, `/healthz`) keep precedence because the static
//! services are attached as a nested service (`/admin`) plus the router's
//! fallback, both of which only handle requests no API route matched.
//!
//! Each app gets an `index.html` SPA fallback so client-side routes (e.g.
//! `/conversations/123`) resolve to the app shell instead of 404ing.
//!
//! admin-ui MUST be built with Vite `base: "/admin/"` so its asset URLs are
//! `/admin/assets/...` (served by the nested `/admin` service) rather than
//! `/assets/...` (which the chat-ui fallback owns). chat-ui keeps `base: "/"`.

use std::path::Path;

use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

/// Sub-directory names under `static_dir` for each bundled SPA.
const CHAT_UI_SUBDIR: &str = "chat-ui";
const ADMIN_UI_SUBDIR: &str = "admin-ui";
const INDEX_HTML: &str = "index.html";

/// Mount the bundled web UIs onto `app`.
///
/// Reads `<static_dir>/chat-ui` and `<static_dir>/admin-ui`. chat-ui becomes
/// the router's fallback (served at `/` and for any unmatched asset path);
/// admin-ui is nested at `/admin`. Missing sub-directories are tolerated —
/// `ServeDir` simply 404s — so a partial bundle never panics.
///
/// The caller is expected to have verified `static_dir` exists; this function
/// does not (it only wires services).
pub fn mount_static_ui(app: Router, static_dir: &Path) -> Router {
    let chat_dir = static_dir.join(CHAT_UI_SUBDIR);
    let admin_dir = static_dir.join(ADMIN_UI_SUBDIR);

    let chat_service = ServeDir::new(&chat_dir).fallback(ServeFile::new(chat_dir.join(INDEX_HTML)));
    let admin_service =
        ServeDir::new(&admin_dir).fallback(ServeFile::new(admin_dir.join(INDEX_HTML)));

    app.nest_service("/admin", admin_service)
        .fallback_service(chat_service)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt; // for `oneshot`

    use super::*;

    /// Build a temp static dir with distinguishable chat-ui + admin-ui shells.
    fn fixture_static_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let chat = dir.path().join(CHAT_UI_SUBDIR);
        let admin = dir.path().join(ADMIN_UI_SUBDIR);
        fs::create_dir_all(&chat).unwrap();
        fs::create_dir_all(&admin).unwrap();
        fs::write(chat.join(INDEX_HTML), "<html>chat-ui shell</html>").unwrap();
        fs::write(chat.join("app.js"), "// chat asset").unwrap();
        fs::write(admin.join(INDEX_HTML), "<html>admin-ui shell</html>").unwrap();
        dir
    }

    /// A stand-in API router so we can prove API routes win over the static
    /// fallback (mirrors how `xiaoguai_api::router` precedes the static mount).
    fn api_app() -> Router {
        Router::new().route("/healthz", get(|| async { "ok" }))
    }

    async fn body_string(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn api_route_wins_over_static_fallback() {
        let dir = fixture_static_dir();
        let app = mount_static_ui(api_app(), dir.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, "ok");
    }

    #[tokio::test]
    async fn root_serves_chat_ui_index() {
        let dir = fixture_static_dir();
        let app = mount_static_ui(api_app(), dir.path());
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("chat-ui shell"));
    }

    #[tokio::test]
    async fn admin_path_serves_admin_ui_index() {
        let dir = fixture_static_dir();
        let app = mount_static_ui(api_app(), dir.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/admin/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("admin-ui shell"));
    }

    #[tokio::test]
    async fn unknown_client_route_falls_back_to_chat_index() {
        // SPA deep link: no such file, must serve chat-ui's index.html (200),
        // not 404, so the client router can take over.
        let dir = fixture_static_dir();
        let app = mount_static_ui(api_app(), dir.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/conversations/abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("chat-ui shell"));
    }

    #[tokio::test]
    async fn chat_ui_asset_is_served_verbatim() {
        let dir = fixture_static_dir();
        let app = mount_static_ui(api_app(), dir.path());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, "// chat asset");
    }
}
