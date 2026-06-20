//! `POST /v1/experts/suggest` — deterministic expert suggestion (T3.3).
//!
//! "一句话找专家": tokenize the goal, score every active persona and team by
//! keyword overlap, return a ranked list for the user to confirm. Suggestion
//! only — nothing is attached. Fully offline and deterministic, no LLM call
//! (owner decision ②A in `docs/plans/2026-06-10-expert-center.md`).
//!
//! NOTE: this deliberately does NOT go through the orchestrator's
//! `CapabilityRouter` — that router requires an agent to cover ALL required
//! capabilities (exact AND-match), which is the wrong semantics for fuzzy
//! free-text goals. The router stays the dispatch mechanism for T4, where
//! intents carry explicit capabilities.
//!
//! ## Scoring (pure functions, unit-tested below)
//!
//! - Tokens = lowercased ASCII alphanumeric words + CJK bigrams (so Chinese
//!   goals match without a segmenter; single CJK chars are too noisy).
//! - Persona score = 2 × |goal ∩ name tokens| + 1 × |goal ∩ prompt tokens|.
//! - Team score = 2 × |goal ∩ name| + 1 × |goal ∩ description| and teams also
//!   inherit their member personas' name tokens at weight 1.
//! - Zero-score candidates are dropped; ties break by name for stable output.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

use xiaoguai_personas::teams::model::Team;
use xiaoguai_personas::Persona;

use crate::error::ApiError;
use crate::state::AppState;

// ─── Tokenizer ────────────────────────────────────────────────────────────────

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}')
}

/// Lowercased ASCII-ish words + CJK bigrams. Pure and deterministic.
/// `pub(crate)` since T4.2: the orchestrate route's auto-routing reuses
/// this tokenizer + [`score_team`] so suggest and dispatch never diverge.
pub(crate) fn tokenize(text: &str) -> HashSet<String> {
    let mut tokens = HashSet::new();
    let mut word = String::new();
    let mut prev_cjk: Option<char> = None;

    for c in text.chars() {
        if c.is_alphanumeric() && !is_cjk(c) {
            word.extend(c.to_lowercase());
            prev_cjk = None;
            continue;
        }
        if !word.is_empty() {
            tokens.insert(std::mem::take(&mut word));
        }
        if is_cjk(c) {
            if let Some(p) = prev_cjk {
                tokens.insert(format!("{p}{c}"));
            }
            prev_cjk = Some(c);
        } else {
            prev_cjk = None;
        }
    }
    if !word.is_empty() {
        tokens.insert(word);
    }
    tokens
}

fn overlap(goal: &HashSet<String>, candidate: &HashSet<String>) -> u64 {
    goal.intersection(candidate).count() as u64
}

// ─── Scoring ──────────────────────────────────────────────────────────────────

const NAME_WEIGHT: u64 = 2;

fn score_persona(goal: &HashSet<String>, p: &Persona) -> u64 {
    NAME_WEIGHT * overlap(goal, &tokenize(&p.name)) + overlap(goal, &tokenize(&p.system_prompt))
}

/// `pub(crate)` since T4.2 — see [`tokenize`].
pub(crate) fn score_team(goal: &HashSet<String>, t: &Team, members: &[&Persona]) -> u64 {
    let member_names: HashSet<String> = members.iter().flat_map(|p| tokenize(&p.name)).collect();
    NAME_WEIGHT * overlap(goal, &tokenize(&t.name))
        + overlap(goal, &tokenize(&t.description))
        + overlap(goal, &member_names)
}

// ─── Request / response bodies ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SuggestBody {
    pub goal: String,
}

#[derive(Debug, Serialize)]
struct Suggestion {
    kind: &'static str, // "persona" | "team"
    id: Uuid,
    name: String,
    description: String,
    score: u64,
    /// For teams: the lead persona to attach. For personas: same as `id`.
    lead_persona_id: Uuid,
}

#[derive(Debug, Serialize)]
struct SuggestResponse {
    suggestions: Vec<Suggestion>,
}

// ─── Handler ──────────────────────────────────────────────────────────────────

// DEC-041: map to the canonical crate::error::ApiError ({code,message}).
fn unavailable(what: &str) -> Response {
    ApiError::ServiceUnavailable(format!("{what} repository not configured")).into_response()
}

pub async fn suggest_experts(
    State(state): State<AppState>,
    Json(body): Json<SuggestBody>,
) -> Response {
    let Some(personas_repo) = state.personas.clone() else {
        return unavailable("personas");
    };

    let goal = tokenize(&body.goal);
    if goal.is_empty() {
        return ApiError::BadRequest("goal must not be blank".into()).into_response();
    }

    let personas = match personas_repo.list().await {
        Ok(ps) => ps,
        Err(e) => {
            return ApiError::Internal(anyhow::anyhow!("experts: persona list failed: {e}"))
                .into_response();
        }
    };
    // Teams are optional — suggestion degrades to personas-only when the
    // teams repo isn't wired.
    let teams = match &state.teams {
        Some(repo) => match repo.list().await {
            Ok(ts) => ts,
            Err(e) => {
                tracing::error!(error = %e, "experts: team list failed");
                vec![]
            }
        },
        None => vec![],
    };

    let mut suggestions: Vec<Suggestion> = Vec::new();
    for p in &personas {
        let score = score_persona(&goal, p);
        if score > 0 {
            suggestions.push(Suggestion {
                kind: "persona",
                id: p.id,
                name: p.name.clone(),
                description: String::new(),
                score,
                lead_persona_id: p.id,
            });
        }
    }
    for t in &teams {
        let members: Vec<&Persona> = personas
            .iter()
            .filter(|p| t.member_persona_ids.contains(&p.id))
            .collect();
        let score = score_team(&goal, t, &members);
        if score > 0 {
            suggestions.push(Suggestion {
                kind: "team",
                id: t.id,
                name: t.name.clone(),
                description: t.description.clone(),
                score,
                lead_persona_id: t.lead_persona_id,
            });
        }
    }

    suggestions.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    (StatusCode::OK, Json(SuggestResponse { suggestions })).into_response()
}

// ─── Unit tests (pure functions) ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_ascii_words() {
        let t = tokenize("Analyse the Quarterly FINANCE report!");
        assert!(t.contains("analyse"));
        assert!(t.contains("finance"));
        assert!(t.contains("report"));
        assert!(!t.contains("the!"));
    }

    #[test]
    fn tokenize_cjk_bigrams() {
        let t = tokenize("财务报表");
        assert!(t.contains("财务"));
        assert!(t.contains("务报"));
        assert!(t.contains("报表"));
        assert!(!t.contains("财"));
    }

    #[test]
    fn tokenize_mixed_text_splits_scripts() {
        let t = tokenize("role/planner 财务 analysis");
        assert!(t.contains("role"));
        assert!(t.contains("planner"));
        assert!(t.contains("analysis"));
        // Single CJK char between spaces yields no bigram — by design.
        assert!(!t.contains("财务 analysis"));
    }

    #[test]
    fn tokenize_blank_is_empty() {
        assert!(tokenize("   ").is_empty());
        assert!(tokenize("!!!").is_empty());
    }

    #[test]
    fn name_overlap_outweighs_prompt_overlap() {
        let goal = tokenize("finance");
        let by_name = Persona {
            id: Uuid::new_v4(),
            name: "Finance Analyst".into(),
            system_prompt: "You analyse things.".into(),
            default_model: None,
            tool_allowlist: None,
            escalation_tier: None,
            created_at: chrono::Utc::now(),
            archived: false,
        };
        let by_prompt = Persona {
            name: "Helper".into(),
            system_prompt: "Finance only.".into(),
            ..by_name.clone()
        };
        assert!(score_persona(&goal, &by_name) > score_persona(&goal, &by_prompt));
    }
}
