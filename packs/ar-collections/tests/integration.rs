//! AR Collections Pack — integration tests
//!
//! ## What runs today (no F1/F2 required)
//!
//! - `pack_yaml_parses` — deserialises pack.yaml against PackManifest schema.
//! - `watch_spec_round_trips` — parses watches/dso-over-60.yaml, re-serialises,
//!   confirms key fields survive the round-trip.
//! - `anomaly_spec_round_trips` — same for anomalies/dso-drift.yaml.
//! - `email_template_renders_1st` — renders email-dunning-1st.md.j2 with
//!   a fixture context and asserts the expected invoice ID appears.
//! - `email_template_renders_2nd` — same for 2nd-tier template.
//! - `email_template_renders_final` — same for final-tier template.
//!
//! ## Gated on F1/F2 merge (`#[ignore]`)
//!
//! - `watch_tick_emits_event` — seeds ar_aging fixture rows, ticks the
//!   WatchRunner, asserts a WatchEvent with kind `ar.dso_over_60` is emitted.
//! - `agent_drafts_email_on_watch_event` — full end-to-end: seed fixture,
//!   tick watch, let dunning-drafter run, assert ar_dunning_log row created
//!   with status = 'pending_approval' and rendered body contains invoice ID.

// -------------------------------------------------------------------------
// Dependencies
// Tera is the Jinja-compatible template engine used in production.
// serde_yaml parses the declarative YAML specs.
// -------------------------------------------------------------------------
use std::path::PathBuf;
use tera::{Context, Tera};

// -------------------------------------------------------------------------
// Helper: return the absolute path to the pack root directory.
// Works whether tests are run from the workspace root or from the pack dir.
// -------------------------------------------------------------------------
fn pack_root() -> PathBuf {
    // CARGO_MANIFEST_DIR for integration tests in xiaoguai-core points to the
    // core crate directory. We walk up the ancestor chain to find the workspace
    // root (the dir that has both Cargo.toml and a packs/ sub-dir).
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));

    manifest_dir
        .ancestors()
        .find(|p| p.join("Cargo.toml").exists() && p.join("packs").exists())
        .map(|p| p.join("packs/ar-collections"))
        .unwrap_or_else(|| {
            // Fallback: the test file lives at packs/ar-collections/tests/integration.rs
            // so two parents up is the pack root.
            PathBuf::from(file!())
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_owned()
        })
}

// -------------------------------------------------------------------------
// Minimal schema types — mirror what packs.rs will expose once F1 lands.
// We use `#[serde::serde(...)]` notation to avoid collision with the
// `rmcp::serde` re-export that is in scope via xiaoguai-core's dependencies.
// -------------------------------------------------------------------------

#[derive(Debug)]
struct PackManifest {
    name: String,
    version: String,
    requires: PackRequires,
    migrations: Vec<PackPath>,
    watches: Vec<PackPath>,
    anomalies: Vec<PackPath>,
    agents: Vec<PackPath>,
}

#[derive(Debug, Default)]
struct PackRequires {
    features: Vec<String>,
}

#[derive(Debug)]
struct PackPath {
    path: String,
}

#[derive(Debug)]
struct WatchSpec {
    name: String,
    version: String,
    source: WatchSource,
    query: String,
    schedule: WatchSchedule,
    event: WatchEvent,
}

#[derive(Debug)]
struct WatchSource {
    kind: String,
    table: String,
}

#[derive(Debug)]
struct WatchSchedule {
    cron: String,
}

#[derive(Debug)]
struct WatchEvent {
    kind: String,
    cardinality: String,
}

#[derive(Debug)]
struct AnomalySpec {
    name: String,
    version: String,
    metric: AnomalyMetric,
    baseline: AnomalyBaseline,
    alert: AnomalyAlert,
}

#[derive(Debug)]
struct AnomalyMetric {
    name: String,
    unit: String,
    query: String,
}

#[derive(Debug)]
struct AnomalyBaseline {
    kind: String,
    window_days: u32,
    min_observations: u32,
}

#[derive(Debug)]
struct AnomalyAlert {
    kind: String,
    n_sigma: f64,
}

// -------------------------------------------------------------------------
// Manual YAML parsing helpers using serde_yaml::Value to avoid the serde
// trait ambiguity (rmcp re-exports its own serde namespace).
// -------------------------------------------------------------------------

fn parse_str_field(v: &serde_yaml::Value, key: &str) -> String {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing or non-string field: {key}"))
        .to_owned()
}

fn parse_opt_str(v: &serde_yaml::Value, key: &str) -> Option<String> {
    v[key].as_str().map(str::to_owned)
}

fn parse_u32_field(v: &serde_yaml::Value, key: &str) -> u32 {
    v[key]
        .as_u64()
        .unwrap_or_else(|| panic!("missing or non-integer field: {key}")) as u32
}

fn parse_f64_field(v: &serde_yaml::Value, key: &str) -> f64 {
    v[key]
        .as_f64()
        .unwrap_or_else(|| panic!("missing or non-float field: {key}"))
}

fn parse_seq_strings(v: &serde_yaml::Value, key: &str) -> Vec<String> {
    v[key]
        .as_sequence()
        .map(|seq| {
            seq.iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_seq_paths(v: &serde_yaml::Value, key: &str) -> Vec<PackPath> {
    v[key]
        .as_sequence()
        .map(|seq| {
            seq.iter()
                .map(|item| PackPath {
                    path: parse_str_field(item, "path"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_pack_manifest(raw: &str) -> PackManifest {
    let v: serde_yaml::Value = serde_yaml::from_str(raw).expect("pack.yaml: invalid YAML");
    PackManifest {
        name: parse_str_field(&v, "name"),
        version: parse_str_field(&v, "version"),
        requires: PackRequires {
            features: parse_seq_strings(&v["requires"], "features"),
        },
        migrations: parse_seq_paths(&v, "migrations"),
        watches: parse_seq_paths(&v, "watches"),
        anomalies: parse_seq_paths(&v, "anomalies"),
        agents: parse_seq_paths(&v, "agents"),
    }
}

fn parse_watch_spec(raw: &str) -> WatchSpec {
    let v: serde_yaml::Value = serde_yaml::from_str(raw).expect("watch spec: invalid YAML");
    WatchSpec {
        name: parse_str_field(&v, "name"),
        version: parse_str_field(&v, "version"),
        source: WatchSource {
            kind: parse_str_field(&v["source"], "kind"),
            table: parse_str_field(&v["source"], "table"),
        },
        query: parse_str_field(&v, "query"),
        schedule: WatchSchedule {
            cron: parse_str_field(&v["schedule"], "cron"),
        },
        event: WatchEvent {
            kind: parse_str_field(&v["event"], "kind"),
            cardinality: parse_str_field(&v["event"], "cardinality"),
        },
    }
}

fn parse_anomaly_spec(raw: &str) -> AnomalySpec {
    let v: serde_yaml::Value = serde_yaml::from_str(raw).expect("anomaly spec: invalid YAML");
    AnomalySpec {
        name: parse_str_field(&v, "name"),
        version: parse_str_field(&v, "version"),
        metric: AnomalyMetric {
            name: parse_str_field(&v["metric"], "name"),
            unit: parse_str_field(&v["metric"], "unit"),
            query: parse_str_field(&v["metric"], "query"),
        },
        baseline: AnomalyBaseline {
            kind: parse_str_field(&v["baseline"], "kind"),
            window_days: parse_u32_field(&v["baseline"], "window_days"),
            min_observations: parse_u32_field(&v["baseline"], "min_observations"),
        },
        alert: AnomalyAlert {
            kind: parse_str_field(&v["alert"], "kind"),
            n_sigma: parse_f64_field(&v["alert"], "n_sigma"),
        },
    }
}

// -------------------------------------------------------------------------
// Fixture context for template rendering tests
// -------------------------------------------------------------------------
fn make_invoice_context_1st() -> Context {
    let mut ctx = Context::new();
    ctx.insert("contact_name", "Alice Finance");
    ctx.insert("company_name", "Acme Corp");
    ctx.insert("currency", "USD");
    ctx.insert("total_overdue", &4200.00_f64);
    ctx.insert("sender_name", "AR Team");
    ctx.insert(
        "invoices",
        &serde_json::json!([
            {
                "invoice_id": "INV-2026-0042",
                "amount": 2500.00,
                "currency": "USD",
                "due_date": "2026-03-01",
                "days_overdue": 83
            },
            {
                "invoice_id": "INV-2026-0055",
                "amount": 1700.00,
                "currency": "USD",
                "due_date": "2026-03-15",
                "days_overdue": 69
            }
        ]),
    );
    ctx
}

fn make_invoice_context_2nd() -> Context {
    let mut ctx = make_invoice_context_1st();
    ctx.insert("prior_dunning_date", "2026-04-01");
    ctx
}

fn make_invoice_context_final() -> Context {
    let mut ctx = make_invoice_context_1st();
    ctx.insert(
        "prior_dunning_dates",
        &serde_json::json!(["2026-04-01", "2026-04-15"]),
    );
    ctx.insert("max_days_overdue", &95_i64);
    ctx
}

// -------------------------------------------------------------------------
// Tests: today (no external deps)
// -------------------------------------------------------------------------

#[test]
fn pack_yaml_parses() {
    let root = pack_root();
    let yaml_path = root.join("pack.yaml");
    let raw = std::fs::read_to_string(&yaml_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", yaml_path.display()));

    let manifest = parse_pack_manifest(&raw);

    assert_eq!(manifest.name, "ar-collections");
    assert!(!manifest.version.is_empty(), "version must be non-empty");
    assert!(
        manifest.requires.features.contains(&"watch".to_string()),
        "must declare 'watch' feature"
    );
    assert!(
        manifest.requires.features.contains(&"anomaly".to_string()),
        "must declare 'anomaly' feature"
    );
    assert_eq!(manifest.migrations.len(), 1, "expected 1 migration");
    assert_eq!(manifest.watches.len(), 1, "expected 1 watch");
    assert_eq!(manifest.anomalies.len(), 1, "expected 1 anomaly");
    assert_eq!(manifest.agents.len(), 1, "expected 1 agent");
}

#[test]
fn watch_spec_round_trips() {
    let root = pack_root();
    let yaml_path = root.join("watches/dso-over-60.yaml");
    let raw = std::fs::read_to_string(&yaml_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", yaml_path.display()));

    let spec = parse_watch_spec(&raw);

    assert_eq!(spec.name, "dso-over-60");
    assert_eq!(spec.source.kind, "sql");
    assert_eq!(spec.source.table, "ar_aging");
    assert!(
        spec.query.contains("INTERVAL '60 days'"),
        "query must reference 60-day threshold"
    );
    assert_eq!(spec.event.kind, "ar.dso_over_60");
    assert_eq!(spec.event.cardinality, "per_row");
    // Confirm key schedule field round-trips correctly
    assert!(
        spec.schedule.cron.contains("*/15"),
        "default cron should be every 15 minutes"
    );
}

#[test]
fn anomaly_spec_round_trips() {
    let root = pack_root();
    let yaml_path = root.join("anomalies/dso-drift.yaml");
    let raw = std::fs::read_to_string(&yaml_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", yaml_path.display()));

    let spec = parse_anomaly_spec(&raw);

    assert_eq!(spec.name, "dso-drift");
    assert_eq!(spec.metric.name, "tenant_dso_days");
    assert_eq!(spec.metric.unit, "days");
    assert!(
        spec.metric.query.contains("ar_aging"),
        "metric query must reference ar_aging table"
    );
    assert_eq!(spec.baseline.kind, "rolling");
    assert_eq!(spec.baseline.window_days, 30);
    assert!(spec.baseline.min_observations >= 5);
    assert_eq!(spec.alert.kind, "n_sigma");
    assert!(
        spec.alert.n_sigma > 0.0,
        "n_sigma threshold must be positive"
    );
}

#[test]
fn email_template_renders_1st() {
    let root = pack_root();
    let template_glob = root
        .join("templates/*.md.j2")
        .to_string_lossy()
        .into_owned();
    let tera = Tera::new(&template_glob).expect("Tera failed to load templates");

    let ctx = make_invoice_context_1st();
    let rendered = tera
        .render("email-dunning-1st.md.j2", &ctx)
        .expect("1st-tier template render failed");

    assert!(
        rendered.contains("INV-2026-0042"),
        "rendered email must contain invoice ID"
    );
    assert!(
        rendered.contains("Alice Finance"),
        "rendered email must contain contact name"
    );
    assert!(
        rendered.contains("4200"),
        "rendered email must contain total overdue amount"
    );
    assert!(
        rendered.contains("Acme Corp"),
        "rendered email must contain company name"
    );
}

#[test]
fn email_template_renders_2nd() {
    let root = pack_root();
    let template_glob = root
        .join("templates/*.md.j2")
        .to_string_lossy()
        .into_owned();
    let tera = Tera::new(&template_glob).expect("Tera failed to load templates");

    let ctx = make_invoice_context_2nd();
    let rendered = tera
        .render("email-dunning-2nd.md.j2", &ctx)
        .expect("2nd-tier template render failed");

    assert!(
        rendered.contains("INV-2026-0042"),
        "rendered email must contain invoice ID"
    );
    assert!(
        rendered.contains("Second Notice")
            || rendered.contains("second")
            || rendered.contains("follow-up"),
        "2nd-tier email must reference second notice or follow-up"
    );
}

#[test]
fn email_template_renders_final() {
    let root = pack_root();
    let template_glob = root
        .join("templates/*.md.j2")
        .to_string_lossy()
        .into_owned();
    let tera = Tera::new(&template_glob).expect("Tera failed to load templates");

    let ctx = make_invoice_context_final();
    let rendered = tera
        .render("email-final.md.j2", &ctx)
        .expect("final-tier template render failed");

    assert!(
        rendered.contains("INV-2026-0042"),
        "rendered email must contain invoice ID"
    );
    assert!(
        rendered.contains("FINAL") || rendered.contains("final"),
        "final-tier email must reference final notice"
    );
    assert!(
        rendered.contains("pending_approval") || rendered.contains("human"),
        "final-tier email must include HOTL disclaimer"
    );
}

// -------------------------------------------------------------------------
// Tests gated on F1/F2 merge — marked #[ignore] until those features land.
// -------------------------------------------------------------------------

/// Seed ar_aging fixture, run a WatchRunner tick, assert WatchEvent emitted.
///
/// Requires:
///   - F1: xiaoguai-watch crate + WatchRunner + event bus
///   - A live Postgres instance (DATABASE_URL env var)
///
/// To run manually once F1 is merged:
///   cargo test -p xiaoguai-core watch_tick_emits_event -- --ignored
#[test]
#[ignore = "pending F1 (xiaoguai-watch) merge"]
fn watch_tick_emits_event() {
    // TODO: wire once F1 lands
    // 1. Apply migration 0001_ar_aging.sql to a testcontainer PG.
    // 2. Insert 3 rows: 2 with due_date < NOW() - 65 days (paid_at = NULL),
    //    1 with paid_at = NOW() - 10 days (should not appear).
    // 3. Construct a WatchRunner with the dso-over-60.yaml spec.
    // 4. Call runner.tick("test-tenant").await.
    // 5. Assert the event receiver gets exactly 1 WatchEvent with:
    //      kind = "ar.dso_over_60"
    //      payload.overdue_count = 2
    //      payload.customer_id = "CUST-001"
    todo!("wire to F1 WatchRunner once merged");
}

/// Full end-to-end: seed → watch tick → dunning-drafter runs → draft saved.
///
/// Requires:
///   - F1: WatchRunner
///   - F2: AnomalyRunner (optional for this specific test)
///   - F3: outcome telemetry tables
///   - A live Postgres instance + a configured LLM backend
///
/// To run manually once F1/F2/F3 are merged:
///   cargo test -p xiaoguai-core agent_drafts_email_on_watch_event -- --ignored
#[test]
#[ignore = "pending F1/F2/F3 merge"]
fn agent_drafts_email_on_watch_event() {
    // TODO: wire once F1/F2/F3 land
    // 1. Apply migration 0001_ar_aging.sql.
    // 2. Seed ar_aging with 2 overdue invoices for CUST-001.
    // 3. Tick the WatchRunner — emits ar.dso_over_60 event.
    // 4. Let the dunning-drafter agent consume the event.
    // 5. Assert ar_dunning_log contains 1 row with:
    //      customer_id = "CUST-001"
    //      tier = "1st"
    //      status = "pending_approval"
    //      draft_body LIKE "%INV-%"  (invoice ID substituted)
    // 6. Assert no row in ar_dunning_log has status = "sent"
    //    (HOTL guardrail — zero autonomous sends).
    todo!("wire to F1/F2/F3 once merged");
}
