//! Error type for the coding workspace. Messages are written to *teach* —
//! a failed git invocation carries the command, status, and stderr so the
//! agent (or operator) can see exactly what to fix, per the project's
//! teaching-error convention.

use std::path::PathBuf;

/// Failures from workspace + checkpoint operations.
#[derive(Debug, thiserror::Error)]
pub enum CodingError {
    /// A filesystem operation failed (create dir, read/write, remove).
    #[error("workspace I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A required CLI (`git`, or `gh` for `open_pr`) could not be launched at
    /// all (not on `PATH`, or not executable). The persistent-workspace mode
    /// shells out to these (DEC-034 trade-off), so this is the first thing to
    /// check when a workspace operation fails to start.
    #[error("could not run `{program}` (is it installed and on PATH?): {source}")]
    Launch {
        program: String,
        #[source]
        source: std::io::Error,
    },

    /// A CLI ran but exited non-zero. Carries the full context to act on.
    #[error("{program} {args:?} failed (exit {code}) in {cwd}:\n{stderr}")]
    Git {
        program: String,
        args: Vec<String>,
        code: i32,
        cwd: PathBuf,
        stderr: String,
    },

    /// A checkpoint id was supplied that this workspace has never recorded.
    #[error("unknown checkpoint {id:?} for this workspace — list checkpoints before rolling back")]
    UnknownCheckpoint { id: String },

    /// An `edit_file` could not be applied (e.g. search text not found).
    #[error("cannot edit {path}: {reason}")]
    Edit { path: PathBuf, reason: String },

    /// A governed mutation was denied by the `HotL` gate; carries the scope and
    /// reason so the agent understands why and what to request.
    #[error("denied by policy: {scope} ({reason})")]
    Denied { scope: String, reason: String },

    /// A tool path argument was rejected because it would escape the workspace
    /// root (absolute path, `..`, or a rooted/prefixed component). Keeps the
    /// coding tools contained to the owner-scoped workspace.
    #[error("unsafe path {path}: {reason}")]
    UnsafePath { path: PathBuf, reason: String },

    /// SEC-22: a model-supplied value destined for a *positional* git argument
    /// was rejected at the boundary because git would parse it as an option
    /// (`-` prefix — option injection, e.g. `--receive-pack=<cmd>` on push).
    #[error("invalid {what} {value:?}: {reason}")]
    InvalidArgument {
        what: String,
        value: String,
        reason: String,
    },
}

impl CodingError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
