//! Bind the API router on a TCP address.
//!
//! Pulled out from the router builder so binaries (`xiaoguai-core serve`)
//! and integration tests share the same launch path.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use tokio::net::TcpListener;

use crate::routes::router;
use crate::state::AppState;

/// Bind `addr`, serve until the future is dropped or the OS reports an
/// error. Returns the actual bound address (useful when `addr.port() == 0`
/// for ephemeral test ports) and a future that drives the server.
pub async fn serve_with_state(
    addr: SocketAddr,
    state: AppState,
) -> Result<(
    SocketAddr,
    impl std::future::Future<Output = std::io::Result<()>>,
)> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    let local = listener.local_addr().context("read local addr")?;
    let app = router(state);
    let fut = async move { axum::serve(listener, app.into_make_service()).await };
    Ok((local, fut))
}
