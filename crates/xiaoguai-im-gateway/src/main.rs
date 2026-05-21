//! Xiaoguai IM gateway binary entry point (placeholder).

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!(
        "xiaoguai-im-gateway v{} — placeholder, not yet wired",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
