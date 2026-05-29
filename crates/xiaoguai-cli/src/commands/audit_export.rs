//! `xiaoguai audit export` — request a compliance bundle from the API.
//!
//! Posts to `/v1/audit/exports`. Writes the response body to `--output` on
//! 2xx; on 409 Conflict (chain broken inside the window), prints the
//! machine-readable error JSON to stderr and returns `Err(...)` so the
//! process exits non-zero. On 501, surfaces "PDF unimplemented". On any
//! other non-2xx, bubbles up.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct ExportArgs {
    pub api_base: String,
    pub tenant_id: String,
    /// Short framework name — `"soc2"`, `"gdpr"`, `"hipaa"`.
    pub framework: String,
    /// RFC3339 inclusive lower bound.
    pub from: String,
    /// RFC3339 inclusive upper bound.
    pub to: String,
    pub output: PathBuf,
    /// Output format — `"json"` or `"csv"`. PDF is reserved (returns 501).
    pub format: String,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    tenant_id: &'a str,
    framework: &'a str,
    format: &'a str,
    from: &'a str,
    to: &'a str,
}

/// Run the export. Writes bytes to `args.output` on success.
///
/// # Errors
///
/// - 409 Conflict: prints `chain_broken` JSON to stderr and returns
///   `Err("audit chain broken; export refused")`.
/// - non-2xx: returns the body verbatim wrapped in an error.
/// - network / IO / parse errors: bubble up via `anyhow::Context`.
pub async fn run(args: ExportArgs) -> Result<()> {
    let client = Client::new();
    let url = format!("{}/v1/audit/exports", args.api_base.trim_end_matches('/'));
    let body = WireRequest {
        tenant_id: &args.tenant_id,
        framework: &args.framework,
        format: &args.format,
        from: &args.from,
        to: &args.to,
    };

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;

    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("read body from {url}"))?;

    if status.is_success() {
        std::fs::write(&args.output, &bytes)
            .with_context(|| format!("write to {}", args.output.display()))?;
        eprintln!("wrote {} bytes to {}", bytes.len(), args.output.display());
        return Ok(());
    }

    // Surface the API's error JSON verbatim on stderr so the operator can
    // re-direct/pipe it.
    let body_text = String::from_utf8_lossy(&bytes);
    eprintln!("{body_text}");

    match status.as_u16() {
        409 => bail!("audit chain broken inside window; export refused (see stderr for first_broken_id + first_broken_ts)"),
        501 => bail!("export format not implemented (likely pdf)"),
        503 => bail!("audit chain exporter not wired on the server"),
        other => bail!("API returned HTTP {other}"),
    }
}
