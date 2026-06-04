//! Tier-3 T4: OAuth 2.1 PKCE end-to-end against a mockito-backed
//! token endpoint. Mirrors PR #72's in-memory-store + mock-server
//! pattern (no live PG, no real browser).
//!
//! Three cases:
//!   1. PKCE happy path — verifier round-trips, code → `access_token`.
//!   2. Refresh path — expired bundle triggers `refresh_pkce`,
//!      stored bundle updated.
//!   3. Refresh-token rotation — server-returned new `refresh_token`
//!      replaces old; absence preserves the old.

use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use sha2::{Digest as _, Sha256};
use xiaoguai_mcp::auth::oauth2_pkce::{
    build_http_client, exchange_code, new_pkce_pair, refresh_pkce, should_refresh,
    InMemoryTokenStore, OAuth2PkceConfig, TokenBundle, TokenStore, REFRESH_LEEWAY_SECS,
};

fn b64url_nopad(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    URL_SAFE_NO_PAD.encode(bytes)
}

fn challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    b64url_nopad(&hasher.finalize())
}

fn cfg(server_url: &str) -> OAuth2PkceConfig {
    OAuth2PkceConfig {
        auth_url: format!("{server_url}/authorize"),
        token_url: format!("{server_url}/token"),
        client_id: "test-client".into(),
        scopes: vec!["mcp.read".into()],
        redirect_uri: "http://127.0.0.1:9999/callback".into(),
    }
}

#[tokio::test]
async fn pkce_verifier_round_trips_through_token_endpoint() {
    let mut server = mockito::Server::new_async().await;
    let pair = new_pkce_pair();
    let expected_challenge = challenge_for(&pair.verifier);
    assert_eq!(pair.challenge, expected_challenge);

    // The token endpoint asserts:
    //  * grant_type = authorization_code
    //  * code = the value we POSTed
    //  * code_verifier = our verifier (and SHA256(verifier) == expected_challenge)
    let verifier_copy = pair.verifier.clone();
    let mock = server
        .mock("POST", "/token")
        .match_header("content-type", "application/x-www-form-urlencoded")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("grant_type".into(), "authorization_code".into()),
            mockito::Matcher::UrlEncoded("code".into(), "AUTHCODE_42".into()),
            mockito::Matcher::UrlEncoded("client_id".into(), "test-client".into()),
            mockito::Matcher::UrlEncoded("code_verifier".into(), verifier_copy),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"access_token":"ACCESS_1","refresh_token":"REFRESH_1","expires_in":3600,"token_type":"Bearer"}"#,
        )
        .create_async()
        .await;

    let http = build_http_client().expect("build client");
    let bundle = exchange_code(&http, &cfg(&server.url()), "AUTHCODE_42", &pair.verifier)
        .await
        .expect("exchange ok");

    assert_eq!(bundle.access_token, "ACCESS_1");
    assert_eq!(bundle.refresh_token.as_deref(), Some("REFRESH_1"));
    assert!(bundle.expires_at > Utc::now() + ChronoDuration::seconds(3500));
    mock.assert_async().await;
}

#[tokio::test]
async fn expired_access_token_triggers_refresh_and_updates_store() {
    let mut server = mockito::Server::new_async().await;
    // First call (refresh) returns a fresh access token, no new refresh_token.
    let mock_refresh = server
        .mock("POST", "/token")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
            mockito::Matcher::UrlEncoded("refresh_token".into(), "OLD_RT".into()),
            mockito::Matcher::UrlEncoded("client_id".into(), "test-client".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"access_token":"NEW_AT","expires_in":3600,"token_type":"Bearer"}"#)
        .create_async()
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let stale = TokenBundle {
        access_token: "STALE_AT".into(),
        refresh_token: Some("OLD_RT".into()),
        // Already expired.
        expires_at: Utc::now() - ChronoDuration::seconds(10),
    };
    store.put("srv-1", &stale).await.unwrap();
    assert!(should_refresh(&stale, Utc::now()));

    let oauth_cfg = cfg(&server.url());
    let http = build_http_client().unwrap();
    let refreshed = refresh_pkce(&http, &oauth_cfg, &stale)
        .await
        .expect("refresh ok");
    store.put("srv-1", &refreshed).await.unwrap();

    assert_eq!(refreshed.access_token, "NEW_AT");
    // RFC 6749 §6: when no new refresh_token is returned, keep the old one.
    assert_eq!(refreshed.refresh_token.as_deref(), Some("OLD_RT"));
    assert!(refreshed.expires_at > Utc::now() + ChronoDuration::seconds(REFRESH_LEEWAY_SECS));

    let from_store = store.get("srv-1").await.unwrap().unwrap();
    assert_eq!(from_store.access_token, "NEW_AT");
    assert_eq!(from_store.refresh_token.as_deref(), Some("OLD_RT"));
    mock_refresh.assert_async().await;
}

#[tokio::test]
async fn refresh_token_rotation_persists_new_refresh_token() {
    let mut server = mockito::Server::new_async().await;
    // Token endpoint returns a NEW refresh_token; stored bundle MUST use it.
    let mock_refresh = server
        .mock("POST", "/token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"access_token":"NEW_AT","refresh_token":"NEW_RT","expires_in":3600,"token_type":"Bearer"}"#,
        )
        .create_async()
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let stale = TokenBundle {
        access_token: "OLD_AT".into(),
        refresh_token: Some("OLD_RT".into()),
        expires_at: Utc::now() - ChronoDuration::seconds(10),
    };
    store.put("srv-2", &stale).await.unwrap();

    let oauth_cfg = cfg(&server.url());
    let http = build_http_client().unwrap();
    let refreshed = refresh_pkce(&http, &oauth_cfg, &stale).await.unwrap();
    // Atomic update: new bundle written in one `put`.
    store.put("srv-2", &refreshed).await.unwrap();

    let from_store = store.get("srv-2").await.unwrap().unwrap();
    assert_eq!(from_store.access_token, "NEW_AT");
    assert_eq!(from_store.refresh_token.as_deref(), Some("NEW_RT"));
    mock_refresh.assert_async().await;

    // Snapshot is keyed by server_id alone.
    let snap = store.snapshot();
    assert_eq!(snap.len(), 1);
    assert!(snap.contains_key("srv-2"));
}

#[tokio::test]
async fn token_endpoint_error_surfaces_as_auth_failed() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/token")
        .with_status(400)
        .with_header("content-type", "application/json")
        .with_body(r#"{"error":"invalid_grant","error_description":"bad code"}"#)
        .create_async()
        .await;

    let http = build_http_client().unwrap();
    let pair = new_pkce_pair();
    let err = exchange_code(&http, &cfg(&server.url()), "BAD_CODE", &pair.verifier)
        .await
        .expect_err("must fail");
    let msg = format!("{err}");
    assert!(msg.contains("invalid_grant"), "got {msg}");
    assert!(msg.contains("bad code"), "got {msg}");
}
