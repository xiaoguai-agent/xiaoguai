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
    let unknown_keys = unknown_top_level_keys(&manifest_path).await?;
    let unknown_features = unrecognised_features(&manifest);

    Ok(render_report(&manifest, &unknown_keys, &unknown_features))
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
        "  would register: {} migration(s), {} watch(es), {} anomaly(ies), {} agent(s)",
        manifest.migrations.len(),
        manifest.watches.len(),
        manifest.anomalies.len(),
        manifest.agents.len()
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
    out
}
