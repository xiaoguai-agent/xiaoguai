//! `RuntimeOutcome` — `AgentOutcome` enriched with caller-friendly
//! fields the three call sites (REST / IM / scheduler) all wanted to
//! compute independently before v0.12.0.
//!
//! Two derived fields land here:
//!
//! * [`RuntimeOutcome::reply_text`] — last non-empty assistant text
//!   message in `messages`. Empty when the loop produced none (rare
//!   in practice; only when a tool-call sequence ran without a
//!   trailing assistant turn).
//! * [`RuntimeOutcome::new_messages`] — messages produced *during*
//!   this run, i.e. the slice of `messages` after the inbound user
//!   prompt that was passed in. Mirrors the v0.7.4 IM gateway's
//!   hand-rolled `rposition` slice so callers don't reinvent it.

use xiaoguai_agent::{AgentOutcome, StopReason};
use xiaoguai_llm::{Message, Role};

#[derive(Debug, Clone)]
pub struct RuntimeOutcome {
    pub stop_reason: StopReason,
    pub iterations: u32,
    /// Full conversation including the input history.
    pub messages: Vec<Message>,
    /// Messages produced this run. Empty when the loop ran a single
    /// model turn that produced nothing (rare).
    pub new_messages: Vec<Message>,
    /// Last assistant text message; empty if the loop produced none.
    pub reply_text: String,
}

impl RuntimeOutcome {
    /// Build from an [`AgentOutcome`] and the *inbound* user prompt
    /// the runtime was asked to handle. The prompt is what the
    /// runtime added on top of the prior history before invoking the
    /// agent — used to find the split point for [`Self::new_messages`].
    #[must_use]
    pub fn from_agent(agent: AgentOutcome, inbound_prompt: &str) -> Self {
        let reply_text = agent
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, Role::Assistant) && !m.content.is_empty())
            .map_or_else(String::new, |m| m.content.clone());

        let new_messages = split_new_messages(&agent.messages, inbound_prompt);

        Self {
            stop_reason: agent.stop_reason,
            iterations: agent.iterations,
            messages: agent.messages,
            new_messages,
            reply_text,
        }
    }
}

/// Walk `messages` from the end and find the most recent user turn
/// whose `content` equals `inbound_prompt`. Return everything from
/// there inclusive. If the inbound prompt isn't found (e.g. the
/// agent's sliding window dropped it from a very long thread), return
/// an empty vec — the caller's defensive fallback (see v0.7.4) was a
/// minimal `[inbound, assistant_text]` pair, but the runtime doesn't
/// know `inbound` as a `Message` here so we hand back the empty case
/// and let the caller decide.
fn split_new_messages(messages: &[Message], inbound_prompt: &str) -> Vec<Message> {
    let idx = messages
        .iter()
        .rposition(|m| matches!(m.role, Role::User) && m.content == inbound_prompt);
    match idx {
        Some(i) => messages[i..].to_vec(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(messages: Vec<Message>) -> AgentOutcome {
        AgentOutcome {
            stop_reason: StopReason::Completed,
            messages,
            iterations: 1,
        }
    }

    #[test]
    fn reply_text_picks_last_non_empty_assistant() {
        let agent = make(vec![
            Message::user("hi"),
            Message::assistant("first"),
            Message::user("more"),
            Message::assistant("final"),
        ]);
        let o = RuntimeOutcome::from_agent(agent, "more");
        assert_eq!(o.reply_text, "final");
    }

    #[test]
    fn reply_text_empty_when_no_assistant_text() {
        let agent = make(vec![Message::user("hi"), Message::assistant("")]);
        let o = RuntimeOutcome::from_agent(agent, "hi");
        assert_eq!(o.reply_text, "");
    }

    #[test]
    fn new_messages_starts_at_matching_user_turn() {
        let agent = make(vec![
            Message::user("prior question"),
            Message::assistant("prior answer"),
            Message::user("fresh prompt"),
            Message::assistant("fresh answer"),
        ]);
        let o = RuntimeOutcome::from_agent(agent, "fresh prompt");
        assert_eq!(o.new_messages.len(), 2);
        assert_eq!(o.new_messages[0].content, "fresh prompt");
        assert_eq!(o.new_messages[1].content, "fresh answer");
    }

    #[test]
    fn new_messages_empty_when_inbound_not_found() {
        // Slide-window scenario: the inbound was dropped from
        // outcome.messages by the agent's history trimmer.
        let agent = make(vec![Message::assistant("orphan answer")]);
        let o = RuntimeOutcome::from_agent(agent, "lost prompt");
        assert!(o.new_messages.is_empty());
    }
}
