//! Xiaoguai core binary entry point.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!(
        "xiaoguai-core v{} — placeholder, not yet wired",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
