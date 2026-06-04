//! Tool-allowlist enforcement helpers.
//!
//! These are pure functions — no I/O — so they are trivially testable
//! without a database. The runtime calls `filter_tools` before dispatching
//! a tool call; the chat-stream handler calls `build_system_messages` to
//! inject the persona's system prompt at the front of the message history.

use crate::model::Persona;

/// Return `true` iff `tool_name` is permitted by `persona`'s allowlist.
///
/// - `persona.tool_allowlist == None`       → all tools allowed.
/// - `persona.tool_allowlist == Some([])`   → no tools allowed.
/// - `persona.tool_allowlist == Some([..]`) → only listed tools allowed.
#[must_use]
pub fn tool_allowed(persona: &Persona, tool_name: &str) -> bool {
    persona.allows_tool(tool_name)
}

/// Filter `available_tools` to only those permitted by `persona`.
///
/// Returns a new `Vec<String>` — the caller retains the original slice
/// unchanged (immutable pattern).
#[must_use]
pub fn filter_tools(persona: &Persona, available_tools: &[String]) -> Vec<String> {
    available_tools
        .iter()
        .filter(|t| tool_allowed(persona, t))
        .cloned()
        .collect()
}

/// Build the leading system message(s) to prepend when a persona is active.
///
/// Returns a single element containing `persona.system_prompt`. Returns an
/// empty `Vec` when the prompt is blank so callers don't inject unnecessary
/// whitespace into the history.
#[must_use]
pub fn build_system_messages(persona: &Persona) -> Vec<String> {
    if persona.system_prompt.trim().is_empty() {
        vec![]
    } else {
        vec![persona.system_prompt.clone()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Persona;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_persona(tool_allowlist: Option<Vec<String>>) -> Persona {
        Persona {
            id: Uuid::new_v4(),
            name: "test".to_string(),
            system_prompt: "You are helpful.".to_string(),
            default_model: None,
            tool_allowlist,
            escalation_tier: None,
            created_at: Utc::now(),
            archived: false,
        }
    }

    #[test]
    fn unrestricted_allows_any_tool() {
        let persona = make_persona(None);
        assert!(tool_allowed(&persona, "web_search"));
        assert!(tool_allowed(&persona, "bash"));
        assert!(tool_allowed(&persona, "anything_goes"));
    }

    #[test]
    fn empty_allowlist_denies_all_tools() {
        let persona = make_persona(Some(vec![]));
        assert!(!tool_allowed(&persona, "web_search"));
        assert!(!tool_allowed(&persona, "bash"));
    }

    #[test]
    fn populated_allowlist_permits_only_listed_tools() {
        let persona = make_persona(Some(vec![
            "web_search".to_string(),
            "read_file".to_string(),
        ]));
        assert!(tool_allowed(&persona, "web_search"));
        assert!(tool_allowed(&persona, "read_file"));
        assert!(!tool_allowed(&persona, "bash"));
        assert!(!tool_allowed(&persona, "delete_file"));
    }

    #[test]
    fn filter_tools_returns_subset() {
        let persona = make_persona(Some(vec!["a".to_string(), "c".to_string()]));
        let available = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let filtered = filter_tools(&persona, &available);
        assert_eq!(filtered, vec!["a", "c"]);
    }

    #[test]
    fn filter_tools_unrestricted_returns_all() {
        let persona = make_persona(None);
        let available = vec!["a".to_string(), "b".to_string()];
        let filtered = filter_tools(&persona, &available);
        assert_eq!(filtered, available);
    }

    #[test]
    fn build_system_messages_non_empty() {
        let persona = make_persona(None);
        let msgs = build_system_messages(&persona);
        assert_eq!(msgs, vec!["You are helpful."]);
    }

    #[test]
    fn build_system_messages_blank_prompt_returns_empty() {
        let mut persona = make_persona(None);
        persona.system_prompt = "   ".to_string();
        let msgs = build_system_messages(&persona);
        assert!(msgs.is_empty());
    }
}
