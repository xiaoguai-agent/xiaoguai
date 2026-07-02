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
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use uuid::Uuid;

use crate::error::CodingError;
use crate::git;

/// Wall-clock ceiling for a single `run_command` invocation. A command still
/// running at the deadline is killed and its run reported as `timed_out` — a
/// resource-safety bound (option C adds no network/denylist sandbox), not a
/// security boundary.
const EXEC_TIMEOUT: Duration = Duration::from_secs(120);

/// Cap on the combined stdout+stderr captured from a command (64 KiB). Output
/// past the cap is dropped with a trailing marker so an unbounded build log
/// can't blow up memory or the audit/tool channel.
const EXEC_OUTPUT_CAP: usize = 64 * 1024;

/// Marker appended when captured output is truncated at [`EXEC_OUTPUT_CAP`].
const TRUNCATION_MARKER: &str = "\n…[truncated]";

/// Result of a [`Workspace::run_command`] — the combined output plus how the
/// process ended. `exit_code` is `None` when the process was terminated by a
/// signal (or killed on timeout); `timed_out` distinguishes the deadline kill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRun {
    /// stdout followed by stderr (each capped, see [`EXEC_OUTPUT_CAP`]).
    pub stdout_combined: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

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

    /// Run `command` via `sh -c` with the workspace root as the working
    /// directory, capturing stdout+stderr (combined, capped) under a wall-clock
    /// timeout. `[WRITE]` — the caller gates `tool_call.run_command` and audits
    /// `code.exec` first.
    ///
    /// The command inherits the server's environment and privileges (option C):
    /// governance is the master opt-in + consult-default + audit chain, not a
    /// sandbox. What this layer enforces is *resource safety* — the
    /// [`EXEC_TIMEOUT`] deadline (the child is killed and `timed_out` set on
    /// expiry) and the [`EXEC_OUTPUT_CAP`] on captured bytes — and that the cwd
    /// is the workspace root.
    pub async fn run_command(&self, command: &str) -> Result<CommandRun, CodingError> {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| CodingError::Launch {
                program: "sh".to_string(),
                source,
            })?;

        // Take the pipes so we can read them concurrently with `wait()` and
        // independently of the timeout outcome.
        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();

        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        let captured = async {
            let read_out = async {
                if let Some(pipe) = stdout_pipe.as_mut() {
                    pipe.read_to_end(&mut stdout_buf).await
                } else {
                    Ok(0)
                }
            };
            let read_err = async {
                if let Some(pipe) = stderr_pipe.as_mut() {
                    pipe.read_to_end(&mut stderr_buf).await
                } else {
                    Ok(0)
                }
            };
            // Drain both pipes and reap the child together; a process can block
            // on a full pipe if we wait before reading.
            let (status, out, err) = tokio::join!(child.wait(), read_out, read_err);
            // Surface the most actionable I/O error, but never let a drain
            // failure mask a successful status.
            out.and(err).map_err(|source| CodingError::Io {
                path: self.root.clone(),
                source,
            })?;
            status.map_err(|source| CodingError::Io {
                path: self.root.clone(),
                source,
            })
        };

        match tokio::time::timeout(EXEC_TIMEOUT, captured).await {
            Ok(result) => {
                let status = result?;
                Ok(CommandRun {
                    stdout_combined: combine_output(&stdout_buf, &stderr_buf),
                    exit_code: status.code(),
                    timed_out: false,
                })
            }
            Err(_elapsed) => {
                // Deadline hit: kill the child (best-effort) and report what we
                // captured so far. `start_kill` is non-blocking; the orphaned
                // `captured` future is dropped, which also drops the pipes.
                // Reap the killed child so it doesn't linger as a zombie until
                // the server process exits (tokio's Child does not reap on drop).
                let _ = child.start_kill();
                let _ = child.wait().await;
                Ok(CommandRun {
                    stdout_combined: combine_output(&stdout_buf, &stderr_buf),
                    exit_code: None,
                    timed_out: true,
                })
            }
        }
    }
}

/// Combine captured stdout then stderr into one lossy-UTF-8 string, capping the
/// total at [`EXEC_OUTPUT_CAP`] bytes with a [`TRUNCATION_MARKER`] when over.
fn combine_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut combined = String::from_utf8_lossy(stdout).into_owned();
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&String::from_utf8_lossy(stderr));
    }
    if combined.len() > EXEC_OUTPUT_CAP {
        // Truncate on a char boundary at/under the cap, then append the marker.
        let mut end = EXEC_OUTPUT_CAP;
        while end > 0 && !combined.is_char_boundary(end) {
            end -= 1;
        }
        combined.truncate(end);
        combined.push_str(TRUNCATION_MARKER);
    }
    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        (dir, ws)
    }

    #[tokio::test]
    async fn run_command_captures_stdout_and_zero_exit() {
        let (_dir, ws) = workspace().await;
        let run = ws.run_command("echo hi").await.unwrap();
        assert!(
            run.stdout_combined.contains("hi"),
            "stdout should contain echo output, got: {:?}",
            run.stdout_combined
        );
        assert_eq!(run.exit_code, Some(0));
        assert!(!run.timed_out);
    }

    #[tokio::test]
    async fn run_command_runs_in_the_workspace_root() {
        // `pwd` must report the workspace root (canonicalised), proving cwd is set.
        let (_dir, ws) = workspace().await;
        let run = ws.run_command("pwd").await.unwrap();
        let printed = run.stdout_combined.trim();
        assert_eq!(
            std::path::Path::new(printed),
            ws.root(),
            "command cwd must be the workspace root"
        );
        // And a file created relative to cwd lands under the root.
        ws.run_command("echo seeded > made-here.txt").await.unwrap();
        assert!(ws.root().join("made-here.txt").exists());
    }

    #[tokio::test]
    async fn run_command_captures_nonzero_exit_and_stderr() {
        let (_dir, ws) = workspace().await;
        let run = ws.run_command("echo oops 1>&2; exit 3").await.unwrap();
        assert_eq!(run.exit_code, Some(3));
        assert!(!run.timed_out);
        assert!(
            run.stdout_combined.contains("oops"),
            "stderr should be captured, got: {:?}",
            run.stdout_combined
        );
    }

    #[tokio::test]
    async fn run_command_times_out_and_is_marked() {
        // EXEC_TIMEOUT is 120s; pause tokio's clock and advance past it so the
        // test does not actually sleep. `sleep 600` would never finish on its
        // own, proving the timeout path kills the child.
        tokio::time::pause();
        let (_dir, ws) = workspace().await;
        let handle = tokio::spawn(async move { ws.run_command("sleep 600").await });
        // Let the child spawn, then jump past the deadline.
        tokio::task::yield_now().await;
        tokio::time::advance(EXEC_TIMEOUT + Duration::from_secs(1)).await;
        let run = handle.await.unwrap().unwrap();
        assert!(run.timed_out, "command past the deadline must be timed_out");
        assert_eq!(run.exit_code, None, "a killed child reports no exit code");
    }

    #[test]
    fn combine_output_caps_oversized_output_with_marker() {
        let big = vec![b'a'; EXEC_OUTPUT_CAP + 4096];
        let out = combine_output(&big, &[]);
        assert!(out.ends_with(TRUNCATION_MARKER), "must mark truncation");
        assert_eq!(
            out.len(),
            EXEC_OUTPUT_CAP + TRUNCATION_MARKER.len(),
            "capped to EXEC_OUTPUT_CAP plus the marker"
        );
    }

    #[test]
    fn combine_output_orders_stdout_then_stderr() {
        let out = combine_output(b"OUT", b"ERR");
        assert_eq!(out, "OUT\nERR");
    }
}
