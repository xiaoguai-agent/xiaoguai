//! Expert enablement prerequisites (`GET /v1/experts`).
//!
//! An "expert" is a curated persona that only becomes *selectable* once its
//! prerequisites are satisfied — the operator must first install the required
//! skills / configure the required MCP servers. This endpoint returns the
//! static prerequisite catalog joined with live readiness so the chat-ui can
//! gate selection and show exactly what's missing.
//!
//! Two requirement kinds:
//!   - `mcp`     — a marketplace slug; satisfied when a live `mcp_servers` row
//!                 exists for it (matched by the installed server's name).
//!   - `package` — a host binary/library; satisfied when its `probe` command
//!                 resolves on `PATH` (`command -v <probe>`), best-effort.
//!
//! A `required` GROUP is satisfied when ANY of its items is (an OR-set — e.g.
//! "monitor OR aiops"). An expert is `ready` when EVERY required group is
//! satisfied. `optional` slugs are add-ons the operator installs later.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::marketplace::marketplace_entries;
use crate::state::AppState;

const CATALOG_JSON: &str = include_str!("../catalog/expert_prerequisites.json");

/// Wall-clock ceiling for a single `command -v` package probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

// ── Static catalog (matches expert_prerequisites.json) ──────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct ExpertCatalog {
    pub version: u32,
    #[serde(default)]
    pub offline_hint: Option<String>,
    #[serde(default)]
    pub offline_hint_en: Option<String>,
    pub experts: Vec<ExpertBlueprint>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExpertBlueprint {
    pub key: String,
    /// Persona display name this blueprint gates (see `persona_seed`).
    pub persona_name: String,
    pub name: String,
    #[serde(default)]
    pub name_zh: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub summary_zh: Option<String>,
    pub required: Vec<RequirementGroup>,
    #[serde(default)]
    pub optional: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RequirementGroup {
    pub label: String,
    #[serde(default)]
    pub label_zh: Option<String>,
    pub any_of: Vec<RequirementItem>,
}

/// One prerequisite. `mcp` items carry `slug`; `package` items carry
/// `id` + `probe` (+ an `install` hint shown to the operator).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RequirementItem {
    Mcp {
        slug: String,
    },
    Package {
        id: String,
        probe: String,
        #[serde(default)]
        install: Option<String>,
    },
}

pub fn expert_catalog() -> &'static ExpertCatalog {
    static CATALOG: OnceLock<ExpertCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON).expect("expert_prerequisites.json is valid")
    })
}

// ── Computed readiness (the response) ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExpertReadinessResponse {
    pub version: u32,
    pub offline_hint: Option<String>,
    pub offline_hint_en: Option<String>,
    pub experts: Vec<ExpertReadiness>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExpertReadiness {
    pub key: String,
    pub persona_name: String,
    pub name: String,
    pub name_zh: Option<String>,
    pub summary: Option<String>,
    pub summary_zh: Option<String>,
    /// The gate: `true` only when every required group is satisfied.
    pub ready: bool,
    pub required: Vec<GroupReadiness>,
    pub optional: Vec<OptionalItem>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GroupReadiness {
    pub label: String,
    pub label_zh: Option<String>,
    pub satisfied: bool,
    pub any_of: Vec<ItemReadiness>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ItemReadiness {
    /// `"mcp"` or `"package"`.
    pub kind: String,
    /// mcp slug or package id.
    pub id: String,
    /// Human label (marketplace name for mcp; package id for package).
    pub label: String,
    pub satisfied: bool,
    /// Install hint (package) or the marketplace slug to install (mcp).
    pub install: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OptionalItem {
    pub slug: String,
    pub name: String,
    pub name_zh: Option<String>,
    pub installed: bool,
}

/// Pure readiness computation — no I/O. `installed_mcp_slugs` are the
/// marketplace slugs with a live server row; `installed_packages` are the
/// package ids whose probe resolved. Kept side-effect-free so it is trivially
/// unit-testable; the handler gathers the two sets then calls this.
#[must_use]
pub fn compute_readiness(
    installed_mcp_slugs: &HashSet<String>,
    installed_packages: &HashSet<String>,
) -> ExpertReadinessResponse {
    let catalog = expert_catalog();
    let market = marketplace_entries();

    let experts = catalog
        .experts
        .iter()
        .map(|bp| {
            let required: Vec<GroupReadiness> = bp
                .required
                .iter()
                .map(|g| {
                    let any_of: Vec<ItemReadiness> = g
                        .any_of
                        .iter()
                        .map(|item| item_readiness(item, installed_mcp_slugs, installed_packages))
                        .collect();
                    GroupReadiness {
                        label: g.label.clone(),
                        label_zh: g.label_zh.clone(),
                        satisfied: any_of.iter().any(|i| i.satisfied),
                        any_of,
                    }
                })
                .collect();

            let optional: Vec<OptionalItem> = bp
                .optional
                .iter()
                .map(|slug| {
                    let entry = market.iter().find(|e| &e.slug == slug);
                    OptionalItem {
                        slug: slug.clone(),
                        name: entry.map_or_else(|| slug.clone(), |e| e.name.clone()),
                        name_zh: entry.and_then(|e| e.name_zh.clone()),
                        installed: installed_mcp_slugs.contains(slug),
                    }
                })
                .collect();

            ExpertReadiness {
                key: bp.key.clone(),
                persona_name: bp.persona_name.clone(),
                name: bp.name.clone(),
                name_zh: bp.name_zh.clone(),
                summary: bp.summary.clone(),
                summary_zh: bp.summary_zh.clone(),
                ready: required.iter().all(|g| g.satisfied),
                required,
                optional,
            }
        })
        .collect();

    ExpertReadinessResponse {
        version: catalog.version,
        offline_hint: catalog.offline_hint.clone(),
        offline_hint_en: catalog.offline_hint_en.clone(),
        experts,
    }
}

fn item_readiness(
    item: &RequirementItem,
    installed_mcp_slugs: &HashSet<String>,
    installed_packages: &HashSet<String>,
) -> ItemReadiness {
    match item {
        RequirementItem::Mcp { slug } => {
            let entry = marketplace_entries().iter().find(|e| &e.slug == slug);
            ItemReadiness {
                kind: "mcp".into(),
                id: slug.clone(),
                label: entry.map_or_else(|| slug.clone(), |e| e.name.clone()),
                satisfied: installed_mcp_slugs.contains(slug),
                install: Some(slug.clone()),
            }
        }
        RequirementItem::Package {
            id,
            probe: _,
            install,
        } => ItemReadiness {
            kind: "package".into(),
            id: id.clone(),
            label: id.clone(),
            satisfied: installed_packages.contains(id),
            install: install.clone(),
        },
    }
}

// ── HTTP handler ────────────────────────────────────────────────────────────

/// `GET /v1/experts` — the prerequisite catalog joined with live readiness.
///
/// # Errors
/// Returns `ServiceUnavailable` when the MCP server repository is not wired.
pub async fn list_experts(
    State(state): State<AppState>,
) -> ApiResult<Json<ExpertReadinessResponse>> {
    let repo = state
        .mcp_servers
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("MCP server store not available".into()))?;

    // Installed MCP slugs: a server row's name equals the marketplace entry's
    // name on install, so resolve names → slugs via the catalog.
    let servers = repo.list().await.map_err(ApiError::from)?;
    let installed_names: HashSet<String> = servers.into_iter().map(|s| s.name).collect();
    let installed_mcp_slugs: HashSet<String> = marketplace_entries()
        .iter()
        .filter(|e| installed_names.contains(&e.name))
        .map(|e| e.slug.clone())
        .collect();

    // Probe every distinct package prerequisite once (best-effort).
    let mut probes: HashSet<(String, String)> = HashSet::new();
    for bp in &expert_catalog().experts {
        for g in &bp.required {
            for item in &g.any_of {
                if let RequirementItem::Package { id, probe, .. } = item {
                    probes.insert((id.clone(), probe.clone()));
                }
            }
        }
    }
    let mut installed_packages: HashSet<String> = HashSet::new();
    for (id, probe) in probes {
        if probe_command_exists(&probe).await {
            installed_packages.insert(id);
        }
    }

    Ok(Json(compute_readiness(
        &installed_mcp_slugs,
        &installed_packages,
    )))
}

/// Best-effort `command -v <probe>` on the host. Any spawn/timeout/non-zero
/// exit → `false` (treated as not installed). The probe name is a fixed
/// catalog value, never user input, but it is passed as a single `sh -c`
/// argument regardless.
async fn probe_command_exists(probe: &str) -> bool {
    use tokio::process::Command;
    let fut = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {probe}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match tokio::time::timeout(PROBE_TIMEOUT, fut).await {
        Ok(Ok(status)) => status.success(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slugs(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn expert<'a>(resp: &'a ExpertReadinessResponse, key: &str) -> &'a ExpertReadiness {
        resp.experts
            .iter()
            .find(|e| e.key == key)
            .expect("expert present")
    }

    #[test]
    fn catalog_parses_and_names_are_present() {
        let c = expert_catalog();
        assert!(c.experts.len() >= 3);
        for bp in &c.experts {
            assert!(!bp.persona_name.is_empty());
            assert!(!bp.required.is_empty());
        }
    }

    #[test]
    fn vmware_ops_needs_policy_and_a_capability_server() {
        // Only aiops installed, no policy package → NOT ready (policy group unmet).
        let r = compute_readiness(&slugs(&["vmware-aiops"]), &slugs(&[]));
        let vm = expert(&r, "vmware-ops");
        assert!(!vm.ready, "missing the policy package must block readiness");
        // The capability group IS satisfied (aiops present)...
        let cap = vm
            .required
            .iter()
            .find(|g| g.any_of.iter().any(|i| i.id == "vmware-aiops"))
            .unwrap();
        assert!(cap.satisfied);
        // ...but the policy group is not.
        let pol = vm
            .required
            .iter()
            .find(|g| g.any_of.iter().any(|i| i.id == "vmware-policy"))
            .unwrap();
        assert!(!pol.satisfied);
    }

    #[test]
    fn vmware_ops_ready_with_policy_plus_monitor() {
        let r = compute_readiness(&slugs(&["vmware-monitor"]), &slugs(&["vmware-policy"]));
        assert!(expert(&r, "vmware-ops").ready);
    }

    #[test]
    fn capability_group_is_an_or_set() {
        // aiops alone satisfies the capability group (monitor OR aiops).
        let r = compute_readiness(&slugs(&["vmware-aiops"]), &slugs(&["vmware-policy"]));
        assert!(expert(&r, "vmware-ops").ready);
    }

    #[test]
    fn policy_alone_is_not_enough() {
        let r = compute_readiness(&slugs(&[]), &slugs(&["vmware-policy"]));
        assert!(
            !expert(&r, "vmware-ops").ready,
            "no capability server → not ready"
        );
    }

    #[test]
    fn optional_installed_flag_reflects_state() {
        let r = compute_readiness(
            &slugs(&["vmware-monitor", "vmware-storage"]),
            &slugs(&["vmware-policy"]),
        );
        let vm = expert(&r, "vmware-ops");
        let storage = vm
            .optional
            .iter()
            .find(|o| o.slug == "vmware-storage")
            .unwrap();
        assert!(storage.installed);
        let vks = vm.optional.iter().find(|o| o.slug == "vmware-vks").unwrap();
        assert!(!vks.installed);
    }

    #[test]
    fn data_analyst_needs_a_sql_source_only() {
        // No packages at all, but sqlite installed → ready (no package req).
        let r = compute_readiness(&slugs(&["sqlite"]), &slugs(&[]));
        assert!(expert(&r, "data-analyst").ready);
        let r2 = compute_readiness(&slugs(&[]), &slugs(&[]));
        assert!(!expert(&r2, "data-analyst").ready);
    }
}
