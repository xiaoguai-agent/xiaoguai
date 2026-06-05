//! [`Workspace`] — a persistent git working tree scoped to one coding session
//! (DEC-034). This is the new *persistent* execution mode that the coding tools
//! operate in, as opposed to the ephemeral fresh-tempdir sandbox of
//! `xiaoguai-mcp-exec` (DEC-005), which is left unchanged.
//!
//! P0-c (this commit) ships the workspace + the checkpoint/rollback primitive
//! (DEC-035). The file/git tools and their `HotL`-gated + audited dispatch
//! (DEC-006 / DEC-004) are wired in subsequent commits per `LLD-CODING-001`.

use std::fmt;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::CodingError;
use crate::git;

/// Stable identifier for a workspace. Time-ordered (UUID v7) so ids sort by
/// creation; appears in audit rows and checkpoint context.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }

    pub(crate) fn from_existing(s: String) -> Self {
        Self(s)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for WorkspaceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A persistent git working tree for one coding session.
#[derive(Debug, Clone)]
pub struct Workspace {
    id: WorkspaceId,
    root: PathBuf,
}

impl Workspace {
    /// Open `root` as a workspace, initialising a fresh git repo there if it is
    /// not already a work tree. The directory is created if missing.
    ///
    /// A fresh [`WorkspaceId`] is minted per call; persisting/reusing ids
    /// across process restarts is a later concern (the id is wired into audit
    /// rows, not yet into a store).
    pub async fn open_or_create(root: &Path) -> Result<Self, CodingError> {
        tokio::fs::create_dir_all(root)
            .await
            .map_err(|e| CodingError::io(root, e))?;

        // Canonicalise so checkpoint paths and worktree pruning compare equal
        // regardless of how the caller spelled the path (symlinks, `.`, …).
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|e| CodingError::io(root, e))?;

        if !git::is_repo(&root).await {
            git::run(&root, &["init", "-q"], None).await?;
            // A snapshot via `commit-tree` needs an author/committer identity;
            // set a workspace-local one so we never depend on global git config
            // (air-gapped boxes often have none).
            git::run(&root, &["config", "user.name", "xiaoguai"], None).await?;
            git::run(&root, &["config", "user.email", "xiaoguai@localhost"], None).await?;
        }

        // The WorkspaceId must be STABLE per on-disk tree, not per process —
        // the checkpoint ref is `refs/xiaoguai/checkpoints/<id>`, so a fresh id
        // each `open` would orphan earlier checkpoints (rollback across CLI
        // invocations would fail). Persist it next to the repo.
        let id = Self::load_or_init_id(&root).await?;

        Ok(Self { id, root })
    }

    /// Read the persisted workspace id from `.git/xiaoguai-workspace-id`, or
    /// mint + persist a new one. Keeps the checkpoint ref stable across opens.
    async fn load_or_init_id(root: &Path) -> Result<WorkspaceId, CodingError> {
        let id_path = root.join(".git").join("xiaoguai-workspace-id");
        match tokio::fs::read_to_string(&id_path).await {
            Ok(s) if !s.trim().is_empty() => Ok(WorkspaceId::from_existing(s.trim().to_string())),
            _ => {
                let id = WorkspaceId::new();
                tokio::fs::write(&id_path, id.as_str())
                    .await
                    .map_err(|e| CodingError::io(&id_path, e))?;
                Ok(id)
            }
        }
    }

    #[must_use]
    pub fn id(&self) -> &WorkspaceId {
        &self.id
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}
