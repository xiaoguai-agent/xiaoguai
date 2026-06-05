//! Thin async wrapper over the system `git` (DEC-034: the persistent workspace
//! shells out to `git`, matching how the rest of the codebase runs
//! subprocesses, rather than adding a libgit2 / pure-Rust dependency). All
//! invocations are non-interactive and capture stdout/stderr.

use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::error::CodingError;

/// Env vars passed through to `git`/`gh` subprocesses (everything else is
/// scrubbed so host app secrets don't leak). Just enough to find the binaries,
/// their config/credential dirs, and authenticate a push/PR.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "APPDATA", // binaries + config/credential dirs
    "LANG",
    "LC_ALL",
    "LC_CTYPE", // locale (git decodes paths/messages with it)
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID", // ssh-based push
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_HOST",
    "GH_ENTERPRISE_TOKEN", // gh auth (push/PR)
];

/// Run `git <args>` in `cwd`, optionally with a dedicated `GIT_INDEX_FILE`
/// (used to snapshot the working tree without disturbing the user's real
/// index). Returns trimmed stdout on success; maps a non-zero exit or a
/// launch failure to a teaching [`CodingError`].
pub(crate) async fn run(
    cwd: &Path,
    args: &[&str],
    index_file: Option<&Path>,
) -> Result<String, CodingError> {
    exec("git", cwd, args, index_file).await
}

/// Run an arbitrary program (e.g. `gh` for `open_pr`) in `cwd`, capturing
/// stdout. Same teaching-error mapping as [`run`].
pub(crate) async fn run_program(
    program: &str,
    cwd: &Path,
    args: &[&str],
) -> Result<String, CodingError> {
    exec(program, cwd, args, None).await
}

async fn exec(
    program: &str,
    cwd: &Path,
    args: &[&str],
    index_file: Option<&Path>,
) -> Result<String, CodingError> {
    let mut cmd = Command::new(program);
    cmd.current_dir(cwd).args(args);

    // Scrub the environment to a git/gh-only allowlist so the host's app
    // secrets (audit signing key, provider API keys, OLLAMA_HOST, …) are NOT
    // handed to these subprocesses — a malicious in-tree `.git/config`/hook
    // could otherwise read them. Keep only what git/gh genuinely need to find
    // their binaries + config + credentials.
    cmd.env_clear();
    for key in ENV_ALLOWLIST {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    // git's own GIT_* knobs (GIT_SSH_COMMAND, GIT_CONFIG_*, …) are git-specific
    // and safe to pass through; our explicit GIT_* below still win (set last).
    for (key, val) in std::env::vars() {
        if key.starts_with("GIT_") {
            cmd.env(key, val);
        }
    }
    // Deterministic, non-interactive: never prompt, never read a pager.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(idx) = index_file {
        cmd.env("GIT_INDEX_FILE", idx);
    }

    let output = cmd.output().await.map_err(|source| CodingError::Launch {
        program: program.to_string(),
        source,
    })?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim_end().to_string())
    } else {
        Err(CodingError::Git {
            program: program.to_string(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
            code: output.status.code().unwrap_or(-1),
            cwd: cwd.to_path_buf(),
            stderr: String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_string(),
        })
    }
}

/// Run `git`, parsing a `-z` (NUL-delimited) listing into owned paths. Empty
/// output yields an empty vec (NOT a single empty entry).
pub(crate) async fn run_z(
    cwd: &Path,
    args: &[&str],
    index_file: Option<&Path>,
) -> Result<Vec<PathBuf>, CodingError> {
    let raw = run(cwd, args, index_file).await?;
    Ok(raw
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Is `dir` inside a git work tree?
pub(crate) async fn is_repo(dir: &Path) -> bool {
    run(dir, &["rev-parse", "--is-inside-work-tree"], None)
        .await
        .map(|s| s.trim() == "true")
        .unwrap_or(false)
}
