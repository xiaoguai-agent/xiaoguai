//! `xiaoguai mcp {register,list,remove}` — administer the MCP server
//! registry stored in `SQLite`.
//!
//! Mirrors `commands::provider` exactly: pure functions taking a
//! `&dyn McpServerRepository`, so unit tests can swap in an in-memory
//! implementation.
//!
//! Secrets policy: registrations accept `--env-keys FOO,BAR` (env-variable
//! NAMES only). Values are resolved by the spawning code at supervisor
//! start time — never persisted in the database or shell history.
//!
//! Tier-3 T4 (2026-05-29): adds `--auth oauth2-pkce` which runs a
//! browser-redirect consent flow at register time. The acquired
//! `TokenBundle` is persisted via the supplied [`TokenStore`].

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use xiaoguai_mcp::auth::{
    build_authorize_url, exchange_code, new_pkce_pair, new_state, oauth2_pkce::build_http_client,
    OAuth2PkceConfig, TokenBundle, TokenStore,
};
use xiaoguai_storage::repositories::McpServerRepository;
use xiaoguai_types::{ids::McpServerInstanceId, McpServer, McpTransport};

/// How long to wait for the user to click the consent link. The CLI
/// blocks for this duration before giving up.
pub const CONSENT_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct RegisterArgs {
    pub name: String,
    pub version: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub endpoint: Option<String>,
}

/// Subset of register args used by the OAuth 2.1 PKCE flow.
#[derive(Debug, Clone)]
pub struct OAuthRegisterArgs {
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
}

/// Register a new MCP server and return the persisted record.
///
/// # Errors
/// Returns an error if the `--name` or `--version` flags are empty, if the
/// transport string is not recognised, if transport-required fields are
/// missing, or if the repository `create` call fails.
pub async fn register(repo: &dyn McpServerRepository, args: RegisterArgs) -> Result<McpServer> {
    if args.name.trim().is_empty() {
        return Err(anyhow!("--name must not be empty"));
    }
    if args.version.trim().is_empty() {
        return Err(anyhow!("--version must not be empty"));
    }
    let transport = McpTransport::parse(&args.transport).ok_or_else(|| {
        anyhow!(
            "unknown transport '{}': expected 'stdio', 'sse', or 'http'",
            args.transport
        )
    })?;
    match transport {
        McpTransport::Stdio => {
            if args.command.as_deref().is_none_or(str::is_empty) {
                return Err(anyhow!("--command is required for transport=stdio"));
            }
        }
        McpTransport::Sse | McpTransport::Http => {
            if args.endpoint.as_deref().is_none_or(str::is_empty) {
                return Err(anyhow!(
                    "--endpoint is required for transport={}",
                    transport.as_str()
                ));
            }
        }
    }

    let now = Utc::now();
    let server = McpServer {
        id: McpServerInstanceId::new(),
        name: args.name,
        version: args.version,
        transport,
        command: args.command,
        args: args.args,
        env_keys: args.env_keys,
        endpoint: args.endpoint,
        enabled: true,
        created_at: now,
        updated_at: now,
    };
    repo.create(&server).await?;
    Ok(server)
}

#[derive(Debug, Clone, Default)]
pub struct ListArgs {}

/// List MCP servers from the repository.
///
/// # Errors
/// Returns an error if the repository query fails.
pub async fn list(repo: &dyn McpServerRepository, _args: ListArgs) -> Result<Vec<McpServer>> {
    Ok(repo.list().await?)
}

#[derive(Debug, Clone)]
pub struct RemoveArgs {
    pub id: String,
}

/// Remove an MCP server by ID.
///
/// # Errors
/// Returns an error if `--id` is empty or if the repository `delete` call
/// fails.
pub async fn remove(repo: &dyn McpServerRepository, args: RemoveArgs) -> Result<()> {
    if args.id.trim().is_empty() {
        return Err(anyhow!("--id must not be empty"));
    }
    repo.delete(&args.id).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// OAuth 2.1 PKCE consent flow (Tier-3 T4)
// ---------------------------------------------------------------------------

/// Outcome of the local consent listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackResult {
    pub code: String,
    pub state: String,
}

/// Parse `?code=...&state=...` out of an HTTP request line of the form
/// `GET /callback?code=ABC&state=XYZ HTTP/1.1`.
///
/// # Errors
/// Returns an error if the request line is malformed, the path is not
/// the expected `/callback`, or either query field is missing.
pub fn parse_callback_request_line(line: &str) -> Result<CallbackResult> {
    let mut parts = line.split_whitespace();
    let _method = parts.next().ok_or_else(|| anyhow!("empty request line"))?;
    let target = parts
        .next()
        .ok_or_else(|| anyhow!("missing request target"))?;
    // Tolerate either an absolute URL or a path-relative target.
    let parsed = if target.starts_with('/') {
        url::Url::parse(&format!("http://127.0.0.1{target}"))
    } else {
        url::Url::parse(target)
    }
    .map_err(|e| anyhow!("malformed target {target:?}: {e}"))?;
    if !parsed.path().ends_with("/callback") && parsed.path() != "/callback" {
        return Err(anyhow!(
            "unexpected callback path {:?}; want /callback",
            parsed.path()
        ));
    }
    let mut code = None;
    let mut state = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            _ => {}
        }
    }
    Ok(CallbackResult {
        code: code.ok_or_else(|| anyhow!("callback missing `code` param"))?,
        state: state.ok_or_else(|| anyhow!("callback missing `state` param"))?,
    })
}

/// Bind a local listener on `127.0.0.1:0`, return the URL the caller
/// should publish as the redirect URI and the listener itself.
///
/// # Errors
/// Returns an error if `bind` fails.
pub async fn bind_callback_listener() -> Result<(TcpListener, String)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind callback listener")?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}/callback");
    Ok((listener, url))
}

/// Accept a single HTTP request on `listener`, parse the OAuth
/// callback query, write a "you can close this window" response, and
/// return the parsed `(code, state)`.
///
/// Times out after `CONSENT_TIMEOUT_SECS` seconds.
///
/// # Errors
/// Returns an error on timeout, malformed request, or I/O failure.
pub async fn accept_one_callback(listener: TcpListener) -> Result<CallbackResult> {
    let accept_fut = async move {
        let (mut socket, _peer) = listener
            .accept()
            .await
            .context("accept callback connection")?;
        let mut buf = [0u8; 8192];
        let n = socket
            .read(&mut buf)
            .await
            .context("read callback request")?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let first = req.lines().next().unwrap_or("");
        let result = parse_callback_request_line(first)?;
        let body = b"<html><body><h2>xiaoguai: OAuth consent recorded.</h2>\
                     <p>You can close this window and return to the terminal.</p></body></html>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n",
            body.len()
        );
        socket
            .write_all(resp.as_bytes())
            .await
            .context("write callback response headers")?;
        socket
            .write_all(body)
            .await
            .context("write callback response body")?;
        let _ = socket.shutdown().await;
        Result::<_, anyhow::Error>::Ok(result)
    };
    tokio::time::timeout(Duration::from_secs(CONSENT_TIMEOUT_SECS), accept_fut)
        .await
        .map_err(|_| {
            anyhow!("timed out waiting for OAuth callback after {CONSENT_TIMEOUT_SECS}s")
        })?
}

/// Run the full OAuth 2.1 PKCE register flow against a pre-bound
/// listener.
///
/// The listener is parameterised so tests can fire the callback
/// synthetically without spawning a browser.
///
/// # Errors
/// Propagates errors from any step: state validation, token exchange,
/// repository write, or store write.
pub async fn register_oauth_with_listener(
    repo: &dyn McpServerRepository,
    store: Arc<dyn TokenStore>,
    listener: TcpListener,
    redirect_uri: String,
    base: RegisterArgs,
    oauth: OAuthRegisterArgs,
) -> Result<(McpServer, TokenBundle, OAuth2PkceConfig)> {
    if base.endpoint.as_deref().is_none_or(str::is_empty) {
        return Err(anyhow!(
            "--endpoint is required for OAuth-authed MCP servers"
        ));
    }
    if oauth.auth_url.trim().is_empty()
        || oauth.token_url.trim().is_empty()
        || oauth.client_id.trim().is_empty()
    {
        return Err(anyhow!(
            "--auth-url, --token-url, --client-id are required for --auth=oauth2-pkce"
        ));
    }
    let oauth_cfg = OAuth2PkceConfig {
        auth_url: oauth.auth_url,
        token_url: oauth.token_url,
        client_id: oauth.client_id,
        scopes: oauth.scopes,
        redirect_uri,
    };
    let pkce = new_pkce_pair();
    let state = new_state();
    let auth_url = build_authorize_url(&oauth_cfg, &pkce.challenge, &state);
    eprintln!("Open this URL in a browser to consent:");
    eprintln!();
    eprintln!("  {auth_url}");
    eprintln!();
    eprintln!("Waiting for callback (timeout: {CONSENT_TIMEOUT_SECS}s)...");
    let cb = accept_one_callback(listener).await?;
    if cb.state != state {
        return Err(anyhow!(
            "state mismatch in OAuth callback: expected {state}, got {}",
            cb.state
        ));
    }
    let http = build_http_client().map_err(|e| anyhow!("build oauth http client: {e}"))?;
    let bundle = exchange_code(&http, &oauth_cfg, &cb.code, &pkce.verifier)
        .await
        .map_err(|e| anyhow!("exchange code: {e}"))?;

    // Persist the MCP server row first, then the token bundle, so a
    // crash between the two steps doesn't leave an orphan token.
    let transport = McpTransport::parse(&base.transport).ok_or_else(|| {
        anyhow!(
            "unknown transport '{}': expected 'stdio', 'sse', or 'http'",
            base.transport
        )
    })?;
    let now = Utc::now();
    let server = McpServer {
        id: McpServerInstanceId::new(),
        name: base.name,
        version: base.version,
        transport,
        command: base.command,
        args: base.args,
        env_keys: base.env_keys,
        endpoint: base.endpoint,
        enabled: true,
        created_at: now,
        updated_at: now,
    };
    repo.create(&server)
        .await
        .map_err(|e| anyhow!("create mcp_server row: {e}"))?;
    // OAuth token bundles are keyed by server id (single implicit owner).
    store
        .put(server.id.as_str(), &bundle)
        .await
        .map_err(|e| anyhow!("persist token bundle: {e}"))?;
    Ok((server, bundle, oauth_cfg))
}

#[must_use]
pub fn format_table(rows: &[McpServer]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str(
        "ID                                     TRANSPORT  NAME             VERSION  ENABLED\n",
    );
    for s in rows {
        let _ = writeln!(
            out,
            "{:38} {:10} {:16} {:8} {}",
            s.id.as_str(),
            s.transport.as_str(),
            s.name,
            s.version,
            s.enabled
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_callback_query() {
        let line = "GET /callback?code=ABC123&state=XYZ789 HTTP/1.1";
        let parsed = parse_callback_request_line(line).unwrap();
        assert_eq!(parsed.code, "ABC123");
        assert_eq!(parsed.state, "XYZ789");
    }

    #[test]
    fn parses_callback_query_with_percent_encoding() {
        let line = "GET /callback?code=A%2BB%2FC&state=hello%20world HTTP/1.1";
        let parsed = parse_callback_request_line(line).unwrap();
        assert_eq!(parsed.code, "A+B/C");
        assert_eq!(parsed.state, "hello world");
    }

    #[test]
    fn rejects_non_callback_path() {
        let line = "GET /not-callback?code=x&state=y HTTP/1.1";
        let err = parse_callback_request_line(line).unwrap_err().to_string();
        assert!(err.contains("/callback"), "got {err}");
    }

    #[test]
    fn rejects_missing_code() {
        let line = "GET /callback?state=only HTTP/1.1";
        let err = parse_callback_request_line(line).unwrap_err().to_string();
        assert!(err.contains("code"), "got {err}");
    }

    #[test]
    fn rejects_missing_state() {
        let line = "GET /callback?code=only HTTP/1.1";
        let err = parse_callback_request_line(line).unwrap_err().to_string();
        assert!(err.contains("state"), "got {err}");
    }
}
