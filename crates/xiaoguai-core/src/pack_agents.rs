//! Phase 4a — parse a pack's `agents[]` and **plan** how they map onto the
//! persona / team execution model, **without touching the serving path**.
//!
//! This is the pure, side-effect-free core of Phase 4 (see
//! `docs/plans/2026-06-25-skill-pack-loader-phase4.md`). It reads each agent
//! YAML, classifies it (conversational team member vs reactive worker), and
//! produces a [`PackAgentPlan`] describing the personas + team a later slice
//! (4b) will upsert, plus an honest list of what will **not** activate in v1
//! (reactive `triggers[]` workers and inline `tools[]` bodies). `pack validate`
//! renders the plan so authors are never surprised.
//!
//! A pack agent is *richer* than a [`xiaoguai_personas::Persona`] — it carries
//! `triggers[]`, inline `tools[]`, `hotl`, and `on_failure`. v1 maps only the
//! conversational layer (`system_prompt` + model); the rest is parsed-and-noted,
//! not executed.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::packs::PackManifest;

/// One pack agent, parsed from its YAML. Only the fields Phase 4 maps are
/// modelled; everything else (`temperature`, `hotl`, `on_failure`, tool query
/// bodies, …) is parsed-and-ignored — serde drops unknown fields.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentDef {
    /// Unique-within-pack agent name.
    pub name: String,
    /// Optional role hint (`worker`, `lead`, …) — informational in v1.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: String,
    /// Event / inbound subscriptions. **Presence ⇒ a reactive worker** (Phase
    /// 4b), not a conversational team member. Captured loosely — the shape
    /// varies across packs and v1 only needs the count.
    #[serde(default)]
    pub triggers: Vec<serde_yaml::Value>,
    #[serde(default)]
    pub llm: AgentLlm,
    #[serde(default)]
    pub system_prompt: String,
    /// Inline tool definitions. v1 does **not** execute these (the agent uses
    /// the platform toolbox); their names are surfaced so the author knows.
    #[serde(default)]
    pub tools: Vec<AgentTool>,
}

/// The `llm:` block of an agent YAML (only the model matters for mapping).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentLlm {
    #[serde(default)]
    pub model: String,
}

/// One inline tool an agent declares (only its name/kind matter in v1).
#[derive(Debug, Clone, Deserialize)]
pub struct AgentTool {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
}

/// Whether an agent is a conversational team member (activated in v1) or a
/// reactive, event-triggered worker (deferred to Phase 4b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    /// No `triggers[]` — runs as an orchestrate team member.
    Conversational,
    /// Has `triggers[]` — event-driven; not a session team member.
    Reactive,
}

/// Classify an agent by the presence of event triggers.
#[must_use]
pub fn classify(agent: &AgentDef) -> AgentRole {
    if agent.triggers.is_empty() {
        AgentRole::Conversational
    } else {
        AgentRole::Reactive
    }
}

/// A persona derived from a conversational pack agent — neutral data, no
/// `id`/timestamps (Phase 4b assigns those when it upserts the real
/// [`xiaoguai_personas::Persona`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedPersona {
    /// Namespaced `"{pack_slug}/{agent_name}"` so pack personas never collide
    /// with operator-authored ones.
    pub name: String,
    pub system_prompt: String,
    /// Empty ⇒ use the session / global default model.
    pub model: String,
    /// Inline pack tools the agent declared — **not** activated in v1; kept for
    /// the validate report and Phase 4b.
    pub declared_tools: Vec<String>,
}

/// A team derived from a pack's conversational agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedTeam {
    /// Team display name (the pack slug).
    pub name: String,
    pub description: String,
    /// Namespaced name of the lead persona.
    pub lead: String,
    /// Namespaced names of the member personas (excludes the lead).
    pub members: Vec<String>,
}

/// The full activation plan for a pack's `agents[]` — what Phase 4b will upsert
/// and what `pack validate` reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackAgentPlan {
    /// One per conversational agent (the lead is also here).
    pub personas: Vec<DerivedPersona>,
    /// The derived team — `None` when the pack has no conversational agents.
    pub team: Option<DerivedTeam>,
    /// Reactive agents skipped in v1 (Phase 4b), by namespaced name.
    pub skipped_reactive: Vec<String>,
    /// Human-readable notes for the validate report (dropped tool bodies,
    /// unparseable agents, …).
    pub warnings: Vec<String>,
}

/// Parse one agent YAML file into an [`AgentDef`].
///
/// # Errors
/// Returns an error if the file cannot be read or the YAML is malformed.
pub async fn parse_agent_def(path: &Path) -> Result<AgentDef> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read agent def: {}", path.display()))?;
    let def: AgentDef = serde_yaml::from_str(&raw)
        .with_context(|| format!("parse agent def: {}", path.display()))?;
    Ok(def)
}

/// Plan how a pack's `agents[]` map onto personas + a team. **Pure analysis** —
/// reads the agent YAMLs but mutates nothing. `pack_dir` resolves the relative
/// `agents[].path` refs.
///
/// Tolerant by design: an agent that fails to parse becomes a warning and is
/// skipped rather than failing the whole plan (the shipped corpus is scaffold-
/// heavy). The function therefore never errors on a single bad agent.
pub async fn plan_pack_agents(manifest: &PackManifest, pack_dir: &Path) -> PackAgentPlan {
    let slug = &manifest.name;
    let mut plan = PackAgentPlan::default();
    let mut conversational_names: Vec<String> = Vec::new();

    for entry in &manifest.agents {
        let path = pack_dir.join(&entry.path);
        let def = match parse_agent_def(&path).await {
            Ok(def) => def,
            Err(e) => {
                plan.warnings
                    .push(format!("agent '{}' could not be parsed: {e}", entry.path));
                continue;
            }
        };
        let ns_name = format!("{slug}/{}", def.name);
        match classify(&def) {
            AgentRole::Reactive => {
                // Captured in `skipped_reactive`; the validate report summarizes
                // these. Event-triggered workers are Phase 4b, not v1.
                plan.skipped_reactive.push(ns_name);
            }
            AgentRole::Conversational => {
                let declared_tools: Vec<String> =
                    def.tools.iter().map(|t| t.name.clone()).collect();
                if !declared_tools.is_empty() {
                    plan.warnings.push(format!(
                        "agent '{}' declares {} inline tool(s) ({}) — NOT executed in v1; the agent uses the platform toolbox",
                        def.name,
                        declared_tools.len(),
                        declared_tools.join(", ")
                    ));
                }
                plan.personas.push(DerivedPersona {
                    name: ns_name.clone(),
                    system_prompt: def.system_prompt,
                    model: def.llm.model,
                    declared_tools,
                });
                conversational_names.push(ns_name);
            }
        }
    }

    if !conversational_names.is_empty() {
        let lead = pick_lead(manifest, &conversational_names);
        let members: Vec<String> = conversational_names
            .iter()
            .filter(|n| **n != lead)
            .cloned()
            .collect();
        plan.team = Some(DerivedTeam {
            name: slug.clone(),
            description: team_description(manifest),
            lead,
            members,
        });
    }

    plan
}

/// Lead = the pack's explicit `lead_agent` (namespaced) when present and itself
/// conversational, else the first conversational agent.
fn pick_lead(manifest: &PackManifest, conversational_ns: &[String]) -> String {
    if let Some(lead) = &manifest.lead_agent {
        let ns = format!("{}/{}", manifest.name, lead);
        if conversational_ns.contains(&ns) {
            return ns;
        }
    }
    conversational_ns
        .first()
        .cloned()
        .unwrap_or_else(|| manifest.name.clone())
}

/// Team description = the pack description, or a sensible fallback.
fn team_description(manifest: &PackManifest) -> String {
    if manifest.description.is_empty() {
        format!("{} agent team", manifest.name)
    } else {
        manifest.description.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_yaml(name: &str, reactive: bool, with_tool: bool) -> String {
        let triggers = if reactive {
            "triggers:\n  - event: some.event\n    source: some-source\n"
        } else {
            ""
        };
        let tools = if with_tool {
            "tools:\n  - name: lookup\n    kind: sql\n    query: SELECT 1\n"
        } else {
            ""
        };
        format!(
            "name: {name}\nkind: worker\ndescription: a {name}\nllm:\n  model: test-model\n  temperature: 0.0\nsystem_prompt: |\n  You are {name}.\n{triggers}{tools}"
        )
    }

    async fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(p, content).await.unwrap();
    }

    fn manifest(yaml_body: &str) -> PackManifest {
        serde_yaml::from_str(yaml_body).expect("manifest parses")
    }

    #[tokio::test]
    async fn parse_agent_def_reads_mapped_fields() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.yaml", &agent_yaml("triage", false, true)).await;
        let def = parse_agent_def(&dir.path().join("a.yaml")).await.unwrap();
        assert_eq!(def.name, "triage");
        assert_eq!(def.llm.model, "test-model");
        assert!(def.system_prompt.contains("You are triage"));
        assert_eq!(def.tools.len(), 1);
        assert_eq!(def.tools[0].name, "lookup");
        assert!(def.triggers.is_empty());
    }

    #[test]
    fn classify_splits_on_triggers() {
        let conv: AgentDef = serde_yaml::from_str("name: a\nsystem_prompt: hi\n").unwrap();
        assert_eq!(classify(&conv), AgentRole::Conversational);
        let react: AgentDef = serde_yaml::from_str("name: b\ntriggers:\n  - event: e\n").unwrap();
        assert_eq!(classify(&react), AgentRole::Reactive);
    }

    #[tokio::test]
    async fn two_conversational_agents_build_a_team() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "agents/lead.yaml",
            &agent_yaml("lead", false, false),
        )
        .await;
        write(
            dir.path(),
            "agents/helper.yaml",
            &agent_yaml("helper", false, false),
        )
        .await;
        let m = manifest(
            "name: demo\nversion: 1.0.0\ndescription: Demo team\nagents:\n  - agents/lead.yaml\n  - agents/helper.yaml\n",
        );
        let plan = plan_pack_agents(&m, dir.path()).await;
        assert_eq!(plan.personas.len(), 2);
        assert_eq!(plan.personas[0].name, "demo/lead");
        assert_eq!(plan.personas[0].model, "test-model");
        let team = plan.team.expect("team");
        assert_eq!(team.name, "demo");
        assert_eq!(team.description, "Demo team");
        assert_eq!(team.lead, "demo/lead");
        assert_eq!(team.members, vec!["demo/helper".to_string()]);
        assert!(plan.skipped_reactive.is_empty());
    }

    #[tokio::test]
    async fn reactive_agents_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "agents/chat.yaml",
            &agent_yaml("chat", false, false),
        )
        .await;
        write(
            dir.path(),
            "agents/watcher.yaml",
            &agent_yaml("watcher", true, false),
        )
        .await;
        let m = manifest(
            "name: ops\nversion: 1.0.0\nagents:\n  - agents/chat.yaml\n  - agents/watcher.yaml\n",
        );
        let plan = plan_pack_agents(&m, dir.path()).await;
        assert_eq!(plan.personas.len(), 1);
        assert_eq!(plan.skipped_reactive, vec!["ops/watcher".to_string()]);
        // A single conversational agent still forms a (lead-only) team.
        let team = plan.team.expect("team");
        assert_eq!(team.lead, "ops/chat");
        assert!(team.members.is_empty());
    }

    #[tokio::test]
    async fn lead_agent_override_is_honored() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "agents/a.yaml", &agent_yaml("a", false, false)).await;
        write(dir.path(), "agents/b.yaml", &agent_yaml("b", false, false)).await;
        let m = manifest(
            "name: t\nversion: 1.0.0\nlead_agent: b\nagents:\n  - agents/a.yaml\n  - agents/b.yaml\n",
        );
        let plan = plan_pack_agents(&m, dir.path()).await;
        let team = plan.team.expect("team");
        assert_eq!(team.lead, "t/b");
        assert_eq!(team.members, vec!["t/a".to_string()]);
    }

    #[tokio::test]
    async fn inline_tools_produce_a_warning_but_still_map() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "agents/sql.yaml",
            &agent_yaml("sql", false, true),
        )
        .await;
        let m = manifest("name: d\nversion: 1.0.0\nagents:\n  - agents/sql.yaml\n");
        let plan = plan_pack_agents(&m, dir.path()).await;
        assert_eq!(plan.personas.len(), 1);
        assert_eq!(plan.personas[0].declared_tools, vec!["lookup".to_string()]);
        assert!(plan
            .warnings
            .iter()
            .any(|w| w.contains("inline tool") && w.contains("platform toolbox")));
    }

    #[tokio::test]
    async fn unparseable_agent_warns_and_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "agents/ok.yaml",
            &agent_yaml("ok", false, false),
        )
        .await;
        write(dir.path(), "agents/bad.yaml", "name: [this is: not valid").await;
        let m =
            manifest("name: p\nversion: 1.0.0\nagents:\n  - agents/ok.yaml\n  - agents/bad.yaml\n");
        let plan = plan_pack_agents(&m, dir.path()).await;
        assert_eq!(plan.personas.len(), 1);
        assert!(plan
            .warnings
            .iter()
            .any(|w| w.contains("could not be parsed")));
    }

    #[tokio::test]
    async fn no_agents_yields_no_team() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest("name: empty\nversion: 1.0.0\n");
        let plan = plan_pack_agents(&m, dir.path()).await;
        assert!(plan.personas.is_empty());
        assert!(plan.team.is_none());
    }
}
