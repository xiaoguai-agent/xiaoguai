//! `xiaoguai code` — drive the governed coding workflow (DEC-034/035) from the
//! CLI. Every mutation is checkpointed and signed into the HMAC audit chain
//! (`code.edit` / `git.commit` / `code.rollback` rows carrying the workspace +
//! checkpoint id), so a local change is as auditable + reversible as an
//! agent-driven one.
//!
//! The CLI runs under the **owner's implicit authority** — it uses an
//! allow-all gate rather than the interactive `HotL` suspend/approve flow (that
//! is the chat/server path). The audit trail and rollback are identical either
//! way, which is the point: `audit = see what it did; rollback = undo it`.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use xiaoguai_agent::AllowAllGate;
use xiaoguai_audit::chain::sink::SqliteAuditSink;
use xiaoguai_coding::{CheckpointId, FileEdit, GovernedTools, Workspace};
use xiaoguai_config::Settings;
use xiaoguai_core::coding_bridge::{AuditStepRecorder, HotlCodingGate};
use xiaoguai_storage::connect;

type Tools = GovernedTools<HotlCodingGate, AuditStepRecorder>;

/// Build a governed-tools facade bound to the real audit chain + an allow-all
/// gate, opening (or initialising) the workspace at `workspace`.
async fn build(settings: &Settings, workspace: &Path) -> Result<Tools> {
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("open SQLite store")?;
    // Sign with the same key the server uses so CLI-written rows verify in the
    // same chain: prefer the configured env var, fall back to the static key.
    let key = std::env::var(&settings.audit.signing_key_env)
        .ok()
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| settings.audit.hmac_key.clone());
    let sink = Arc::new(SqliteAuditSink::new(pool, key));
    let ws = Workspace::open_or_create(workspace)
        .await
        .context("open coding workspace")?;
    Ok(GovernedTools::new(
        ws,
        HotlCodingGate::new(Arc::new(AllowAllGate)),
        AuditStepRecorder::new(sink),
    ))
}

/// `xiaoguai code status` — porcelain status of the workspace.
pub async fn status(settings: &Settings, workspace: &Path) -> Result<()> {
    let tools = build(settings, workspace).await?;
    let out = tools.workspace().git_status().await?;
    if out.trim().is_empty() {
        println!("(clean)");
    } else {
        println!("{out}");
    }
    Ok(())
}

/// `xiaoguai code write <path> --content <text>` — governed whole-file write.
pub async fn write(
    settings: &Settings,
    workspace: &Path,
    path: &Path,
    content: String,
) -> Result<()> {
    let tools = build(settings, workspace).await?;
    let out = tools
        .edit_file(path, &FileEdit::Write(content))
        .await
        .context("governed edit_file")?;
    println!(
        "edited {} ({} bytes) — checkpoint {}",
        path.display(),
        out.result.bytes_after,
        out.checkpoint
    );
    Ok(())
}

/// `xiaoguai code commit <message>` — governed commit.
pub async fn commit(settings: &Settings, workspace: &Path, message: String) -> Result<()> {
    let tools = build(settings, workspace).await?;
    let out = tools
        .git_commit(&message)
        .await
        .context("governed git_commit")?;
    println!("committed {} — checkpoint {}", out.result, out.checkpoint);
    Ok(())
}

/// `xiaoguai code rollback <checkpoint>` — governed rollback to a checkpoint.
pub async fn rollback(settings: &Settings, workspace: &Path, checkpoint: String) -> Result<()> {
    let tools = build(settings, workspace).await?;
    tools
        .rollback(&CheckpointId::from_sha(checkpoint.clone()))
        .await
        .context("governed rollback")?;
    println!("rolled back to {checkpoint}");
    Ok(())
}

/// `xiaoguai code push <branch>` — governed push (egress; audited `git.push`).
pub async fn push(
    settings: &Settings,
    workspace: &Path,
    remote: String,
    branch: String,
) -> Result<()> {
    let tools = build(settings, workspace).await?;
    let out = tools
        .git_push(&remote, &branch)
        .await
        .context("governed git_push")?;
    println!("pushed {remote} {branch}");
    if !out.trim().is_empty() {
        println!("{out}");
    }
    Ok(())
}

/// `xiaoguai code open-pr <title>` — governed PR via `gh` (egress; `pr.open`).
pub async fn open_pr(
    settings: &Settings,
    workspace: &Path,
    title: String,
    body: String,
    base: String,
) -> Result<()> {
    let tools = build(settings, workspace).await?;
    let url = tools
        .open_pr(&title, &body, &base)
        .await
        .context("governed open_pr")?;
    println!("opened PR: {url}");
    Ok(())
}
