//! `xiaoguai pack validate <dir>` — parse + validate a skill-pack manifest.
//!
//! Phase 1 of the pack loader (`docs/plans/2026-06-21-skill-pack-loader.md`):
//! parse `pack.yaml` via the canonical `xiaoguai_core::packs::PackLoader`,
//! confirm every declared migration / watch / anomaly / agent path exists,
//! check `requires.features` against the features this build knows about, and
//! report what *would* be registered. Strictly **read-only** — no migrations,
//! no registration, no side effects (the actual registration is a later phase,
//! gated on owner decisions — see the design doc §3).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use xiaoguai_core::pack_agents::{plan_pack_agents, PackAgentPlan};
use xiaoguai_core::packs::{PackLoader, PackManifest};

/// Platform features this build recognises in a pack's `requires.features`.
/// Phase 1 only *reports* unrecognised features — it does not gate on them.
const KNOWN_FEATURES: &[&str] = &[
    "watch",
    "anomaly",
    "llm",
    "outcome-telemetry",
    "scheduler",
    "rag",
    "memory",
];

/// Top-level manifest keys that `PackManifest` models. Anything else in the
/// YAML is parsed-but-dropped — Phase 1 surfaces these so pack authors aren't
/// surprised by silently-ignored fields (the `packs/*` manifests are not
/// schema-uniform; see the design doc §1).
const KNOWN_KEYS: &[&str] = &[
    "name",
    "version",
    "description",
    "requires",
    "migrations",
    "watches",
    "anomalies",
    "agents",
    "lead_agent",
    "sources",
    "outputs",
    "dashboards",
    // Conventional metadata the loader intentionally ignores — every shipped
    // pack carries these, so listing them keeps the "unknown key" warning
    // meaningful: it then fires only on genuinely-unmodeled *structural* keys
    // (e.g. `sources` / `outputs` / `depends` in the divergent manifests).
    "author",
    "license",
    "schema",
];

/// Parse + validate the pack manifest at `dir` (a directory containing
/// `pack.yaml`, or the `pack.yaml` file itself). Returns a human-readable
/// report on success.
///
/// # Errors
/// Returns an error when the manifest is missing, unparseable, or declares a
/// migration / watch / anomaly / agent path that does not exist on disk —
/// i.e. the pack would fail to load. Unknown keys / features are warnings, not
/// errors (a structurally valid pack still loads).
pub async fn validate(dir: &Path) -> Result<String> {
    let (base, rel) = resolve_manifest(dir)?;

    let loader = PackLoader::with_base(&base);
    let manifest = loader
        .load(&rel)
        .await
        .with_context(|| format!("validate pack at {}", dir.display()))?;

    let manifest_path = base.join(&rel);
    let pack_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let unknown_keys = unknown_top_level_keys(&manifest_path).await?;
    let unknown_features = unrecognised_features(&manifest);
    let missing_adapters = missing_adapter_files(&manifest, pack_dir);
    let agent_plan = plan_pack_agents(&manifest, pack_dir).await;

    Ok(render_report(
        &manifest,
        &unknown_keys,
        &unknown_features,
        &missing_adapters,
        &agent_plan,
    ))
}

/// Install the pack at `dir`: validate it, then record it as **enabled** in
/// `installed_skill_packs` (with its absolute on-disk path stored in the row's
/// `config` JSON) so the next `serve` boot wires its anomaly specs as scheduled
/// jobs. Idempotent on the pack slug — re-installing updates version + config.
///
/// # Errors
/// Returns an error when the pack fails to validate (see [`validate`]) or the
/// store write fails.
pub async fn install(pool: &sqlx::SqlitePool, dir: &Path) -> Result<String> {
    // Validate first — a pack that wouldn't load must not be recorded.
    let report = validate(dir).await?;

    let (base, rel) = resolve_manifest(dir)?;
    let manifest = PackLoader::with_base(&base).load(&rel).await?;
    let pack_dir = base.canonicalize().unwrap_or_else(|_| base.clone());
    let config = serde_json::json!({
        "enabled": true,
        "pack_dir": pack_dir.to_string_lossy(),
    })
    .to_string();

    sqlx::query(
        "INSERT INTO installed_skill_packs (id, pack_slug, version, config) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(pack_slug) DO UPDATE SET \
            version = excluded.version, config = excluded.config",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&manifest.name)
    .bind(&manifest.version)
    .bind(&config)
    .execute(pool)
    .await
    .context("record installed pack")?;

    Ok(format!(
        "{}\n✓ installed '{}' v{} (enabled) — its {} anomaly spec(s) wire on the next `serve` boot",
        report.trim_end(),
        manifest.name,
        manifest.version,
        manifest.anomalies.len()
    ))
}

/// Declared `sources` / `outputs` adapter paths whose files don't exist on disk.
///
/// Soft signal, not a hard error: many shipped packs are scaffold — they
/// declare inbound sources + output adapters whose YAML files were never
/// written. `PackLoader::load` deliberately does NOT validate these (only
/// migrations/watches/anomalies/agents are hard-checked), so `pack validate`
/// surfaces the gap as a warning while still passing the pack.
fn missing_adapter_files(manifest: &PackManifest, pack_dir: &Path) -> Vec<String> {
    manifest
        .sources
        .iter()
        .chain(manifest.outputs.iter())
        .filter(|p| !pack_dir.join(&p.path).exists())
        .map(|p| p.path.clone())
        .collect()
}

/// Returns `true` when `path` is a single pack — a `pack.yaml` file, or a
/// directory that directly contains one — rather than a parent directory of
/// many packs.
#[must_use]
pub fn is_single_pack(path: &Path) -> bool {
    path.is_file() || path.join("pack.yaml").exists()
}

/// Combined outcome of validating every pack under a parent directory.
pub struct BatchOutcome {
    /// Multi-line report: one `✓`/`✗` line per pack, then a summary.
    pub report: String,
    /// How many packs failed to validate.
    pub failed: usize,
    /// How many packs were checked.
    pub total: usize,
}

/// Validate every immediate `<parent>/<name>/pack.yaml` and return a combined
/// report. Each pack is checked independently — one failure does not abort the
/// rest — so a CI gate surfaces every problem in a single pass.
///
/// # Errors
/// Errors only when `parent` cannot be read or holds no packs; a pack that
/// fails to validate is recorded in the report (and `failed`), not propagated.
pub async fn validate_all(parent: &Path) -> Result<BatchOutcome> {
    use std::fmt::Write as _;

    let mut packs: Vec<PathBuf> = Vec::new();
    let mut entries = tokio::fs::read_dir(parent)
        .await
        .with_context(|| format!("read packs directory {}", parent.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        let p = entry.path();
        if p.join("pack.yaml").exists() {
            packs.push(p);
        }
    }
    packs.sort();
    anyhow::ensure!(
        !packs.is_empty(),
        "no packs found under {} (expected <subdir>/pack.yaml)",
        parent.display()
    );

    let mut report = String::new();
    let mut failed = 0;
    for pack in &packs {
        let name = pack.file_name().map_or_else(
            || pack.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        match validate(pack).await {
            Ok(_) => {
                let _ = writeln!(report, "✓ {name}");
            }
            Err(e) => {
                failed += 1;
                let reason = format!("{e:#}").replace('\n', "; ");
                let _ = writeln!(report, "✗ {name}: {reason}");
            }
        }
    }
    let total = packs.len();
    let _ = writeln!(report, "\n{}/{total} pack(s) valid", total - failed);
    Ok(BatchOutcome {
        report,
        failed,
        total,
    })
}

/// Resolve `dir` to `(base_dir, manifest_rel_path)`, accepting either a
/// directory holding `pack.yaml` or the `pack.yaml` file directly.
fn resolve_manifest(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    if dir.is_file() {
        let base = dir
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let file = dir
            .file_name()
            .map_or_else(|| PathBuf::from("pack.yaml"), PathBuf::from);
        Ok((base, file))
    } else {
        let manifest = dir.join("pack.yaml");
        anyhow::ensure!(
            manifest.exists(),
            "no pack.yaml found in {} (pass the pack directory or its pack.yaml)",
            dir.display()
        );
        Ok((dir.to_path_buf(), PathBuf::from("pack.yaml")))
    }
}

/// Features the pack requests that this build does not recognise.
fn unrecognised_features(manifest: &PackManifest) -> Vec<String> {
    manifest
        .requires
        .features
        .iter()
        .filter(|f| !KNOWN_FEATURES.contains(&f.as_str()))
        .cloned()
        .collect()
}

/// Top-level YAML keys present in the manifest that `PackManifest` does not
/// model (parsed but dropped on load).
async fn unknown_top_level_keys(manifest_path: &Path) -> Result<Vec<String>> {
    let raw = tokio::fs::read_to_string(manifest_path)
        .await
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let value: serde_yaml::Value =
        serde_yaml::from_str(&raw).context("re-parse manifest for key audit")?;

    let mut extra = Vec::new();
    if let serde_yaml::Value::Mapping(map) = value {
        for key in map.keys() {
            if let Some(s) = key.as_str() {
                if !KNOWN_KEYS.contains(&s) {
                    extra.push(s.to_string());
                }
            }
        }
    }
    extra.sort();
    Ok(extra)
}

fn render_report(
    manifest: &PackManifest,
    unknown_keys: &[String],
    unknown_features: &[String],
    missing_adapters: &[String],
    agent_plan: &PackAgentPlan,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "✓ pack '{}' v{} — manifest valid",
        manifest.name, manifest.version
    );
    if !manifest.description.is_empty() {
        let _ = writeln!(out, "  {}", manifest.description.trim());
    }
    let _ = writeln!(
        out,
        "  would register: {} migration(s), {} watch(es), {} anomaly(ies), {} agent(s); \
         declares {} source(s) + {} output(s)",
        manifest.migrations.len(),
        manifest.watches.len(),
        manifest.anomalies.len(),
        manifest.agents.len(),
        manifest.sources.len(),
        manifest.outputs.len()
    );
    if !manifest.requires.xiaoguai_version.is_empty() {
        let _ = writeln!(
            out,
            "  requires xiaoguai {}",
            manifest.requires.xiaoguai_version
        );
    }
    if !manifest.requires.features.is_empty() {
        let _ = writeln!(
            out,
            "  requires features: {}",
            manifest.requires.features.join(", ")
        );
    }
    if !unknown_features.is_empty() {
        let _ = writeln!(
            out,
            "  ⚠ unrecognised feature(s) not provided by this build: {}",
            unknown_features.join(", ")
        );
    }
    if !unknown_keys.is_empty() {
        let _ = writeln!(
            out,
            "  ⚠ ignored unknown manifest key(s) (parsed but not loaded): {}",
            unknown_keys.join(", ")
        );
    }
    if !missing_adapters.is_empty() {
        let _ = writeln!(
            out,
            "  ⚠ {} declared source/output file(s) not found — pack is scaffold: {}",
            missing_adapters.len(),
            missing_adapters.join(", ")
        );
    }
    render_agent_plan(&mut out, agent_plan);
    out
}

/// Append the Phase-4 agent-team activation plan to a validate report: the
/// team that would be created, and an honest list of what won't activate in v1
/// (event-triggered workers + inline tool bodies).
fn render_agent_plan(out: &mut String, plan: &PackAgentPlan) {
    use std::fmt::Write as _;
    if let Some(team) = &plan.team {
        let members = if team.members.is_empty() {
            " (lead only)".to_string()
        } else {
            format!(", members: {}", team.members.join(", "))
        };
        let _ = writeln!(
            out,
            "  agent team (Phase 4): would activate '{}' — lead '{}'{} → {} persona(s)",
            team.name,
            team.lead,
            members,
            plan.personas.len()
        );
    }
    if !plan.skipped_reactive.is_empty() {
        let _ = writeln!(
            out,
            "  ⚠ {} event-triggered agent(s) not activated in v1 (Phase 4b): {}",
            plan.skipped_reactive.len(),
            plan.skipped_reactive.join(", ")
        );
    }
    for w in &plan.warnings {
        let _ = writeln!(out, "  ⚠ {w}");
    }
}
