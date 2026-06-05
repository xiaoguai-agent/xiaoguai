//! `xiaoguai init` — interactive setup wizard.
//!
//! This module holds the **pure, testable** pieces (menu rendering + answer
//! parsing). The terminal I/O (reading stdin, hiding the key with `stty`) lives
//! in `main.rs::handle_init`, which feeds these helpers and then drives the
//! already-tested [`crate::commands::provider::update`].

use std::fmt::Write as _;

use xiaoguai_types::LlmProvider;

/// Render the numbered provider menu shown by the wizard.
#[must_use]
pub fn format_provider_menu(providers: &[LlmProvider]) -> String {
    let mut out = String::new();
    for (i, p) in providers.iter().enumerate() {
        let models = if p.models.is_empty() {
            "(no models)".to_string()
        } else {
            p.models.join(", ")
        };
        let key = if p.api_key.is_some() {
            " [key stored]"
        } else if p.api_key_env.is_some() {
            " [key via env]"
        } else {
            ""
        };
        // `write!` to a String is infallible.
        let _ = writeln!(
            out,
            "  {n}) {name:<22} {models}{key}",
            n = i + 1,
            name = p.name,
        );
    }
    out
}

/// Parse a 1-based menu choice into a 0-based index. Returns `None` for
/// non-numeric input or an out-of-range number.
#[must_use]
pub fn parse_selection(input: &str, count: usize) -> Option<usize> {
    let n: usize = input.trim().parse().ok()?;
    // `then` (lazy) not `then_some` — the latter would eval `n - 1` even when
    // out of range, underflowing usize for input "0".
    (1..=count).contains(&n).then(|| n - 1)
}

/// Parse a yes/no answer. Empty input returns `default_yes`; `y`/`yes` → true,
/// `n`/`no` → false; anything else falls back to `default_yes`.
#[must_use]
pub fn parse_yes_no(input: &str, default_yes: bool) -> bool {
    match input.trim().to_ascii_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use xiaoguai_types::{LlmProvider, ProviderId, ProviderKind};

    fn prov(name: &str, models: Vec<&str>, api_key: Option<&str>) -> LlmProvider {
        let now = Utc::now();
        LlmProvider {
            id: ProviderId::new(),
            name: name.into(),
            kind: ProviderKind::OpenAiCompat,
            endpoint: "https://x/v1".into(),
            models: models.into_iter().map(Into::into).collect(),
            default_for_models: vec![],
            fallback_order: 100,
            api_key_env: None,
            api_key: api_key.map(Into::into),
            created_at: now,
            updated_at: now,
            cost_per_1k_input_usd: None,
            cost_per_1k_output_usd: None,
        }
    }

    #[test]
    fn selection_parses_in_range_one_based() {
        assert_eq!(parse_selection("1", 3), Some(0));
        assert_eq!(parse_selection(" 3 ", 3), Some(2));
        assert_eq!(parse_selection("0", 3), None);
        assert_eq!(parse_selection("4", 3), None);
        assert_eq!(parse_selection("x", 3), None);
        assert_eq!(parse_selection("", 3), None);
    }

    #[test]
    fn yes_no_handles_defaults_and_words() {
        assert!(parse_yes_no("", true));
        assert!(!parse_yes_no("", false));
        assert!(parse_yes_no("y", false));
        assert!(parse_yes_no("YES", false));
        assert!(!parse_yes_no("n", true));
        assert!(!parse_yes_no("No", true));
        // Unrecognised falls back to the default.
        assert!(parse_yes_no("maybe", true));
    }

    #[test]
    fn menu_numbers_and_flags_each_provider() {
        let menu = format_provider_menu(&[
            prov("minimax", vec!["MiniMax-M2"], None),
            prov("openai", vec!["gpt-4o"], Some("sk-stored")),
        ]);
        assert!(menu.contains("1) minimax"));
        assert!(menu.contains("MiniMax-M2"));
        assert!(menu.contains("2) openai"));
        assert!(menu.contains("[key stored]"));
    }
}
