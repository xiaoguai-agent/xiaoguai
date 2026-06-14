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

/// Render the wizard epilogue's next steps (T8.1,
/// `docs/plans/2026-06-10-install-polish.md` §1): start the server, then
/// send a first message / open the UI. `port` is the configured serve port.
#[must_use]
pub fn format_next_steps(port: u16) -> String {
    format!(
        "Next steps:\n\
         \x20 1. start the server:    xiaoguai serve\n\
         \x20 2. say hello:           open http://localhost:{port}/ — or run: xiaoguai cli"
    )
}

/// The region picker shown for `MiniMax` during `init`. `MiniMax` runs two
/// separate regions whose API keys are **not interchangeable**, so a correct
/// key against the wrong host 401s — the #1 fresh-install failure for CN users.
pub const MINIMAX_REGION_MENU: &str = "\
  1) International  — api.minimax.io    (console: minimax.io)\n\
  \x20 2) China / 国内   — api.minimaxi.com  (console: platform.minimaxi.com)";

/// Map a 1-based `MiniMax` region choice to its API base URL (the backend
/// appends `/v1/chat/completions`). Returns `None` for unrecognised input so
/// the caller can re-prompt.
#[must_use]
pub fn minimax_region_endpoint(input: &str) -> Option<&'static str> {
    match input.trim() {
        "1" => Some("https://api.minimax.io"),
        "2" => Some("https://api.minimaxi.com"),
        _ => None,
    }
}

/// Mask an API key for an on-screen confirmation line: keep the last 4
/// characters, bullet out the rest, and note the length — enough to confirm
/// "yes, that's the key I pasted" without dumping the secret into terminal
/// scrollback. Keys of 4 chars or fewer are fully masked.
#[must_use]
pub fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n == 0 {
        return "(empty)".to_string();
    }
    let tail: String = if n <= 4 {
        String::new()
    } else {
        key.chars().skip(n - 4).collect()
    };
    let bullets = "•".repeat(n - tail.chars().count());
    format!("{bullets}{tail} ({n} chars)")
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

    #[test]
    fn next_steps_carry_serve_then_first_message_on_the_configured_port() {
        let s = format_next_steps(7601);
        assert!(s.contains("xiaoguai serve"));
        assert!(s.contains("http://localhost:7601/"));
        assert!(s.contains("xiaoguai cli"));
        assert_eq!(s.lines().count(), 3);
    }
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
    fn minimax_region_maps_choice_to_host() {
        assert_eq!(minimax_region_endpoint("1"), Some("https://api.minimax.io"));
        assert_eq!(
            minimax_region_endpoint(" 2 "),
            Some("https://api.minimaxi.com")
        );
        assert_eq!(minimax_region_endpoint("3"), None);
        assert_eq!(minimax_region_endpoint(""), None);
        assert_eq!(minimax_region_endpoint("intl"), None);
    }

    #[test]
    fn minimax_region_menu_names_both_hosts() {
        assert!(MINIMAX_REGION_MENU.contains("api.minimax.io"));
        assert!(MINIMAX_REGION_MENU.contains("api.minimaxi.com"));
    }

    #[test]
    fn mask_key_shows_tail_and_length_without_leaking() {
        assert_eq!(mask_key(""), "(empty)");
        // long key: last 4 visible, rest bulleted, length noted
        let m = mask_key("sk-abcdefghij6789");
        assert!(m.ends_with("6789 (17 chars)"), "got {m}");
        assert!(m.starts_with('•'));
        assert!(!m.contains("abcde")); // body not leaked
        // short key fully masked
        assert_eq!(mask_key("abcd"), "•••• (4 chars)");
        assert_eq!(mask_key("xy"), "•• (2 chars)");
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
