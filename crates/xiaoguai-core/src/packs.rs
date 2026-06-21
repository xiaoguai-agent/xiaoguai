//! Skill Pack loader — manifest parsing + path validation.
//!
//! Parses a `pack.yaml` manifest and validates that its declared
//! migration / watch / anomaly / agent paths exist on disk. Registration into
//! the live registries is a later, owner-gated phase (see
//! `docs/plans/2026-06-21-skill-pack-loader.md`); today this is parse + validate
//! only, used by `xiaoguai pack validate`.
//!
//! ## Status
//!
//! This module is **feature-gated** behind `cfg(feature = "packs")`.
//! It is a stub: the registration calls are no-ops until the watch and
//! anomaly registries land with F1 and F2 respectively.
//!
//! ## Wiring plan (once F1/F2 merge)
//!
//! ```text
//! PackLoader::load("packs/ar-collections/pack.yaml")
//!     → PackManifest::from_yaml(...)
//!     → apply_migrations(pool, &manifest.migrations)   // sqlx migrate
//!     → register_watches(watch_registry, &manifest.watches)
//!       // TODO: wire to xiaoguai-watch once it lands
//!     → register_anomalies(anomaly_registry, &manifest.anomalies)
//!       // TODO: wire to xiaoguai-anomaly once it lands
//!     → register_agents(agent_registry, &manifest.agents)
//!       // TODO: wire to xiaoguai-agent pack extension once it lands
//! ```
//!
//! ## Usage (today — parse + validate only)
//!
//! ```rust,ignore
//! # #[cfg(feature = "packs")]
//! # {
//! use xiaoguai_core::packs::PackLoader;
//!
//! let loader = PackLoader::new();
//! let manifest = loader.load("packs/ar-collections/pack.yaml").await?;
//! println!("Loaded pack: {} v{}", manifest.name, manifest.version);
//! # }
//! ```

// Module-gated via `#[cfg(feature = "packs")]` on `pub mod packs;` in lib.rs;
// no inner `#![cfg]` (would duplicate the attribute).
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
// Explicit re-import via `::serde` to avoid collision with `rmcp::serde`
// which is in scope through xiaoguai-core's rmcp dependency.
use ::serde::{Deserialize, Deserializer};

// ---------------------------------------------------------------------------
// Pack manifest schema
// ---------------------------------------------------------------------------

/// Top-level pack manifest parsed from `pack.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct PackManifest {
    /// Unique pack identifier (kebab-case, e.g. `ar-collections`).
    pub name: String,

    /// `SemVer` pack version.
    pub version: String,

    /// Human-readable description of the pack.
    #[serde(default)]
    pub description: String,

    /// Required xiaoguai platform features and minimum version.
    #[serde(default)]
    pub requires: PackRequires,

    /// Ordered list of SQL migration files to apply on install.
    #[serde(default)]
    pub migrations: Vec<PackPath>,

    /// Declarative watch specs to register.
    #[serde(default)]
    pub watches: Vec<PackPath>,

    /// Declarative anomaly specs to register.
    #[serde(default)]
    pub anomalies: Vec<PackPath>,

    /// Agent definitions to boot.
    #[serde(default)]
    pub agents: Vec<PackPath>,

    /// Dashboard layout definitions (admin-ui surface — not wired yet).
    #[serde(default)]
    pub dashboards: Vec<DashboardDef>,
}

/// Platform requirements declared by the pack.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PackRequires {
    /// Minimum xiaoguai version (semver requirement string).
    #[serde(default)]
    pub xiaoguai_version: String,

    /// Platform feature flags the pack depends on.
    /// Known values: `watch`, `anomaly`, `llm`, `outcome-telemetry`.
    #[serde(default)]
    pub features: Vec<String>,
}

/// A relative path reference within the pack directory.
///
/// Tolerant of two equivalent YAML idioms the `packs/*` manifests use
/// interchangeably — a bare string or a `{ path: ... }` mapping:
///
/// ```yaml
/// migrations:
///   - migrations/0001.sql      # bare string
///   - path: migrations/0002.sql # mapping
/// ```
///
/// Both deserialize to the same `PackPath`. (~45% of the shipped manifests use
/// the bare-string form; accepting it lets the loader validate them without a
/// schema-conversion pass — see `docs/plans/2026-06-21-skill-pack-loader.md` §4.)
#[derive(Debug, Clone)]
pub struct PackPath {
    pub path: String,
}

impl<'de> Deserialize<'de> for PackPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bare(String),
            Mapping { path: String },
        }
        let path = match Raw::deserialize(deserializer)? {
            Raw::Bare(path) | Raw::Mapping { path } => path,
        };
        Ok(PackPath { path })
    }
}

/// Minimal dashboard definition (wired by a future admin-ui surface).
#[derive(Debug, Clone, Deserialize)]
pub struct DashboardDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

// ---------------------------------------------------------------------------
// PackLoader
// ---------------------------------------------------------------------------

/// Loads and validates pack manifests.
///
/// Create one instance per process; it is stateless today (no caches).
pub struct PackLoader {
    /// Base directory for resolving relative paths inside manifests.
    /// Defaults to the current working directory.
    base_dir: PathBuf,
}

impl PackLoader {
    /// Create a `PackLoader` that resolves paths relative to `cwd`.
    pub fn new() -> Self {
        Self {
            base_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    /// Create a `PackLoader` that resolves paths relative to `base`.
    pub fn with_base(base: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base.into(),
        }
    }

    /// Parse and validate a `pack.yaml` manifest.
    ///
    /// Does **not** apply migrations or register watches/anomalies — those
    /// steps require the F1/F2 registries which are not yet merged.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The file cannot be read.
    /// - The YAML is malformed.
    /// - A declared migration, watch, or anomaly path does not exist on disk.
    pub async fn load(&self, manifest_path: impl AsRef<Path>) -> Result<PackManifest> {
        let manifest_path = self.base_dir.join(manifest_path.as_ref());
        let raw = tokio::fs::read_to_string(&manifest_path)
            .await
            .with_context(|| format!("read pack manifest: {}", manifest_path.display()))?;

        let manifest: PackManifest = serde_yaml::from_str(&raw)
            .with_context(|| format!("parse pack manifest: {}", manifest_path.display()))?;

        let pack_dir = manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_owned();

        Self::validate(&manifest, &pack_dir)
            .with_context(|| format!("validate pack '{}'", manifest.name))?;

        tracing::info!(
            pack = %manifest.name,
            version = %manifest.version,
            migrations = manifest.migrations.len(),
            watches = manifest.watches.len(),
            anomalies = manifest.anomalies.len(),
            agents = manifest.agents.len(),
            "pack manifest loaded"
        );

        Ok(manifest)
    }

    /// Validate that all referenced paths exist within the pack directory.
    fn validate(manifest: &PackManifest, pack_dir: &Path) -> Result<()> {
        for entry in &manifest.migrations {
            let p = pack_dir.join(&entry.path);
            anyhow::ensure!(p.exists(), "migration path does not exist: {}", p.display());
        }
        for entry in &manifest.watches {
            let p = pack_dir.join(&entry.path);
            anyhow::ensure!(
                p.exists(),
                "watch spec path does not exist: {}",
                p.display()
            );
        }
        for entry in &manifest.anomalies {
            let p = pack_dir.join(&entry.path);
            anyhow::ensure!(
                p.exists(),
                "anomaly spec path does not exist: {}",
                p.display()
            );
        }
        for entry in &manifest.agents {
            let p = pack_dir.join(&entry.path);
            anyhow::ensure!(
                p.exists(),
                "agent definition path does not exist: {}",
                p.display()
            );
        }
        Ok(())
    }
}

impl Default for PackLoader {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Registry stubs — to be wired once F1/F2 land
// ---------------------------------------------------------------------------

/// Placeholder watch registry.
///
/// TODO: wire to `xiaoguai_watch::WatchRegistry` once F1 lands.
pub struct WatchRegistry;

impl WatchRegistry {
    /// Register a watch spec from its YAML path.
    ///
    /// Currently a no-op stub; will call the real watch runner once F1 is
    /// merged.
    #[allow(unused_variables)]
    pub fn register(&self, spec_path: &Path) -> Result<()> {
        // TODO: wire to xiaoguai-watch once it lands
        //
        // let spec = WatchSpec::from_yaml(spec_path)?;
        // self.inner.register(spec)?;
        tracing::debug!(
            path = %spec_path.display(),
            "WatchRegistry::register — stub (pending F1 merge)"
        );
        Ok(())
    }
}

/// Placeholder anomaly registry.
///
/// TODO: wire to `xiaoguai_anomaly::AnomalyRegistry` once F2 lands.
pub struct AnomalyRegistry;

impl AnomalyRegistry {
    /// Register an anomaly spec from its YAML path.
    #[allow(unused_variables)]
    pub fn register(&self, spec_path: &Path) -> Result<()> {
        // TODO: wire to xiaoguai-anomaly once it lands
        tracing::debug!(
            path = %spec_path.display(),
            "AnomalyRegistry::register — stub (pending F2 merge)"
        );
        Ok(())
    }
}

/// Register all watches and anomalies declared in the manifest into their
/// respective stub registries.
///
/// This is called by the server boot path when `cfg(feature = "packs")`.
/// It is a no-op today; the registries are stubs until F1/F2 merge.
pub fn register_pack(
    manifest: &PackManifest,
    pack_dir: &Path,
    watches: &WatchRegistry,
    anomalies: &AnomalyRegistry,
) -> Result<()> {
    for entry in &manifest.watches {
        watches
            .register(&pack_dir.join(&entry.path))
            .with_context(|| format!("register watch '{}'", entry.path))?;
    }
    for entry in &manifest.anomalies {
        anomalies
            .register(&pack_dir.join(&entry.path))
            .with_context(|| format!("register anomaly '{}'", entry.path))?;
    }
    tracing::info!(
        pack = %manifest.name,
        watches = manifest.watches.len(),
        anomalies = manifest.anomalies.len(),
        "pack registered (stub registries — pending F1/F2 merge)"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::TempDir;

    fn minimal_manifest_yaml() -> &'static str {
        r#"
name: test-pack
version: "0.1.0"
description: "Unit test pack"
requires:
  features:
    - watch
    - llm
migrations:
  - path: migrations/0001_test.sql
watches:
  - path: watches/test-watch.yaml
anomalies:
  - path: anomalies/test-anomaly.yaml
agents:
  - path: agents/test-agent.yaml
"#
    }

    fn write_stub(dir: &Path, rel: &str) {
        let full = dir.join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&full).unwrap();
        writeln!(f, "# stub").unwrap();
    }

    fn make_pack_dir() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path();
        write_stub(p, "migrations/0001_test.sql");
        write_stub(p, "watches/test-watch.yaml");
        write_stub(p, "anomalies/test-anomaly.yaml");
        write_stub(p, "agents/test-agent.yaml");
        tmp
    }

    #[test]
    fn manifest_parses_correctly() {
        let manifest: PackManifest =
            serde_yaml::from_str(minimal_manifest_yaml()).expect("parse failed");
        assert_eq!(manifest.name, "test-pack");
        assert_eq!(manifest.version, "0.1.0");
        assert!(manifest.requires.features.contains(&"watch".to_string()));
        assert_eq!(manifest.migrations.len(), 1);
        assert_eq!(manifest.watches.len(), 1);
    }

    #[tokio::test]
    async fn loader_validates_existing_paths() {
        let tmp = make_pack_dir();
        let manifest_path = tmp.path().join("pack.yaml");
        std::fs::write(&manifest_path, minimal_manifest_yaml()).unwrap();

        let loader = PackLoader::with_base(tmp.path());
        let result = loader.load("pack.yaml").await;
        assert!(result.is_ok(), "validation should pass: {result:?}");
    }

    #[tokio::test]
    async fn loader_rejects_missing_migration() {
        let tmp = TempDir::new().unwrap();
        // Write manifest referencing a migration file that does not exist.
        std::fs::write(
            tmp.path().join("pack.yaml"),
            r#"
name: bad-pack
version: "0.1.0"
migrations:
  - path: migrations/missing.sql
"#,
        )
        .unwrap();

        let loader = PackLoader::with_base(tmp.path());
        let result = loader.load("pack.yaml").await;
        assert!(
            result.is_err(),
            "should fail when migration path is missing"
        );
        let err = result.unwrap_err();
        // anyhow error chains: check every layer for our sentinel string.
        let full = format!("{err:#}");
        assert!(
            full.contains("migration path does not exist") || full.contains("missing.sql"),
            "error chain should mention the missing migration path, got: {full}"
        );
    }

    #[test]
    fn register_pack_stub_does_not_panic() {
        let manifest: PackManifest = serde_yaml::from_str(minimal_manifest_yaml()).expect("parse");
        let watches = WatchRegistry;
        let anomalies = AnomalyRegistry;
        // Paths don't exist — that's fine for stub registration.
        let _ = register_pack(&manifest, Path::new("/tmp/fake"), &watches, &anomalies);
    }
}
