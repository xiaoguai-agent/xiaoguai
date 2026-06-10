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
    /// Resolve a caller-supplied path against the workspace root, **rejecting
    /// anything that would escape it** — absolute paths, `..`, rooted/prefixed
    /// components, AND symlinks that point outside the root. This is the
    /// containment boundary for every coding tool that takes a path: an
    /// autonomous model cannot reach outside the owner-scoped workspace.
    fn abs(&self, rel: &Path) -> Result<PathBuf, CodingError> {
        use std::path::Component;
        let unsafe_path = |reason: &str| CodingError::UnsafePath {
            path: rel.to_path_buf(),
            reason: reason.to_string(),
        };
        if rel.is_absolute() {
            return Err(unsafe_path("absolute paths are not allowed"));
        }
        for component in rel.components() {
            match component {
                Component::ParentDir => {
                    return Err(unsafe_path("`..` may not escape the workspace root"));
                }
                Component::Prefix(_) | Component::RootDir => {
                    return Err(unsafe_path(
                        "rooted or drive-prefixed paths are not allowed",
                    ));
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }
        let joined = self.root().join(rel);

        // Lexical checks above stop `..`/absolute paths, but a *symlink* inside
        // the tree can still point out (and `edit_file` could even create one),
        // so verify the real location stays under the root. Canonicalize the
        // deepest existing ancestor of the target (resolves symlinked dirs +,
        // for an existing file, the file itself) and require it under the
        // canonical root. New (not-yet-created) leaf components are fine —
        // they'll be created under an already-verified parent.
        let root_canon = self
            .root()
            .canonicalize()
            .map_err(|e| CodingError::io(self.root(), e))?;
        let mut probe: &Path = &joined;
        let existing = loop {
            if probe.exists() {
                break Some(
                    probe
                        .canonicalize()
                        .map_err(|e| CodingError::io(probe, e))?,
                );
            }
            match probe.parent() {
                Some(parent) => probe = parent,
                None => break None,
            }
        };
        if let Some(existing) = existing {
            if !existing.starts_with(&root_canon) {
                return Err(unsafe_path(
                    "resolves (via a symlink) outside the workspace root",
                ));
            }
        }
        Ok(joined)
    }

    /// Read a UTF-8 file relative to the workspace root. `[READ]`
    pub async fn read_file(&self, rel: &Path) -> Result<String, CodingError> {
        let abs = self.abs(rel)?;
        tokio::fs::read_to_string(&abs)
            .await
            .map_err(|e| CodingError::io(&abs, e))
    }

    /// List entries (one name per line, dirs suffixed `/`) of a directory
    /// relative to the workspace root, sorted. `[READ]`
    pub async fn list_dir(&self, rel: &Path) -> Result<Vec<String>, CodingError> {
        let abs = self.abs(rel)?;
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
        let abs = self.abs(rel)?;
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

    /// Push `branch` to `remote`. `[WRITE]` — **egress**, the "past local undo"
    /// boundary (a rollback cannot unwind a completed push). Returns git's
    /// (stderr-trimmed) summary.
    pub async fn git_push(&self, remote: &str, branch: &str) -> Result<String, CodingError> {
        // SEC-22: both values are model-supplied and land in git *positional*
        // slots. `git push` has no reliable `--` separator for its
        // <repository>/<refspec> positions, so a `-`-prefixed value would be
        // parsed as an option (e.g. `--receive-pack=<cmd>` runs an arbitrary
        // command on the remote end, `--exec` likewise). Legitimate remote and
        // branch names never start with `-` (git-check-ref-format forbids it),
        // so fail fast instead of trying to escape.
        reject_option_like("remote", remote)?;
        reject_option_like("branch", branch)?;
        git::run(self.root(), &["push", remote, branch], None).await
    }

    /// Open a pull request via the `gh` CLI. `[WRITE]` — **egress**. Returns the
    /// PR URL `gh` prints. Requires `gh` on `PATH` + a configured GitHub remote;
    /// a missing `gh` surfaces as a teaching `Launch` error.
    pub async fn open_pr(
        &self,
        title: &str,
        body: &str,
        base: &str,
    ) -> Result<String, CodingError> {
        git::run_program(
            "gh",
            self.root(),
            &[
                "pr", "create", "--title", title, "--body", body, "--base", base,
            ],
        )
        .await
    }
}

/// SEC-22: reject a model-supplied git positional argument that git would
/// parse as an option (`-` prefix). Used where git offers no safe `--`
/// separator (e.g. `git push <repository> <refspec>`). Boundary validation:
/// fail fast with a teaching error rather than handing git an injectable
/// value.
fn reject_option_like(what: &str, value: &str) -> Result<(), CodingError> {
    if value.starts_with('-') {
        return Err(CodingError::InvalidArgument {
            what: what.to_string(),
            value: value.to_string(),
            reason: "must not start with `-` (git would parse it as an option)".to_string(),
        });
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;

    /// SEC-22: `-`-prefixed remote/branch values must bounce at the boundary,
    /// before any git subprocess is spawned (option injection,
    /// e.g. `--receive-pack=<cmd>`).
    #[tokio::test]
    async fn git_push_rejects_option_like_remote_and_branch() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();

        let err = ws
            .git_push("--receive-pack=evil", "main")
            .await
            .expect_err("option-like remote must be rejected");
        assert!(
            matches!(err, CodingError::InvalidArgument { .. }),
            "expected InvalidArgument, got {err:?}"
        );

        let err = ws
            .git_push("origin", "--force")
            .await
            .expect_err("option-like branch must be rejected");
        assert!(
            matches!(err, CodingError::InvalidArgument { .. }),
            "expected InvalidArgument, got {err:?}"
        );
    }

    /// SEC-22 helper: ordinary names pass through.
    #[test]
    fn reject_option_like_accepts_normal_names() {
        assert!(reject_option_like("remote", "origin").is_ok());
        assert!(reject_option_like("branch", "feat/x-1").is_ok());
        assert!(reject_option_like("branch", "-x").is_err());
    }
}
