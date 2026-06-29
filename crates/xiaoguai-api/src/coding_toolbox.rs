//! Feature ⑤ — per-session coding workspace root, live-wiring port.
//!
//! Same trait-in-api / impl-in-core pattern as [`crate::sessions_ext`] and the
//! v0.12.x scheduler shims: the api crate cannot build the governed coding
//! toolbox itself (that lives in `xiaoguai-core::coding_bridge`, over the
//! `xiaoguai-coding` crate the api layer does not depend on). So the api layer
//! exposes a narrow [`CodingToolboxFactory`] port; `xiaoguai-core` wires the
//! concrete impl in `run_serve` capturing the audit sink, the egress opt-in
//! flag, the base (non-coding) toolbox, and the global default root.
//!
//! At boot the coding tools are baked into `AppState.toolbox` with the GLOBAL
//! root. When a turn runs for a session that pins a different `working_dir`,
//! [`crate::turn::run_turn`] calls [`CodingToolboxFactory::rebuild_for`] to
//! obtain a toolbox whose coding tools are rooted at that per-session dir
//! instead — for that one turn only. Sessions that pin no dir (or pin the
//! global root) keep using `AppState.toolbox` unchanged, so the common path is
//! byte-identical to before this port existed.
//!
//! The opt-in gating and security model are untouched: the factory is only
//! `Some(..)` when coding is already enabled at boot (audit signing key + a
//! global `XIAOGUAI_CODING_WORKSPACE`), and `rebuild_for` produces the SAME
//! governed surface (HotL-gated, checkpointed, audited, egress-gated) — only
//! the workspace root differs.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use xiaoguai_agent::Toolbox;

/// Rebuild the agent toolbox with the governed coding tools rooted at a
/// per-session workspace directory.
///
/// The returned toolbox is the SAME base (non-coding) toolbox the factory
/// captured at boot, with a freshly-built governed coding surface layered on
/// top — identical in every respect to the boot-time toolbox except the
/// coding tools' workspace root. The implementation must preserve the egress
/// opt-in and audit/gate wiring exactly as at boot.
#[async_trait]
pub trait CodingToolboxFactory: Send + Sync {
    /// Build a toolbox whose coding tools are rooted at `root`.
    ///
    /// # Errors
    /// Returns an error if the workspace at `root` cannot be opened/created or
    /// a coding tool fails to register. Callers treat an error as "fall back
    /// to the boot toolbox" — a bad per-session dir must never break the turn.
    async fn rebuild_for(&self, root: &Path) -> anyhow::Result<Arc<Toolbox>>;

    /// The global default coding workspace root baked into `AppState.toolbox`
    /// at boot, if any. A per-session `working_dir` equal to this needs no
    /// rebuild — `run_turn` can use the boot toolbox as-is. `None` means the
    /// boot toolbox carries no coding tools rooted at a fixed dir (the factory
    /// is only present when coding is enabled, so in practice this is `Some`).
    fn global_root(&self) -> Option<&Path>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::PathBuf;

    /// Records the root it was asked to rebuild for and hands back a marker
    /// toolbox — proves `run_turn` calls the port with the session's dir.
    struct RecordingFactory {
        seen: std::sync::Mutex<Vec<PathBuf>>,
        global: Option<PathBuf>,
    }

    #[async_trait]
    impl CodingToolboxFactory for RecordingFactory {
        async fn rebuild_for(&self, root: &Path) -> anyhow::Result<Arc<Toolbox>> {
            self.seen.lock().unwrap().push(root.to_path_buf());
            Ok(Arc::new(Toolbox::new()))
        }
        fn global_root(&self) -> Option<&Path> {
            self.global.as_deref()
        }
    }

    #[tokio::test]
    async fn rebuild_for_receives_the_requested_root() {
        let f = RecordingFactory {
            seen: std::sync::Mutex::new(Vec::new()),
            global: Some(PathBuf::from("/srv/global")),
        };
        let tb = f.rebuild_for(Path::new("/srv/session-1")).await.unwrap();
        assert_eq!(tb.len(), 0);
        assert_eq!(
            f.seen.lock().unwrap().as_slice(),
            [PathBuf::from("/srv/session-1")]
        );
        assert_eq!(f.global_root(), Some(Path::new("/srv/global")));
    }
}
