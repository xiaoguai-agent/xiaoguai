//! Checkpoint + rollback primitive (DEC-035). A checkpoint is a content-
//! addressed snapshot of the working tree, stored as a commit on a hidden ref
//! (`refs/xiaoguai/checkpoints/<ws>`) so unchanged blobs dedupe via git's
//! object store and the snapshot never touches the user's index, HEAD, or
//! branches. Rollback restores the working tree to a checkpoint, handling
//! modified, added, and deleted files.
//!
//! This is the safety net that makes autonomous edits reversible — "audit =
//! see what it did; rollback = undo it". The [`CheckpointId`] returned here is
//! embedded in the audit row of the action it precedes (wired in a later
//! commit per `LLD-CODING-001`).
//!
//! Known step-1 limitations (hardened in P0-b): `.gitignore`d paths are neither
//! snapshotted nor pruned (intentional — don't snapshot build artifacts); empty
//! directories left after a prune are not removed; checkpoints are not yet
//! pruned by count/age.

use std::collections::HashSet;
use std::path::PathBuf;

use uuid::Uuid;

use crate::error::CodingError;
use crate::git;
use crate::workspace::Workspace;

/// Identifier of a checkpoint — the git commit SHA of the snapshot. Pass it
/// back to [`Workspace::rollback`] to restore the tree to that point.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CheckpointId(String);

impl CheckpointId {
    /// Reconstruct a checkpoint id from a stored commit SHA (e.g. read back
    /// from an audit row). No validation — [`Workspace::rollback`] verifies the
    /// SHA belongs to this workspace's checkpoint chain.
    #[must_use]
    pub fn from_sha(sha: impl Into<String>) -> Self {
        Self(sha.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Workspace {
    /// The hidden ref under which this workspace's checkpoint chain lives.
    fn checkpoint_ref(&self) -> String {
        format!("refs/xiaoguai/checkpoints/{}", self.id())
    }

    /// Snapshot the current working tree and return its [`CheckpointId`].
    ///
    /// Implemented with a throwaway temp index so the user's real index/HEAD
    /// are untouched: stage the whole tree into the temp index, `write-tree`,
    /// `commit-tree` (parented on the previous checkpoint so the chain is
    /// walkable), and advance the hidden ref.
    pub async fn checkpoint(&self, label: &str) -> Result<CheckpointId, CodingError> {
        let root = self.root();
        let tmp_index = root
            .join(".git")
            .join(format!("xiaoguai-cp-{}.index", Uuid::now_v7()));

        let result = async {
            // Fresh temp index → every worktree file is staged (honours .gitignore).
            git::run(root, &["add", "-A"], Some(&tmp_index)).await?;
            let tree = git::run(root, &["write-tree"], Some(&tmp_index)).await?;

            let cp_ref = self.checkpoint_ref();
            let parent = git::run(root, &["rev-parse", "--verify", "-q", &cp_ref], None)
                .await
                .ok()
                .filter(|s| !s.is_empty());

            let message = format!("checkpoint: {label}");
            let mut args: Vec<&str> = vec!["commit-tree", &tree, "-m", &message];
            if let Some(parent) = parent.as_deref() {
                args.push("-p");
                args.push(parent);
            }
            let commit = git::run(root, &args, None).await?;

            git::run(root, &["update-ref", &cp_ref, &commit], None).await?;
            Ok(CheckpointId(commit))
        }
        .await;

        // Best-effort cleanup; a leftover temp index is harmless but noisy.
        let _ = tokio::fs::remove_file(&tmp_index).await;
        result
    }

    /// Restore the working tree to `to`. Modified files are reverted, files
    /// added since the checkpoint are removed, and files deleted since the
    /// checkpoint are recreated.
    pub async fn rollback(&self, to: &CheckpointId) -> Result<(), CodingError> {
        let root = self.root();

        // Validate `to` belongs to this workspace's checkpoint chain (its tip
        // included), so a stray SHA can't be used to overwrite the tree.
        let cp_ref = self.checkpoint_ref();
        let known = git::run(
            root,
            &["merge-base", "--is-ancestor", to.as_str(), &cp_ref],
            None,
        )
        .await
        .is_ok();
        if !known {
            return Err(CodingError::UnknownCheckpoint {
                id: to.as_str().to_string(),
            });
        }

        let tmp_index = root
            .join(".git")
            .join(format!("xiaoguai-rb-{}.index", Uuid::now_v7()));

        let result = async {
            // Point the temp index at the snapshot tree, then materialise every
            // snapshot file into the worktree (overwrites modified/deleted).
            git::run(root, &["read-tree", to.as_str()], Some(&tmp_index)).await?;
            let snapshot: HashSet<PathBuf> =
                git::run_z(root, &["ls-files", "-z"], Some(&tmp_index))
                    .await?
                    .into_iter()
                    .collect();
            git::run(root, &["checkout-index", "-a", "-f"], Some(&tmp_index)).await?;

            // Prune files present now but absent from the snapshot (i.e. added
            // since the checkpoint). Tracked + untracked-non-ignored = the
            // worktree's visible set.
            let mut current = git::run_z(root, &["ls-files", "-z"], None).await?;
            current.extend(
                git::run_z(root, &["ls-files", "-z", "-o", "--exclude-standard"], None).await?,
            );

            for path in current {
                if !snapshot.contains(&path) {
                    let abs = root.join(&path);
                    tokio::fs::remove_file(&abs)
                        .await
                        .map_err(|e| CodingError::io(&abs, e))?;
                }
            }
            Ok(())
        }
        .await;

        let _ = tokio::fs::remove_file(&tmp_index).await;
        result
    }
}
