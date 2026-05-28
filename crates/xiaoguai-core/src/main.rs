//! `xiaoguai-core` binary — thin shim that delegates to
//! [`xiaoguai_core::run_with_cli`]. The actual boot/wiring lives in the
//! library so the unified `xiaoguai` CLI can call into it.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    xiaoguai_core::run_with_cli().await
}
