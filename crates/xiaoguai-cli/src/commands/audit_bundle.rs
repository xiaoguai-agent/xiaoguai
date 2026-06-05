//! `xiaoguai audit bundle` — one-command **evidence bundle** (DEC-037, P1.5).
//!
//! Wraps the existing chain-verified compliance export (DEC-016, non-bypassable)
//! and adds a human-readable Markdown transcript, so an operator can hand an
//! auditor a single folder: the signed JSON bundle plus a readable step-by-step
//! of what the agent did. Chain verification still runs server-side inside the
//! window before anything is emitted — a broken chain refuses the bundle.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Serialize;
use xiaoguai_audit::export::ComplianceBundle;

#[derive(Debug, Clone)]
pub struct BundleArgs {
    pub api_base: String,
    pub framework: String,
    pub from: String,
    pub to: String,
    /// Output directory; created if absent. Receives `audit-bundle.json` +
    /// `transcript.md`.
    pub out_dir: PathBuf,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    framework: &'a str,
    format: &'a str,
    from: &'a str,
    to: &'a str,
}

/// Fetch the chain-verified JSON export, write it alongside a rendered
/// transcript, and print a summary.
pub async fn run(args: BundleArgs) -> Result<()> {
    let client = Client::new();
    let url = format!("{}/v1/audit/exports", args.api_base.trim_end_matches('/'));
    let body = WireRequest {
        framework: &args.framework,
        format: "json",
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

    if !status.is_success() {
        eprintln!("{}", String::from_utf8_lossy(&bytes));
        match status.as_u16() {
            409 => bail!("audit chain broken inside window; bundle refused (see stderr)"),
            503 => bail!("audit chain exporter not wired on the server"),
            other => bail!("API returned HTTP {other}"),
        }
    }

    let bundle: ComplianceBundle =
        serde_json::from_slice(&bytes).context("parse compliance bundle JSON")?;

    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create {}", args.out_dir.display()))?;
    let json_path = args.out_dir.join("audit-bundle.json");
    let md_path = args.out_dir.join("transcript.md");
    std::fs::write(&json_path, &bytes).with_context(|| format!("write {}", json_path.display()))?;
    std::fs::write(&md_path, render_transcript(&bundle))
        .with_context(|| format!("write {}", md_path.display()))?;

    let proof = &bundle.header.chain_proof;
    println!(
        "evidence bundle written to {}\n  rows {}–{} ({}) · chain seal {}…\n  {}\n  {}",
        args.out_dir.display(),
        proof.first_id,
        proof.last_id,
        proof.count,
        short_hex(&proof.end_hmac_hex),
        json_path.display(),
        md_path.display(),
    );
    Ok(())
}

fn short_hex(hex: &str) -> &str {
    &hex[..hex.len().min(16)]
}

/// Render the bundle as a human-readable Markdown transcript. Pure — unit
/// tested without a server.
#[must_use]
pub fn render_transcript(bundle: &ComplianceBundle) -> String {
    let h = &bundle.header;
    let p = &h.chain_proof;
    let mut out = String::new();
    out.push_str("# Agent run — compliance evidence bundle\n\n");
    out.push_str(&format!("- **Framework:** {}\n", h.framework_label));
    out.push_str(&format!(
        "- **Window:** {} → {}\n",
        h.window.from, h.window.to
    ));
    out.push_str(&format!("- **Generated:** {}\n", h.generated_at));
    out.push_str(&format!(
        "- **Chain proof (tamper-evident seal):** rows {}–{} · count {} · end HMAC `{}`\n\n",
        p.first_id, p.last_id, p.count, p.end_hmac_hex
    ));

    if bundle.rows.is_empty() {
        out.push_str("_No audited actions in this window._\n");
        return out;
    }

    out.push_str("| # | time (UTC) | actor | action | resource | details |\n");
    out.push_str("|---|------------|-------|--------|----------|---------|\n");
    for r in &bundle.rows {
        out.push_str(&format!(
            "| {} | {} | {} | `{}` | {} | {} |\n",
            r.id,
            r.ts,
            md_cell(&r.actor),
            r.action,
            md_cell(r.resource.as_deref().unwrap_or("—")),
            md_cell(&r.details_summary),
        ));
    }
    out
}

/// Escape pipe + newline so a value can't break the Markdown table.
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use xiaoguai_audit::export::{BundleHeader, BundleRow, ChainProof, ExportWindow, Framework};

    fn sample() -> ComplianceBundle {
        let ts = Utc.with_ymd_and_hms(2026, 6, 5, 12, 0, 0).unwrap();
        ComplianceBundle {
            header: BundleHeader {
                framework: Framework::Soc2Cc72,
                framework_label: "SOC 2".to_string(),
                tenant_id: "ten_local_owner".to_string(),
                window: ExportWindow { from: ts, to: ts },
                generated_at: ts,
                chain_proof: ChainProof {
                    first_id: 1,
                    last_id: 2,
                    count: 2,
                    start_prev_hmac_hex: "00".repeat(32),
                    end_hmac_hex: "ab".repeat(32),
                },
            },
            rows: vec![BundleRow {
                id: 1,
                ts,
                actor: "agent".to_string(),
                action: "code.edit".to_string(),
                resource: Some("workspace:ws-1".to_string()),
                details_summary: "src/a.rs | hunks=1".to_string(),
            }],
        }
    }

    #[test]
    fn transcript_has_seal_and_row() {
        let md = render_transcript(&sample());
        assert!(md.contains("# Agent run"));
        assert!(md.contains("Chain proof"));
        assert!(md.contains("code.edit"));
        assert!(md.contains("workspace:ws-1"));
        // pipe in details_summary is escaped so it can't break the table
        assert!(md.contains("src/a.rs \\| hunks=1"));
    }

    #[test]
    fn empty_window_renders_a_note() {
        let mut b = sample();
        b.rows.clear();
        let md = render_transcript(&b);
        assert!(md.contains("No audited actions"));
    }
}
