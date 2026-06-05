//! `xiaoguai-coding` — the governed coding workflow (DEC-034 / DEC-035,
//! `LLD-CODING-001`).
//!
//! A persistent git [`Workspace`] (a real working tree on disk) is the
//! execution context for the coding tools, with a [`CheckpointId`]-based
//! checkpoint/rollback primitive so every mutating step is reversible. This is
//! deliberately a *new persistent mode* alongside — not a replacement for — the
//! ephemeral `xiaoguai-mcp-exec` sandbox (DEC-005), and it reuses the existing
//! `HotL` gate (DEC-006) + HMAC audit chain (DEC-004) for governance rather than
//! introducing a parallel approval/audit path.
//!
//! P0-c (current): workspace + checkpoint/rollback. Subsequent commits add the
//! `edit_file` / `git_*` tools and their gated + audited dispatch.

mod checkpoint;
mod error;
mod git;
mod governed;
mod tools;
mod workspace;

pub use checkpoint::CheckpointId;
pub use error::CodingError;
pub use governed::{
    CodingGate, CodingStep, GateDecision, GovernedOutcome, GovernedTools, StepRecorder,
};
pub use tools::{EditSummary, FileEdit};
pub use workspace::{Workspace, WorkspaceId};

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::fs;

    async fn read(ws: &Workspace, rel: &str) -> Option<String> {
        fs::read_to_string(ws.root().join(rel)).await.ok()
    }

    async fn write(ws: &Workspace, rel: &str, body: &str) {
        let path = ws.root().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.unwrap();
        }
        fs::write(path, body).await.unwrap();
    }

    #[tokio::test]
    async fn open_or_create_initialises_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        assert!(git::is_repo(ws.root()).await);
        assert!(!ws.id().as_str().is_empty());
    }

    #[tokio::test]
    async fn rollback_reverts_a_modified_file() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        write(&ws, "a.txt", "A").await;

        let cp = ws.checkpoint("after A").await.unwrap();
        write(&ws, "a.txt", "B").await;
        assert_eq!(read(&ws, "a.txt").await.as_deref(), Some("B"));

        ws.rollback(&cp).await.unwrap();
        assert_eq!(read(&ws, "a.txt").await.as_deref(), Some("A"));
    }

    #[tokio::test]
    async fn rollback_removes_a_file_added_since_the_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        write(&ws, "keep.txt", "keep").await;

        let cp = ws.checkpoint("only keep").await.unwrap();
        write(&ws, "added.txt", "added").await;
        write(&ws, "nested/deep.txt", "deep").await;

        ws.rollback(&cp).await.unwrap();
        assert_eq!(read(&ws, "keep.txt").await.as_deref(), Some("keep"));
        assert_eq!(read(&ws, "added.txt").await, None);
        assert_eq!(read(&ws, "nested/deep.txt").await, None);
    }

    #[tokio::test]
    async fn rollback_recreates_a_file_deleted_since_the_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        write(&ws, "doomed.txt", "alive").await;

        let cp = ws.checkpoint("with doomed").await.unwrap();
        fs::remove_file(ws.root().join("doomed.txt")).await.unwrap();
        assert_eq!(read(&ws, "doomed.txt").await, None);

        ws.rollback(&cp).await.unwrap();
        assert_eq!(read(&ws, "doomed.txt").await.as_deref(), Some("alive"));
    }

    #[tokio::test]
    async fn workspace_id_is_stable_across_reopen_so_rollback_survives() {
        // Regression: a fresh WorkspaceId per open orphaned the checkpoint ref,
        // so rollback across separate opens (e.g. separate CLI invocations)
        // failed with UnknownCheckpoint. The id must persist per on-disk tree.
        let dir = tempfile::tempdir().unwrap();

        let ws1 = Workspace::open_or_create(dir.path()).await.unwrap();
        write(&ws1, "a.txt", "original").await;
        let cp = ws1.checkpoint("v1").await.unwrap();
        let id1 = ws1.id().as_str().to_string();

        // Re-open the SAME path in a separate Workspace value.
        let ws2 = Workspace::open_or_create(dir.path()).await.unwrap();
        assert_eq!(ws2.id().as_str(), id1, "id must persist per tree");

        write(&ws2, "a.txt", "changed").await;
        ws2.rollback(&cp).await.unwrap(); // must find cp made by ws1
        assert_eq!(read(&ws2, "a.txt").await.as_deref(), Some("original"));
    }

    #[tokio::test]
    async fn rollback_rejects_an_unknown_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        write(&ws, "a.txt", "A").await;
        ws.checkpoint("real").await.unwrap();

        let bogus = CheckpointId::from_sha("0".repeat(40));
        let err = ws.rollback(&bogus).await.unwrap_err();
        assert!(matches!(err, CodingError::UnknownCheckpoint { .. }));
    }
}
