//! #15 — governed coding tools activate **per-session** when an audit signing
//! key is set but no global `XIAOGUAI_CODING_WORKSPACE` is configured.
//!
//! Approach A (owner-chosen): "set a `working_dir` ⇒ coding turns on for that
//! session". The boot toolbox carries no coding (so the agent can never touch
//! the server's own CWD — security-review H1), but the Feature ⑤ factory is
//! wired with a `None` global root so any session that pins a `working_dir`
//! gets the SAME governed coding surface, rooted there, built on demand.

use std::sync::Arc;

use sqlx::SqlitePool;
use xiaoguai_agent::Toolbox;
use xiaoguai_api::coding_toolbox::CodingToolboxFactory;
use xiaoguai_audit::chain::sink::SqliteAuditSink;
use xiaoguai_core::coding_bridge::CodingToolboxFactoryImpl;

async fn sqlite_pool() -> (tempfile::TempDir, SqlitePool) {
    let dir = tempfile::tempdir().unwrap();
    let pool = xiaoguai_storage::db::connect(dir.path().join("t.db").to_str().unwrap(), 5)
        .await
        .unwrap();
    xiaoguai_storage::db::migrate(&pool).await.unwrap();
    (dir, pool)
}

/// With a `None` global root, `global_root()` reports `None` and `rebuild_for`
/// still builds the governed coding surface at the requested session dir.
#[tokio::test]
async fn factory_without_global_root_activates_coding_at_a_session_dir() {
    let (_db, pool) = sqlite_pool().await;
    let sink = Arc::new(SqliteAuditSink::new(
        pool,
        b"15-per-session-coding-integration-test-key".to_vec(),
    ));

    // audit key present, NO global workspace → factory has a None global root.
    let factory = CodingToolboxFactoryImpl::new(sink, false, false, Toolbox::new(), None);
    assert_eq!(
        factory.global_root(),
        None,
        "no global XIAOGUAI_CODING_WORKSPACE was configured"
    );

    // A session that pins a working_dir gets governed coding rooted there.
    let ws = tempfile::tempdir().unwrap();
    let tb = factory
        .rebuild_for(ws.path())
        .await
        .expect("coding toolbox builds at the session working_dir");
    assert!(
        !tb.is_empty(),
        "a session working_dir must activate the governed coding tools"
    );
}

/// A `Some(global)` root still works (the pre-#15 boot-workspace behaviour) and
/// is reported by `global_root()` so `run_turn` can skip a needless rebuild
/// when a session pins the same dir.
#[tokio::test]
async fn factory_with_global_root_reports_it() {
    let (_db, pool) = sqlite_pool().await;
    let sink = Arc::new(SqliteAuditSink::new(
        pool,
        b"15-per-session-coding-integration-test-key".to_vec(),
    ));
    let global = tempfile::tempdir().unwrap();
    let factory = CodingToolboxFactoryImpl::new(
        sink,
        false,
        false,
        Toolbox::new(),
        Some(global.path().to_path_buf()),
    );
    assert_eq!(factory.global_root(), Some(global.path()));
}
