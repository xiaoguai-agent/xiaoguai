//! Coding operations on a [`Workspace`] (DEC-034, `LLD-CODING-001` §2).
//!
//! These are the *pure operations* — read files, search, edit, and the `git_*`
//! verbs. They do NOT gate or audit themselves: per DEC-006 the agent loop
//! gates every tool call by `tool_call.<name>` before dispatch, and the
//! checkpoint + audit sequencing is applied by [`crate::governed`]. Keeping the
//! operations free of cross-cutting concerns makes them directly testable and
//! reusable from either the governed facade or an integration test.

use std::path::{Path, PathBuf};

use crate::error::CodingError;
use crate::git;
use crate::workspace::Workspace;

/// A single edit to a file. `edit_file` is intentionally small for P0-c —
/// whole-file write and literal search/replace cover the agent's common cases;
/// unified-diff application (`git apply`) is added under the same enum later.
#[derive(Debug, Clone)]
pub enum FileEdit {
    /// Replace the file's entire contents (creating it + parent dirs if absent).
    Write(String),
    /// Replace occurrences of `find` with `replace`. `all = false` replaces only
    /// the first occurrence; a missing `find` is an error (no silent no-op).
    Replace {
        find: String,
        replace: String,
        all: bool,
    },
}

/// Outcome of an [`Workspace::edit_file`] — enough for the agent + audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditSummary {
    pub path: PathBuf,
    pub replacements: usize,
    pub bytes_after: usize,
}

impl Workspace {
    fn abs(&self, rel: &Path) -> PathBuf {
        self.root().join(rel)
    }

    /// Read a UTF-8 file relative to the workspace root. `[READ]`
    pub async fn read_file(&self, rel: &Path) -> Result<String, CodingError> {
        let abs = self.abs(rel);
        tokio::fs::read_to_string(&abs)
            .await
            .map_err(|e| CodingError::io(&abs, e))
    }

    /// List entries (one name per line, dirs suffixed `/`) of a directory
    /// relative to the workspace root, sorted. `[READ]`
    pub async fn list_dir(&self, rel: &Path) -> Result<Vec<String>, CodingError> {
        let abs = self.abs(rel);
        let mut rd = tokio::fs::read_dir(&abs)
            .await
            .map_err(|e| CodingError::io(&abs, e))?;
        let mut out = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| CodingError::io(&abs, e))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            out.push(if is_dir { format!("{name}/") } else { name });
        }
        out.sort();
        Ok(out)
    }

    /// Search the worktree for `pattern`, returning `path:line:text` matches.
    /// Uses `git grep --no-index` so untracked files are searched too (a fresh
    /// checkpoint workspace keeps an empty HEAD). `[READ]`
    pub async fn grep(&self, pattern: &str) -> Result<Vec<String>, CodingError> {
        // `git grep` exits 1 when there are no matches — treat that as empty,
        // not an error.
        match git::run(
            self.root(),
            &["grep", "--no-index", "-nI", "-e", pattern],
            None,
        )
        .await
        {
            Ok(out) => Ok(out.lines().map(str::to_string).collect()),
            Err(CodingError::Git { code: 1, .. }) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// Apply a [`FileEdit`] to `rel`. Writes atomically (temp + rename).
    /// `[WRITE]` — the caller gates `tool_call.edit_file` and checkpoints first.
    pub async fn edit_file(&self, rel: &Path, edit: &FileEdit) -> Result<EditSummary, CodingError> {
        let abs = self.abs(rel);
        let (contents, replacements) = match edit {
            FileEdit::Write(body) => (body.clone(), 0),
            FileEdit::Replace { find, replace, all } => {
                let current = tokio::fs::read_to_string(&abs)
                    .await
                    .map_err(|e| CodingError::io(&abs, e))?;
                let count = current.matches(find.as_str()).count();
                if count == 0 {
                    return Err(CodingError::Edit {
                        path: abs,
                        reason: format!("search text not found: {find:?}"),
                    });
                }
                let new = if *all {
                    current.replace(find.as_str(), replace)
                } else {
                    current.replacen(find.as_str(), replace, 1)
                };
                (new, if *all { count } else { 1 })
            }
        };

        write_atomic(&abs, contents.as_bytes()).await?;
        Ok(EditSummary {
            path: rel.to_path_buf(),
            replacements,
            bytes_after: contents.len(),
        })
    }

    /// Porcelain status of the worktree (`git status --porcelain`). `[READ]`
    pub async fn git_status(&self) -> Result<String, CodingError> {
        // `--no-index` is not applicable; status needs the repo. Empty HEAD is
        // fine — untracked files show as `??`.
        git::run(self.root(), &["status", "--porcelain"], None).await
    }

    /// Stage everything and commit. Returns the new commit SHA. `[WRITE]`
    pub async fn git_commit(&self, message: &str) -> Result<String, CodingError> {
        git::run(self.root(), &["add", "-A"], None).await?;
        git::run(self.root(), &["commit", "-q", "-m", message], None).await?;
        git::run(self.root(), &["rev-parse", "HEAD"], None).await
    }

    /// Create and switch to a new branch. `[WRITE]`
    pub async fn git_branch(&self, name: &str) -> Result<(), CodingError> {
        git::run(self.root(), &["checkout", "-q", "-b", name], None).await?;
        Ok(())
    }
}

/// Write `bytes` to `path` atomically: write a sibling temp file, fsync, rename.
async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CodingError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| CodingError::io(parent, e))?;
    }
    let tmp = path.with_extension("xiaoguai-tmp");
    tokio::fs::write(&tmp, bytes)
        .await
        .map_err(|e| CodingError::io(&tmp, e))?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| CodingError::io(path, e))?;
    Ok(())
}
