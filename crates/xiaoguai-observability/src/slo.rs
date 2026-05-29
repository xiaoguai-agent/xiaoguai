//! SLO contracts on top of the four SRE golden signals (DEC-022).
//!
//! This module owns the **declarative** half of the SLO module:
//!
//! - The `Slo` struct (and the enums it nests) — parsed from
//!   `docs/runbooks/slo.md` YAML by [`load_slos`].
//! - Lenient per-tenant override parsers — mirroring the
//!   `SandboxTier::from_str_lenient` pattern from `xiaoguai-tasks`.
//!
//! The **runtime** half (gauge registration, scrape loop) lives in
//! [`crate::prometheus`] as part of the unified `MetricHandles` registry.
//!
//! See `xiaoguai-agent-design/docs/lld/lld-observability.md` (LLD-OBS-001)
//! for the architecture and `docs/runbooks/slo.md` for the declared SLOs.

use serde::Deserialize;
use std::time::Duration;
use thiserror::Error;

// ── Declarative types (mirrored by docs/runbooks/slo.md YAML) ─────────────────

/// One published Service Level Objective.
#[derive(Debug, Clone, Deserialize)]
pub struct Slo {
    /// Stable identifier (used as `slo` label in alert rules).
    pub id: String,
    pub signal: Signal,
    pub surface: ContractSurface,
    pub threshold: Threshold,
    pub window: Window,
    #[serde(deserialize_with = "duration_from_str")]
    pub burn_rate_fast: Duration,
    #[serde(deserialize_with = "duration_from_str")]
    pub burn_rate_slow: Duration,
    pub page_chain: PageChain,
}

/// One of the four Google SRE golden signals.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Latency,
    Traffic,
    Errors,
    Saturation,
}

impl Signal {
    pub fn as_label(self) -> &'static str {
        match self {
            Signal::Latency => "latency",
            Signal::Traffic => "traffic",
            Signal::Errors => "errors",
            Signal::Saturation => "saturation",
        }
    }
}

/// The user-facing capability under contract.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContractSurface {
    /// An HTTP route (e.g. `/v1/chat/*`). The route pattern doubles as the
    /// `surface` label value in `xiaoguai_slo_burn_rate`.
    Http { route: String },
    /// A per-tenant budget (rate-limit, daily token cap). `limit_source`
    /// names where the limit is configured (config file path or table column).
    TenantBudget { limit_source: String },
    /// A background queue / worker pool.
    Queue { name: String },
}

impl ContractSurface {
    pub fn label_value(&self) -> &str {
        match self {
            ContractSurface::Http { route } => route,
            ContractSurface::TenantBudget { limit_source } => limit_source,
            ContractSurface::Queue { name } => name,
        }
    }
}

/// The numerical bound that makes the SLO measurable.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Threshold {
    LatencyP95Seconds { value: f64 },
    FirstTokenP95Seconds { value: f64 },
    Non2xxRate { value: f64 },
    RateLimitDenyRatio { value: f64 },
    UtilisationRatio { value: f64 },
    RequestsPerSecond { value: f64 },
}

/// The averaging window for the SLI computation.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Window {
    RollingHours {
        hours: u32,
    },
    /// Tenant-local day boundary (used for daily budgets like token saturation).
    DayBoundary,
}

/// How an alert breach reaches a human on-call.
#[derive(Debug, Clone, Deserialize)]
pub struct PageChain {
    pub severity: String, // "critical" | "warning"
    pub team: String,
    pub runbook_anchor: String,
}

/// The top-level YAML shape parsed from `docs/runbooks/slo.md` frontmatter.
#[derive(Debug, Deserialize)]
pub struct SloFile {
    pub slos: Vec<Slo>,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum SloError {
    #[error("failed to parse SLO YAML: {0}")]
    Parse(#[from] serde_yaml::Error),

    #[error("SLO declaration has invalid window: signal={signal:?} requested={requested:?}")]
    WindowOutOfRange { signal: Signal, requested: Duration },

    /// Lenient override parse failed — caller falls back to DEC-022 default
    /// and increments `xiaoguai_slo_override_parse_failed_total{tenant}`.
    #[error("tenant `{tenant}` override `{key}` failed lenient parse: raw=`{raw}`")]
    OverrideParse {
        tenant: String,
        key: String,
        raw: String,
    },
}

// ── Loader ────────────────────────────────────────────────────────────────────

/// Parse a YAML SLO declaration block into a `Vec<Slo>`.
///
/// The input is the YAML body (everything inside `slos: …` in
/// `docs/runbooks/slo.md`'s code fence). The operator's deploy process
/// extracts that block and passes it here at process start.
pub fn load_slos(yaml: &str) -> Result<Vec<Slo>, SloError> {
    let file: SloFile = serde_yaml::from_str(yaml)?;
    Ok(file.slos)
}

// ── Per-tenant override parsers (lenient) ─────────────────────────────────────
//
// Pattern mirrors `xiaoguai-tasks::SandboxTier::from_str_lenient` from sprint-8
// DEC-019: invalid input → `None` (fall back to default) rather than `Err`.
// Callers translate `None` into a counter increment for SRE visibility.

/// `tenant_settings.settings->>'slo_latency_p95_ms'` — integer ms in [100, `600_000`].
pub fn parse_latency_p95_ms_override(raw: &str) -> Option<u32> {
    raw.trim()
        .parse::<u32>()
        .ok()
        .filter(|ms| (100..=600_000).contains(ms))
}

/// `tenant_settings.settings->>'slo_error_budget_pct'` — float in (0.0001, 0.5].
pub fn parse_error_budget_pct_override(raw: &str) -> Option<f64> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| (0.0001..=0.5).contains(v))
}

/// `tenant_settings.settings->>'slo_saturation_ratio'` — float in [0.0, 1.0].
pub fn parse_saturation_ratio_override(raw: &str) -> Option<f64> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| (0.0..=1.0).contains(v))
}

/// `tenant_settings.settings->>'slo_alert_severity'` — "critical" | "warning".
/// Other values (including "ignore") fall back to the declaration's severity.
pub fn parse_alert_severity_override(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "critical" => Some("critical"),
        "warning" => Some("warning"),
        _ => None,
    }
}

// ── Duration helper ───────────────────────────────────────────────────────────

fn duration_from_str<'de, D>(d: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let s = String::deserialize(d)?;
    let s = s.trim();
    if s.is_empty() {
        return Err(D::Error::custom("empty duration"));
    }
    let (n_str, unit) = s.split_at(s.len() - 1);
    let n: u64 = n_str
        .parse()
        .map_err(|_| D::Error::custom(format!("invalid duration number: `{n_str}`")))?;
    match unit {
        "s" => Ok(Duration::from_secs(n)),
        "m" => Ok(Duration::from_secs(n * 60)),
        "h" => Ok(Duration::from_secs(n * 3_600)),
        "d" => Ok(Duration::from_secs(n * 86_400)),
        other => Err(D::Error::custom(format!(
            "invalid duration unit `{other}` (expected s/m/h/d)"
        ))),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r##"
slos:
  - id: api-latency-chat-p95-5s
    signal: latency
    surface:
      kind: http
      route: /v1/chat/*
    threshold:
      kind: latency_p95_seconds
      value: 5.0
    window:
      kind: rolling_hours
      hours: 1
    burn_rate_fast: 1h
    burn_rate_slow: 6h
    page_chain:
      severity: critical
      team: platform
      runbook_anchor: "#api-latency-fast-burn"

  - id: api-saturation-tenant-llm-budget
    signal: saturation
    surface:
      kind: tenant_budget
      limit_source: tenant_settings.daily_llm_token_budget
    threshold:
      kind: utilisation_ratio
      value: 0.8
    window:
      kind: day_boundary
    burn_rate_fast: 1h
    burn_rate_slow: 6h
    page_chain:
      severity: warning
      team: tenant-ops
      runbook_anchor: "#saturation-fast-burn"
"##;

    #[test]
    fn load_slos_parses_sample() {
        let slos = load_slos(SAMPLE_YAML).expect("parse SAMPLE_YAML");
        assert_eq!(slos.len(), 2);
        assert_eq!(slos[0].signal, Signal::Latency);
        assert_eq!(slos[0].id, "api-latency-chat-p95-5s");
        assert_eq!(slos[0].burn_rate_fast, Duration::from_secs(3_600));
        assert_eq!(slos[0].burn_rate_slow, Duration::from_secs(6 * 3_600));
        assert_eq!(slos[1].signal, Signal::Saturation);
        assert!(matches!(slos[1].window, Window::DayBoundary));
    }

    #[test]
    fn signal_label_round_trip() {
        for s in [
            Signal::Latency,
            Signal::Traffic,
            Signal::Errors,
            Signal::Saturation,
        ] {
            assert!(!s.as_label().is_empty());
        }
        assert_eq!(Signal::Latency.as_label(), "latency");
    }

    #[test]
    fn contract_surface_label_value() {
        let http = ContractSurface::Http {
            route: "/v1/chat/*".into(),
        };
        assert_eq!(http.label_value(), "/v1/chat/*");
        let tb = ContractSurface::TenantBudget {
            limit_source: "tenant_settings.daily_llm_token_budget".into(),
        };
        assert_eq!(tb.label_value(), "tenant_settings.daily_llm_token_budget");
    }

    #[test]
    fn parse_latency_override_lenient() {
        assert_eq!(parse_latency_p95_ms_override("2500"), Some(2500));
        assert_eq!(parse_latency_p95_ms_override("  2500  "), Some(2500));
        assert_eq!(parse_latency_p95_ms_override("fast"), None);
        assert_eq!(parse_latency_p95_ms_override("50"), None); // < 100
        assert_eq!(parse_latency_p95_ms_override("1000000"), None); // > 600_000
        assert_eq!(parse_latency_p95_ms_override(""), None);
        assert_eq!(parse_latency_p95_ms_override("-5"), None); // parse u32 rejects
    }

    #[test]
    fn parse_error_budget_pct_lenient() {
        assert_eq!(parse_error_budget_pct_override("0.005"), Some(0.005));
        assert_eq!(parse_error_budget_pct_override("0.5"), Some(0.5));
        assert_eq!(parse_error_budget_pct_override("0.0"), None); // 0 not allowed
        assert_eq!(parse_error_budget_pct_override("0.99"), None); // > 0.5
        assert_eq!(parse_error_budget_pct_override("foo"), None);
    }

    #[test]
    fn parse_saturation_ratio_lenient() {
        assert_eq!(parse_saturation_ratio_override("0.95"), Some(0.95));
        assert_eq!(parse_saturation_ratio_override("0.0"), Some(0.0));
        assert_eq!(parse_saturation_ratio_override("1.0"), Some(1.0));
        assert_eq!(parse_saturation_ratio_override("1.5"), None);
        assert_eq!(parse_saturation_ratio_override("foo"), None);
    }

    #[test]
    fn parse_severity_override_case_insensitive() {
        assert_eq!(parse_alert_severity_override("Critical"), Some("critical"));
        assert_eq!(parse_alert_severity_override("WARNING"), Some("warning"));
        assert_eq!(parse_alert_severity_override("ignore"), None);
        assert_eq!(parse_alert_severity_override(""), None);
    }

    #[test]
    fn duration_parses_units() {
        // Round-trip the duration_from_str helper via a minimal YAML.
        let y = r##"
slos:
  - id: test
    signal: latency
    surface: { kind: http, route: "/x" }
    threshold: { kind: latency_p95_seconds, value: 1.0 }
    window: { kind: rolling_hours, hours: 1 }
    burn_rate_fast: 30s
    burn_rate_slow: 2d
    page_chain: { severity: warning, team: platform, runbook_anchor: "#x" }
"##;
        let slos = load_slos(y).expect("parse");
        assert_eq!(slos[0].burn_rate_fast, Duration::from_secs(30));
        assert_eq!(slos[0].burn_rate_slow, Duration::from_secs(2 * 86_400));
    }

    #[test]
    fn duration_rejects_invalid_unit() {
        let y = r##"
slos:
  - id: test
    signal: latency
    surface: { kind: http, route: "/x" }
    threshold: { kind: latency_p95_seconds, value: 1.0 }
    window: { kind: rolling_hours, hours: 1 }
    burn_rate_fast: 30x
    burn_rate_slow: 6h
    page_chain: { severity: warning, team: platform, runbook_anchor: "#x" }
"##;
        let err = load_slos(y).expect_err("must reject `30x`");
        assert!(matches!(err, SloError::Parse(_)));
    }
}
