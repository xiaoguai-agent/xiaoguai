//! Team glossary injection text (T7.1,
//! `docs/plans/2026-06-10-memory-multisource.md` §1.1).
//!
//! Three-tier context model: `USER.md` (owner identity, outermost) → team
//! glossary (team knowledge, this module) → persona `system_prompt` (role).
//! The glossary is injected as a System message:
//!
//! * chat turns ([`crate::turn::run_turn`]): right AFTER the identity
//!   message and BEFORE the session history — for Execute **and** Consult
//!   modes, and for loop ticks too (a loop tick belongs to a session like
//!   any other turn, so the team's shared vocabulary applies);
//! * orchestrate member/synthesis runs
//!   ([`crate::orchestrate::OrchestrateMemberRunner`]): right after the
//!   persona system messages (the persona prompt leads there) and before
//!   the user prompt.
//!
//! Like identity, the glossary is never persisted into the session history.

use xiaoguai_personas::teams::model::Team;

/// Build the glossary System-message text for `team`, or `None` when the
/// team carries no glossary or a blank one (repos normalise blank to `None`,
/// but stay defensive — old rows may predate normalisation). Pure.
#[must_use]
pub fn glossary_system_text(team: &Team) -> Option<String> {
    let md = team.glossary_md.as_deref()?.trim();
    if md.is_empty() {
        return None;
    }
    Some(format!("Team glossary ({}):\n{md}", team.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn team(glossary_md: Option<&str>) -> Team {
        let lead = Uuid::new_v4();
        Team {
            id: Uuid::new_v4(),
            name: "Finance Squad".to_string(),
            description: String::new(),
            lead_persona_id: lead,
            member_persona_ids: vec![lead],
            recommended_pack_slugs: vec![],
            glossary_md: glossary_md.map(str::to_string),
            created_at: Utc::now(),
            archived: false,
        }
    }

    #[test]
    fn formats_name_and_markdown() {
        let text = glossary_system_text(&team(Some("MRR = monthly recurring revenue"))).unwrap();
        assert_eq!(
            text,
            "Team glossary (Finance Squad):\nMRR = monthly recurring revenue"
        );
    }

    #[test]
    fn none_when_absent_or_blank() {
        assert_eq!(glossary_system_text(&team(None)), None);
        assert_eq!(glossary_system_text(&team(Some("  \n\t"))), None);
    }

    #[test]
    fn markdown_is_trimmed() {
        let text = glossary_system_text(&team(Some("  term\n"))).unwrap();
        assert!(text.ends_with("term"));
    }
}
