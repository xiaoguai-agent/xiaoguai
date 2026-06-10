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
#[cfg(feature = "mcp")]
mod mcp_client;
mod tools;
mod workspace;

pub use checkpoint::CheckpointId;
pub use error::CodingError;
pub use governed::{
    CodingGate, CodingStep, GateDecision, GovernedOutcome, GovernedTools, StepRecorder,
};
#[cfg(feature = "mcp")]
pub use mcp_client::{coding_tool_descriptors, CodingMcpClient};
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
    async fn rollback_removes_agent_files_captured_in_a_later_checkpoint() {
        // Files the agent adds are captured by the NEXT governed checkpoint, so
        // they live in the chain between `cp` and the tip → rollback removes them.
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        write(&ws, "keep.txt", "keep").await;
        let cp = ws.checkpoint("only keep").await.unwrap();

        write(&ws, "added.txt", "added").await;
        write(&ws, "nested/deep.txt", "deep").await;
        ws.checkpoint("after agent add").await.unwrap(); // captures the additions

        ws.rollback(&cp).await.unwrap();
        assert_eq!(read(&ws, "keep.txt").await.as_deref(), Some("keep"));
        assert_eq!(read(&ws, "added.txt").await, None);
        assert_eq!(read(&ws, "nested/deep.txt").await, None);
    }

    #[tokio::test]
    async fn rollback_preserves_user_files_not_in_any_checkpoint() {
        // Regression: rollback must NOT delete files the user created
        // out-of-band (never captured by a checkpoint) — that would be data loss
        // on the owner's own repo.
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        write(&ws, "keep.txt", "keep").await;
        let cp = ws.checkpoint("only keep").await.unwrap();

        // User adds a file after the checkpoint; no further checkpoint is taken.
        write(&ws, "user-notes.txt", "my notes").await;

        ws.rollback(&cp).await.unwrap();
        assert_eq!(read(&ws, "keep.txt").await.as_deref(), Some("keep"));
        assert_eq!(
            read(&ws, "user-notes.txt").await.as_deref(),
            Some("my notes"),
            "rollback must not delete user-created files outside the checkpoint chain"
        );
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

    #[cfg(unix)]
    #[tokio::test]
    async fn path_guard_rejects_symlink_escape_read_and_write() {
        // Security regression: the lexical `..`/absolute guard is not enough —
        // a symlink inside the tree (which the agent could even create) must not
        // let read_file/edit_file reach outside the workspace root.
        use std::path::Path;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "TOPSECRET")
            .await
            .unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        std::os::unix::fs::symlink(outside.path(), ws.root().join("escape")).unwrap();

        let r = ws.read_file(Path::new("escape/secret.txt")).await;
        assert!(
            matches!(r, Err(CodingError::UnsafePath { .. })),
            "read via in-tree symlink must be denied, got {r:?}"
        );
        let w = ws
            .edit_file(Path::new("escape/pwn.txt"), &FileEdit::Write("x".into()))
            .await;
        assert!(
            matches!(w, Err(CodingError::UnsafePath { .. })),
            "write via in-tree symlink must be denied, got {w:?}"
        );
        // The outside file is untouched.
        assert_eq!(
            fs::read_to_string(outside.path().join("secret.txt"))
                .await
                .unwrap(),
            "TOPSECRET"
        );
        assert!(!outside.path().join("pwn.txt").exists());
    }

    /// SEC-20: the worktree (including `.git/hooks/`) is model-writable, so a
    /// hook would be arbitrary code execution on the host. Every git call must
    /// run with `-c core.hooksPath=/dev/null` — the commit must succeed (the
    /// hook exits 1, which would abort it) and the hook's side effect must be
    /// absent.
    #[cfg(unix)]
    #[tokio::test]
    async fn git_commit_does_not_execute_workspace_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();

        let hooks_dir = ws.root().join(".git").join("hooks");
        fs::create_dir_all(&hooks_dir).await.unwrap();
        let hook = hooks_dir.join("pre-commit");
        fs::write(&hook, "#!/bin/sh\ntouch hook-ran.txt\nexit 1\n")
            .await
            .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        write(&ws, "a.txt", "A").await;
        let sha = ws.git_commit("hooks must not run").await.unwrap();
        assert_eq!(sha.len(), 40, "commit must succeed despite the exit-1 hook");
        assert!(
            read(&ws, "hook-ran.txt").await.is_none(),
            "pre-commit hook executed — core.hooksPath override is broken"
        );
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
