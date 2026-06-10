//! Agent-backed [`MemberRunner`] for the executive orchestration pattern
//! (T4.2 of `docs/plans/2026-06-10-executive-orchestration.md` §2.2).
//!
//! [`OrchestrateMemberRunner`] composes one in-process agent turn per team
//! member: persona system prompt as the leading System message, persona
//! `default_model` (else the session/global default), the shared toolbox
//! narrowed by the persona's allowlist, the inherited `HotL` gate from
//! `agent_defaults`, and the attribution label `orch:<run_id>:<persona_id>`
//! (disjoint from `sess_*` — exact-match budget sums stay unaffected).
//!
//! Member runs go through [`xiaoguai_runtime::run_to_completion`] — member
//! `AgentEvent`s are NOT streamed to the client in v1. The SSE surface is
//! the executive runner's own [`ExecEvent`] stream (`MemberStarted` /
//! `MemberCompleted` / ...); member transcripts surface through attribution
//! + audit, per plan §0 size discipline.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use xiaoguai_agent::{AgentConfig, StopReason, Toolbox};
use xiaoguai_llm::{LlmBackend, Message as LlmMessage};
use xiaoguai_orchestrator::patterns::executive::{MemberOutcome, MemberRunner, MemberSpec};
use xiaoguai_orchestrator::OrchestratorError;
use xiaoguai_personas::{build_system_messages, filter_tools, Persona};
use xiaoguai_runtime::{run_to_completion, RuntimeContext};

/// Build the per-member token-usage attribution label.
///
/// Contract: the `orch:` prefix is disjoint from session ids (`sess_*`) and
/// from the scheduler/IM synthetic labels (`scheduler:<job_id>`,
/// `im:<provider>:<conv>`), so exact-match budget sums never absorb
/// orchestration usage. Pure — unit-tested below.
#[must_use]
pub fn attribution_label(run_id: Uuid, persona_id: Uuid) -> String {
    format!("orch:{run_id}:{persona_id}")
}

/// Build the lead's synthesis prompt: the goal followed by one numbered
/// section per surviving member (name + outcome text), plus an instruction
/// to synthesize a single answer and surface inter-member disagreements
/// explicitly (plan §2.1 synthesis contract). Pure — unit-tested below.
#[must_use]
pub fn build_synthesis_prompt(goal: &str, sections: &[(String, String)]) -> String {
    let mut prompt = format!("Goal:\n{goal}\n\nTeam member findings:\n");
    for (i, (name, text)) in sections.iter().enumerate() {
        let n = i + 1;
        prompt.push_str(&format!("\n{n}. [{name}]\n{text}\n"));
    }
    prompt.push_str(
        "\nSynthesize the findings above into ONE final answer to the goal. \
         Where members disagree, surface the disagreement explicitly — name \
         the members and their positions — rather than papering over it.",
    );
    prompt
}

/// Agent-backed [`MemberRunner`] over the shared runtime. Constructed once
/// per orchestrate call with everything pre-resolved (no repository access
/// inside a run): the personas map, the session's fallback model, the
/// owner actor, the per-call `run_id`, and the turn's cancellation token.
pub struct OrchestrateMemberRunner {
    backend: Arc<dyn LlmBackend>,
    toolbox: Arc<Toolbox>,
    /// Cloned per member run; carries the inherited `HotL` gate
    /// (`AgentConfig.hotl_gate` is part of the clone).
    agent_defaults: AgentConfig,
    /// Pre-resolved active personas keyed by id (lead included).
    personas: HashMap<Uuid, Persona>,
    /// Session model — used when the persona has no `default_model`.
    /// May be empty (the LLM router substitutes its default).
    fallback_model: String,
    /// Audit/attribution actor — the session owner.
    actor: String,
    /// One id per orchestrate call; stamped into every attribution label.
    run_id: Uuid,
    /// Clone of the session turn's cancellation token, so
    /// `POST /v1/sessions/:id/cancel` stops every member run.
    cancel: CancellationToken,
    /// T7.1: pre-formatted team-glossary System message
    /// ([`crate::glossary::glossary_system_text`]); `None` when the team has
    /// no glossary. Injected right after the persona system messages — the
    /// persona prompt stays the leading message here (it defines the role),
    /// the glossary follows as shared team context, then the user prompt.
    /// Applied to member runs AND the lead's synthesis turn (both go
    /// through `run_persona_turn`).
    glossary_message: Option<String>,
}

impl OrchestrateMemberRunner {
    #[must_use]
    pub fn new(
        backend: Arc<dyn LlmBackend>,
        toolbox: Arc<Toolbox>,
        agent_defaults: AgentConfig,
        personas: HashMap<Uuid, Persona>,
        fallback_model: String,
        actor: String,
        run_id: Uuid,
        cancel: CancellationToken,
        glossary_message: Option<String>,
    ) -> Self {
        Self {
            backend,
            toolbox,
            agent_defaults,
            personas,
            fallback_model,
            actor,
            run_id,
            cancel,
            glossary_message,
        }
    }

    fn persona(&self, id: Uuid) -> Result<&Persona, OrchestratorError> {
        self.personas.get(&id).ok_or_else(|| {
            OrchestratorError::Internal(format!("persona {id} not pre-resolved for this run"))
        })
    }

    /// One full agent turn for `persona` with `prompt` as the user message.
    /// Shared by member runs and the lead's synthesis turn — both are
    /// governed identically (persona prompt, allowlist, `HotL`, attribution).
    async fn run_persona_turn(
        &self,
        persona: &Persona,
        prompt: &str,
    ) -> Result<xiaoguai_runtime::RuntimeOutcome, OrchestratorError> {
        let mut messages: Vec<LlmMessage> = build_system_messages(persona)
            .into_iter()
            .map(LlmMessage::system)
            .collect();
        // T7.1: team glossary right after the persona system messages —
        // every member (and the synthesis turn) shares the team vocabulary.
        if let Some(glossary) = &self.glossary_message {
            messages.push(LlmMessage::system(glossary));
        }
        messages.push(LlmMessage::user(prompt));

        let model = persona
            .default_model
            .clone()
            .unwrap_or_else(|| self.fallback_model.clone());
        let toolbox = Arc::new(subset_toolbox(&self.toolbox, persona));
        let ctx = RuntimeContext::new(self.backend.clone(), toolbox, self.agent_defaults.clone())
            .with_model(model)
            .with_attribution(
                Some(attribution_label(self.run_id, persona.id)),
                Some(self.actor.clone()),
            );

        run_to_completion(&ctx, messages, self.cancel.clone())
            .await
            .map_err(|e| OrchestratorError::WorkerFailed(e.to_string()))
    }

    /// Display name for an outcome's persona; falls back to the id when the
    /// persona is somehow missing (defensive — should not happen, the map
    /// is built from the same member list).
    fn persona_name(&self, id: Uuid) -> String {
        self.personas
            .get(&id)
            .map_or_else(|| id.to_string(), |p| p.name.clone())
    }
}

#[async_trait]
impl MemberRunner for OrchestrateMemberRunner {
    async fn run_member(
        &self,
        member: &MemberSpec,
        goal: &str,
    ) -> Result<MemberOutcome, OrchestratorError> {
        let persona = self.persona(member.id)?;
        let outcome = self.run_persona_turn(persona, goal).await?;
        // Soft failure (`ok: false`) when the turn did not run to a clean
        // stop or produced no usable text — the executive loop drops it
        // from synthesis but keeps the run going.
        let ok = matches!(outcome.stop_reason, StopReason::Completed)
            && !outcome.reply_text.trim().is_empty();
        Ok(MemberOutcome {
            id: member.id,
            ok,
            text: outcome.reply_text,
            iterations: outcome.iterations,
        })
    }

    async fn run_synthesis(
        &self,
        lead: &MemberSpec,
        goal: &str,
        outcomes: &[MemberOutcome],
    ) -> Result<String, OrchestratorError> {
        let persona = self.persona(lead.id)?;
        let sections: Vec<(String, String)> = outcomes
            .iter()
            .map(|o| (self.persona_name(o.id), o.text.clone()))
            .collect();
        let prompt = build_synthesis_prompt(goal, &sections);
        let outcome = self.run_persona_turn(persona, &prompt).await?;
        if outcome.reply_text.trim().is_empty() {
            return Err(OrchestratorError::WorkerFailed(
                "synthesis turn produced no text".to_string(),
            ));
        }
        Ok(outcome.reply_text)
    }
}

/// Narrow `base` to the tools the persona's allowlist permits. The
/// [`Toolbox`] has no subset constructor, so this rebuilds one from the
/// allowed entries (clone of client handle + descriptor) — `base` stays
/// untouched (immutable pattern), mirroring how `loop_tools` builds a
/// per-turn toolbox instead of mutating the shared one.
fn subset_toolbox(base: &Toolbox, persona: &Persona) -> Toolbox {
    let available: Vec<String> = base.to_specs().into_iter().map(|s| s.name).collect();
    let allowed = filter_tools(persona, &available);
    let mut tb = Toolbox::new();
    for name in &allowed {
        if let Some(entry) = base.get(name) {
            // `insert_or_replace`: names come from `base` so duplicates are
            // impossible, but this avoids an unreachable error branch.
            tb.insert_or_replace(entry.client.clone(), entry.descriptor.clone());
        }
    }
    tb
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn persona(name: &str, allowlist: Option<Vec<String>>) -> Persona {
        Persona {
            id: Uuid::new_v4(),
            name: name.to_string(),
            system_prompt: format!("You are {name}."),
            default_model: None,
            tool_allowlist: allowlist,
            escalation_tier: None,
            created_at: Utc::now(),
            archived: false,
        }
    }

    #[test]
    fn attribution_label_has_orch_prefix_and_both_ids() {
        let run_id = Uuid::new_v4();
        let persona_id = Uuid::new_v4();
        let label = attribution_label(run_id, persona_id);
        assert!(label.starts_with("orch:"), "got {label}");
        assert!(label.contains(&run_id.to_string()));
        assert!(label.contains(&persona_id.to_string()));
        // Disjoint from session attribution: never collides with `sess_*`.
        assert!(!label.starts_with("sess_"));
    }

    #[test]
    fn attribution_label_is_exact_format() {
        let run_id = Uuid::nil();
        let persona_id = Uuid::nil();
        assert_eq!(
            attribution_label(run_id, persona_id),
            format!("orch:{run_id}:{persona_id}")
        );
    }

    #[test]
    fn synthesis_prompt_numbers_sections_and_instructs_on_disagreement() {
        let sections = vec![
            ("Analyst".to_string(), "Revenue is up.".to_string()),
            ("Skeptic".to_string(), "Revenue is flat.".to_string()),
        ];
        let prompt = build_synthesis_prompt("Assess Q2", &sections);
        assert!(prompt.contains("Assess Q2"));
        assert!(prompt.contains("1. [Analyst]"));
        assert!(prompt.contains("2. [Skeptic]"));
        assert!(prompt.contains("Revenue is up."));
        assert!(prompt.contains("Revenue is flat."));
        // The disagreement instruction is part of the v1 conflict contract.
        assert!(prompt.contains("disagree"));
        // Section order is preserved (original member order).
        let a = prompt.find("Analyst").expect("analyst present");
        let s = prompt.find("Skeptic").expect("skeptic present");
        assert!(a < s);
    }

    #[test]
    fn synthesis_prompt_with_no_sections_still_carries_goal() {
        let prompt = build_synthesis_prompt("the goal", &[]);
        assert!(prompt.contains("the goal"));
        assert!(prompt.contains("Synthesize"));
    }

    #[test]
    fn subset_toolbox_of_empty_base_is_empty() {
        let base = Toolbox::new();
        let p = persona("Restricted", Some(vec!["a".to_string()]));
        assert!(subset_toolbox(&base, &p).is_empty());
    }

    #[test]
    fn subset_toolbox_empty_allowlist_denies_everything() {
        // Even with a non-empty base, `Some([])` means no tools.
        let base = Toolbox::new();
        let p = persona("NoTools", Some(vec![]));
        let tb = subset_toolbox(&base, &p);
        assert!(tb.is_empty());
        assert!(tb.to_specs().is_empty());
    }
}
