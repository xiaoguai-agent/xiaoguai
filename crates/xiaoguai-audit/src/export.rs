//! Compliance export from the HMAC-chained audit log.
//!
//! Produces SOC2 CC7.2 / GDPR Art. 30 / HIPAA §164.312 bundles over a time
//! window. Every bundle carries a `ChainProof` header so auditors don't have
//! to take our word that the chain is intact.
//!
//! ## Design constraints
//!
//! - **JSON is canonical.** CSV is a projection — same row count, same column
//!   meanings, no synthesized columns.
//! - **Chain verification is non-bypassable.** If `verify_chain` fails inside
//!   the window, [`export_bundle`] returns [`ExportError::ChainBroken`] with
//!   the first broken row's id + ts. There is no `skip_verify` flag.
//! - **Templates are static.** Each framework is a hardcoded `match` arm
//!   over `action` strings, not a runtime DSL.
//! - **PDF is deferred.** [`render_pdf`] is a stub that returns
//!   [`ExportError::PdfUnimplemented`] so the API surface is in place.
//!
//! ## Window semantics
//!
//! `export_bundle` verifies chain continuity *within the window* — it walks
//! `verify_chain(&rows[0].prev_hmac, &rows)`, trusting the first row's
//! `prev_hmac` as the start. This is intentional: the global chain integrity
//! is checked by the existing `/v1/admin/audit/verify` endpoint, which walks
//! the full chain from genesis. The export proof only certifies that the
//! window itself is internally consistent.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::chain::{ChainError, ChainedAudit, StoredEntry, HMAC_LEN};

/// One of the three supported compliance frameworks.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Framework {
    /// SOC2 Trust Services Criteria CC7.2 — system monitoring.
    Soc2Cc72,
    /// GDPR Article 30 — records of processing activities.
    GdprArt30,
    /// HIPAA §164.312 — technical safeguards (access control + audit controls).
    Hipaa164312,
}

impl Framework {
    /// Parse a CLI-friendly short name.
    ///
    /// # Errors
    /// Returns the unrecognised input verbatim in the `Err` variant.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "soc2" | "soc2-cc7.2" | "soc2cc72" => Ok(Self::Soc2Cc72),
            "gdpr" | "gdpr-art30" | "gdprart30" => Ok(Self::GdprArt30),
            "hipaa" | "hipaa-164.312" | "hipaa164312" => Ok(Self::Hipaa164312),
            other => Err(other.into()),
        }
    }

    /// Human-readable framework label used in headers and runbook output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Soc2Cc72 => "SOC2 CC7.2 (System Monitoring)",
            Self::GdprArt30 => "GDPR Art. 30 (Records of Processing)",
            Self::Hipaa164312 => "HIPAA §164.312 (Technical Safeguards)",
        }
    }
}

/// Output format. PDF is reserved — see [`render_pdf`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Json,
    Csv,
    Pdf,
}

impl Format {
    /// Parse the CLI flag value.
    ///
    /// # Errors
    /// Returns the unrecognised input verbatim in the `Err` variant.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            "pdf" => Ok(Self::Pdf),
            other => Err(other.into()),
        }
    }

    /// `Content-Type` for HTTP responses.
    #[must_use]
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            Self::Csv => "text/csv",
            Self::Pdf => "application/pdf",
        }
    }
}

/// Inclusive `[from, to]` window used to filter the audit chain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportWindow {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

impl ExportWindow {
    /// Construct a window. Returns `None` if `from > to`.
    #[must_use]
    pub fn new(from: DateTime<Utc>, to: DateTime<Utc>) -> Option<Self> {
        if from > to {
            None
        } else {
            Some(Self { from, to })
        }
    }
}

/// Chain-integrity proof embedded in every bundle header.
///
/// `start_prev_hmac_hex` is the `prev_hmac` of the first row in the window.
/// `end_hmac_hex` is the `hmac` of the last row. An auditor can re-walk the
/// rows offline (using the canonical encoding from `chain.rs`) and confirm
/// `end_hmac` matches the recomputed terminal HMAC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChainProof {
    pub first_id: i64,
    pub last_id: i64,
    pub count: u64,
    pub start_prev_hmac_hex: String,
    pub end_hmac_hex: String,
}

impl ChainProof {
    /// Empty-window sentinel — zero ids, all-zero hex markers.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            first_id: 0,
            last_id: 0,
            count: 0,
            start_prev_hmac_hex: "00".repeat(HMAC_LEN),
            end_hmac_hex: "00".repeat(HMAC_LEN),
        }
    }
}

/// Header of a compliance bundle. Always present, including on empty windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleHeader {
    pub framework: Framework,
    pub framework_label: String,
    pub tenant_id: String,
    pub window: ExportWindow,
    pub generated_at: DateTime<Utc>,
    pub chain_proof: ChainProof,
}

/// A single projected row in the bundle.
///
/// `details_summary` is a short text projection — keys + scalar values joined
/// by `; `. The full `details` JSON stays in the source `audit_log`; the
/// auditor can request a raw extract if needed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleRow {
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub resource: Option<String>,
    pub details_summary: String,
}

/// The compliance export bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceBundle {
    pub header: BundleHeader,
    pub rows: Vec<BundleRow>,
}

/// Errors that may abort an export.
#[derive(Debug, Error, Serialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum ExportError {
    #[error("audit chain broken at row {first_broken_id} ({first_broken_ts})")]
    ChainBroken {
        first_broken_id: i64,
        first_broken_ts: DateTime<Utc>,
    },

    #[error("pdf rendering is not yet implemented — track as follow-up")]
    PdfUnimplemented,

    #[error("chain engine error: {message}")]
    Chain { message: String },
}

impl From<ChainError> for ExportError {
    fn from(e: ChainError) -> Self {
        Self::Chain {
            message: e.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-framework template projections (Step 4.2)
// ---------------------------------------------------------------------------

/// SOC2 CC7.2 — system-monitoring actions.
///
/// Filter set is deliberately hardcoded; runtime overrides are out of scope
/// (see plan §7). To add an action, edit this `match` and the matching
/// runbook section.
fn soc2_cc72_keeps(action: &str) -> bool {
    matches!(
        action,
        "session.create"
            | "session.cancel"
            | "tool.invoke"
            | "tool.deny"
            | "auth.login"
            | "auth.failure"
            | "policy.deny"
            | "audit.verify"
            | "cost.charge"
            | "hotl.escalate"
    )
}

/// GDPR Art. 30 — records of processing activities (personal data flows).
fn gdpr_art30_keeps(action: &str) -> bool {
    matches!(
        action,
        "memory.create"
            | "memory.update"
            | "memory.delete"
            | "memory.recall"
            | "session.create"
            | "session.delete"
            | "data.export"
            | "data.purge"
            | "consent.grant"
            | "consent.revoke"
    )
}

/// HIPAA §164.312 — technical safeguards (access control + audit controls).
///
/// PHI-flagged tool invocations (`resource` starts with `phi:`) are kept; all
/// other `tool.invoke` rows are dropped.
fn hipaa_164312_keeps(action: &str, resource: Option<&str>) -> bool {
    match action {
        "auth.login" | "auth.failure" | "session.create" | "audit.verify" | "policy.deny" => true,
        "tool.invoke" => resource.is_some_and(|r| r.starts_with("phi:")),
        _ => false,
    }
}

fn project(framework: Framework, rows: &[StoredEntry]) -> Vec<BundleRow> {
    rows.iter()
        .filter(|r| match framework {
            Framework::Soc2Cc72 => soc2_cc72_keeps(&r.entry.action),
            Framework::GdprArt30 => gdpr_art30_keeps(&r.entry.action),
            Framework::Hipaa164312 => {
                hipaa_164312_keeps(&r.entry.action, r.entry.resource.as_deref())
            }
        })
        .map(|r| BundleRow {
            id: r.id,
            ts: r.entry.ts,
            actor: r.entry.actor.clone(),
            action: r.entry.action.clone(),
            resource: r.entry.resource.clone(),
            details_summary: summarize_details(&r.entry.details),
        })
        .collect()
}

/// Flatten a `details` JSON into a short `key=value; key=value` line.
///
/// Nested objects/arrays are rendered as their JSON-compact form (auditors
/// generally want one-line-per-row CSV).
fn summarize_details(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut parts: Vec<String> = map
                .iter()
                .map(|(k, val)| format!("{k}={}", scalar_or_json(val)))
                .collect();
            parts.sort();
            parts.join("; ")
        }
        other => scalar_or_json(other),
    }
}

fn scalar_or_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Bundle assembly (Step 4.3)
// ---------------------------------------------------------------------------

/// Build a compliance bundle for `framework` over `rows`.
///
/// `rows` MUST be the slice of audit entries returned by the storage layer
/// for `[window.from, window.to]`, in chronological order. The function:
///
/// 1. Verifies chain continuity *within the slice* — calls
///    `chain.verify_chain(&rows[0].prev_hmac, rows)`. The global chain (back
///    to genesis) is the job of `/v1/admin/audit/verify`.
/// 2. Builds a `ChainProof` carrying first/last id, count, and the boundary
///    HMACs in hex.
/// 3. Applies the per-framework projection.
///
/// # Errors
///
/// - [`ExportError::ChainBroken`] — the slice fails `verify_chain`. The
///   error includes the first broken row's `id` and `ts`.
/// - [`ExportError::Chain`] — engine-level error (e.g. bad HMAC length).
pub fn export_bundle(
    framework: Framework,
    tenant_id: String,
    rows: Vec<StoredEntry>,
    window: ExportWindow,
    chain: &ChainedAudit,
) -> Result<ComplianceBundle, ExportError> {
    let generated_at = Utc::now();

    if rows.is_empty() {
        return Ok(ComplianceBundle {
            header: BundleHeader {
                framework,
                framework_label: framework.label().into(),
                tenant_id,
                window,
                generated_at,
                chain_proof: ChainProof::empty(),
            },
            rows: Vec::new(),
        });
    }

    let start_prev = rows[0].prev_hmac.clone();
    if start_prev.len() != HMAC_LEN {
        return Err(ExportError::Chain {
            message: "invalid prev_hmac length on first row".into(),
        });
    }

    if let Err(e) = chain.verify_chain(&start_prev, &rows) {
        let broken_id = match e {
            ChainError::HmacMismatch(id) | ChainError::LinkBroken(_, id) => id,
            other => return Err(other.into()),
        };
        let broken_ts = rows
            .iter()
            .find(|r| r.id == broken_id)
            .map_or_else(Utc::now, |r| r.entry.ts);
        return Err(ExportError::ChainBroken {
            first_broken_id: broken_id,
            first_broken_ts: broken_ts,
        });
    }

    let first = rows
        .first()
        .expect("non-empty checked above; QED slice has a first row");
    let last = rows
        .last()
        .expect("non-empty checked above; QED slice has a last row");

    let chain_proof = ChainProof {
        first_id: first.id,
        last_id: last.id,
        count: u64::try_from(rows.len()).unwrap_or(u64::MAX),
        start_prev_hmac_hex: hex::encode(&start_prev),
        end_hmac_hex: hex::encode(&last.hmac),
    };

    let projected = project(framework, &rows);

    Ok(ComplianceBundle {
        header: BundleHeader {
            framework,
            framework_label: framework.label().into(),
            tenant_id,
            window,
            generated_at,
            chain_proof,
        },
        rows: projected,
    })
}

// ---------------------------------------------------------------------------
// Serialization (Step 4.4)
// ---------------------------------------------------------------------------

/// Render the bundle as canonical JSON.
///
/// # Errors
/// Returns [`ExportError::Chain`] wrapping the serde error if serialization
/// fails (shouldn't happen for a well-formed `ComplianceBundle`).
pub fn render_json(bundle: &ComplianceBundle) -> Result<String, ExportError> {
    serde_json::to_string_pretty(bundle).map_err(|e| ExportError::Chain {
        message: format!("json encode: {e}"),
    })
}

/// Render the bundle as CSV.
///
/// Column set: `id,ts,actor,action,resource,details_summary` — a subset of
/// the JSON row keys, no synthesized fields. RFC 4180 escaping (CRLF line
/// endings; quote fields containing `,`, `"`, or `\n`).
///
/// The CSV is preceded by a `# bundle-header: { ... }` comment line carrying
/// the JSON header so the chain proof travels with the CSV file. Auditors
/// reading the CSV with Excel will see the header as an extra comment row;
/// programmatic parsers can skip lines starting with `#`.
///
/// # Errors
/// Returns [`ExportError::Chain`] if header serialization fails.
pub fn render_csv(bundle: &ComplianceBundle) -> Result<String, ExportError> {
    let header_json = serde_json::to_string(&bundle.header).map_err(|e| ExportError::Chain {
        message: format!("csv header encode: {e}"),
    })?;
    let mut out = String::new();
    out.push_str("# bundle-header: ");
    out.push_str(&header_json);
    out.push_str("\r\n");
    out.push_str("id,ts,actor,action,resource,details_summary\r\n");
    for row in &bundle.rows {
        out.push_str(&row.id.to_string());
        out.push(',');
        out.push_str(&csv_escape(&row.ts.to_rfc3339()));
        out.push(',');
        out.push_str(&csv_escape(&row.actor));
        out.push(',');
        out.push_str(&csv_escape(&row.action));
        out.push(',');
        out.push_str(&csv_escape(row.resource.as_deref().unwrap_or("")));
        out.push(',');
        out.push_str(&csv_escape(&row.details_summary));
        out.push_str("\r\n");
    }
    Ok(out)
}

/// PDF rendering stub — surface area in place, no implementation.
///
/// # Errors
/// Always returns [`ExportError::PdfUnimplemented`]. Tracked as a post-T5
/// follow-up.
pub fn render_pdf(_bundle: &ComplianceBundle) -> Result<Vec<u8>, ExportError> {
    Err(ExportError::PdfUnimplemented)
}

/// Render the bundle in the requested format.
///
/// Convenience wrapper around the three renderers. PDF returns the stub
/// error.
///
/// # Errors
/// Propagates any of the underlying render errors.
pub fn render(bundle: &ComplianceBundle, format: Format) -> Result<Vec<u8>, ExportError> {
    match format {
        Format::Json => render_json(bundle).map(String::into_bytes),
        Format::Csv => render_csv(bundle).map(String::into_bytes),
        Format::Pdf => render_pdf(bundle),
    }
}

/// RFC 4180 minimal escaping — quote any field containing `,`, `"`, `\r`, or
/// `\n`; double internal `"`.
fn csv_escape(s: &str) -> String {
    let needs_quote = s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r');
    if !needs_quote {
        return s.to_string();
    }
    let escaped = s.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

// ---------------------------------------------------------------------------
// Unit tests (Step 4.5)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{AuditEntry, ChainedAudit, StoredEntry};
    use chrono::TimeZone;
    use serde_json::json;

    const KEY: &[u8] = b"export-test-key-do-not-use-in-prod";

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap()
    }

    fn build_entry(
        ts_offset_secs: i64,
        action: &str,
        actor: &str,
        resource: Option<&str>,
        details: serde_json::Value,
    ) -> AuditEntry {
        AuditEntry {
            ts: t0() + chrono::Duration::seconds(ts_offset_secs),
            tenant_id: "tenant-1".into(),
            actor: actor.into(),
            action: action.into(),
            resource: resource.map(String::from),
            details,
        }
    }

    fn build_stored(chain: &ChainedAudit, entries: Vec<AuditEntry>) -> Vec<StoredEntry> {
        let mut prev = vec![0u8; HMAC_LEN];
        let mut out = Vec::with_capacity(entries.len());
        for (i, e) in entries.into_iter().enumerate() {
            let h = chain.compute_hmac(&prev, &e).expect("hmac");
            out.push(StoredEntry {
                id: i64::try_from(i + 1).unwrap(),
                entry: e,
                prev_hmac: prev.clone(),
                hmac: h.clone(),
            });
            prev = h;
        }
        out
    }

    fn window() -> ExportWindow {
        ExportWindow {
            from: t0(),
            to: t0() + chrono::Duration::days(1),
        }
    }

    fn fixture() -> (ChainedAudit, Vec<StoredEntry>) {
        let chain = ChainedAudit::new(KEY.to_vec());
        let entries = vec![
            build_entry(0, "session.create", "user:1", Some("session:a"), json!({})),
            build_entry(
                1,
                "tool.invoke",
                "user:1",
                Some("phi:patient/42"),
                json!({"tool":"lookup"}),
            ),
            build_entry(2, "memory.recall", "user:1", None, json!({"q":"hello"})),
            build_entry(3, "tool.invoke", "user:1", Some("public:doc/1"), json!({})),
            build_entry(4, "auth.login", "user:2", None, json!({"ip":"203.0.113.1"})),
            build_entry(
                5,
                "policy.deny",
                "system",
                Some("budget:llm"),
                json!({"why":"cap"}),
            ),
            build_entry(6, "memory.delete", "user:1", Some("memory:42"), json!({})),
            build_entry(7, "cost.charge", "system", None, json!({"usd":0.04})),
            build_entry(8, "data.export", "user:1", Some("export:csv"), json!({})),
            build_entry(9, "audit.verify", "system", None, json!({"ok":true})),
        ];
        let stored = build_stored(&chain, entries);
        (chain, stored)
    }

    #[test]
    fn framework_parse_accepts_known_short_names() {
        assert_eq!(Framework::parse("soc2"), Ok(Framework::Soc2Cc72));
        assert_eq!(Framework::parse("GDPR"), Ok(Framework::GdprArt30));
        assert_eq!(
            Framework::parse("hipaa-164.312"),
            Ok(Framework::Hipaa164312)
        );
        assert!(Framework::parse("iso27001").is_err());
    }

    #[test]
    fn format_parse_round_trips() {
        assert_eq!(Format::parse("json"), Ok(Format::Json));
        assert_eq!(Format::parse("CSV"), Ok(Format::Csv));
        assert_eq!(Format::parse("pdf"), Ok(Format::Pdf));
        assert!(Format::parse("xml").is_err());
    }

    #[test]
    fn window_rejects_inverted_range() {
        let later = t0() + chrono::Duration::days(1);
        assert!(ExportWindow::new(later, t0()).is_none());
        assert!(ExportWindow::new(t0(), later).is_some());
    }

    #[test]
    fn projection_soc2_filters_correctly() {
        let (_chain, stored) = fixture();
        let rows = project(Framework::Soc2Cc72, &stored);
        let actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
        // SOC2 keeps: session.create, tool.invoke (x2), auth.login, policy.deny,
        // cost.charge, audit.verify — drops: memory.recall, memory.delete, data.export.
        assert!(actions.contains(&"session.create"));
        assert!(actions.contains(&"auth.login"));
        assert!(actions.contains(&"policy.deny"));
        assert!(actions.contains(&"cost.charge"));
        assert!(actions.contains(&"audit.verify"));
        assert_eq!(
            actions.iter().filter(|a| **a == "tool.invoke").count(),
            2,
            "SOC2 keeps all tool.invoke regardless of resource"
        );
        assert!(!actions.contains(&"memory.recall"));
        assert!(!actions.contains(&"memory.delete"));
        assert!(!actions.contains(&"data.export"));
    }

    #[test]
    fn projection_gdpr_filters_correctly() {
        let (_chain, stored) = fixture();
        let rows = project(Framework::GdprArt30, &stored);
        let actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
        assert!(actions.contains(&"memory.recall"));
        assert!(actions.contains(&"memory.delete"));
        assert!(actions.contains(&"data.export"));
        assert!(actions.contains(&"session.create"));
        // GDPR drops auth.login, policy.deny, cost.charge, audit.verify, tool.invoke.
        assert!(!actions.contains(&"auth.login"));
        assert!(!actions.contains(&"policy.deny"));
        assert!(!actions.contains(&"cost.charge"));
        assert!(!actions.contains(&"audit.verify"));
        assert!(!actions.contains(&"tool.invoke"));
    }

    #[test]
    fn projection_hipaa_filters_by_phi_resource() {
        let (_chain, stored) = fixture();
        let rows = project(Framework::Hipaa164312, &stored);
        // HIPAA keeps: auth.login, session.create, policy.deny, audit.verify,
        // and tool.invoke ONLY when resource starts with `phi:`.
        let tool_rows: Vec<&BundleRow> =
            rows.iter().filter(|r| r.action == "tool.invoke").collect();
        assert_eq!(tool_rows.len(), 1);
        assert_eq!(tool_rows[0].resource.as_deref(), Some("phi:patient/42"));
    }

    #[test]
    fn export_bundle_happy_path_carries_chain_proof() {
        let (chain, stored) = fixture();
        let bundle = export_bundle(
            Framework::Soc2Cc72,
            "tenant-1".into(),
            stored.clone(),
            window(),
            &chain,
        )
        .expect("happy path");

        assert_eq!(bundle.header.chain_proof.first_id, 1);
        assert_eq!(bundle.header.chain_proof.last_id, 10);
        assert_eq!(bundle.header.chain_proof.count, 10);
        assert_eq!(
            bundle.header.chain_proof.start_prev_hmac_hex,
            "00".repeat(HMAC_LEN)
        );
        assert_eq!(
            bundle.header.chain_proof.end_hmac_hex,
            hex::encode(&stored.last().unwrap().hmac)
        );
        assert_eq!(bundle.header.tenant_id, "tenant-1");
    }

    #[test]
    fn export_bundle_refuses_tampered_chain() {
        let (chain, mut stored) = fixture();
        // Mutate the JSON payload of row 3 — its stored HMAC was computed
        // over the *original* details, so verify_chain finds a mismatch at id 3.
        stored[2].entry.details = json!({ "tampered": true });

        let err = export_bundle(
            Framework::Soc2Cc72,
            "tenant-1".into(),
            stored,
            window(),
            &chain,
        )
        .expect_err("must refuse");

        match err {
            ExportError::ChainBroken {
                first_broken_id, ..
            } => assert_eq!(first_broken_id, 3),
            other => panic!("expected ChainBroken, got {other:?}"),
        }
    }

    #[test]
    fn export_bundle_empty_window_is_not_error() {
        let chain = ChainedAudit::new(KEY.to_vec());
        let bundle = export_bundle(
            Framework::Soc2Cc72,
            "tenant-1".into(),
            Vec::new(),
            window(),
            &chain,
        )
        .expect("empty is fine");
        assert_eq!(bundle.header.chain_proof.count, 0);
        assert_eq!(bundle.rows.len(), 0);
        assert_eq!(
            bundle.header.chain_proof.start_prev_hmac_hex,
            "00".repeat(HMAC_LEN)
        );
    }

    #[test]
    fn json_and_csv_have_same_row_count() {
        let (chain, stored) = fixture();
        let bundle = export_bundle(
            Framework::Soc2Cc72,
            "tenant-1".into(),
            stored,
            window(),
            &chain,
        )
        .unwrap();
        let json = render_json(&bundle).unwrap();
        let csv = render_csv(&bundle).unwrap();

        // JSON row count from the parsed Value.
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let json_rows = parsed.get("rows").and_then(|v| v.as_array()).unwrap().len();

        // CSV row count = total non-empty lines minus the header line and the
        // bundle-header comment.
        let csv_rows = csv
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("id,"))
            .count();

        assert_eq!(json_rows, csv_rows);
        assert_eq!(json_rows, bundle.rows.len());
    }

    #[test]
    fn csv_escapes_commas_quotes_and_newlines() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("has,comma"), "\"has,comma\"");
        assert_eq!(csv_escape("has\"quote"), "\"has\"\"quote\"");
        assert_eq!(csv_escape("has\nnewline"), "\"has\nnewline\"");
    }

    #[test]
    fn pdf_render_returns_unimplemented() {
        let (chain, stored) = fixture();
        let bundle = export_bundle(
            Framework::Soc2Cc72,
            "tenant-1".into(),
            stored,
            window(),
            &chain,
        )
        .unwrap();
        let err = render_pdf(&bundle).unwrap_err();
        assert!(matches!(err, ExportError::PdfUnimplemented));
    }

    #[test]
    fn summarize_details_sorts_keys() {
        let v = json!({ "b": 2, "a": 1 });
        let s = summarize_details(&v);
        assert_eq!(s, "a=1; b=2");
    }
}
