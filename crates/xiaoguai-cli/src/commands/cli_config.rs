//! Persistent CLI preferences for the interactive `xiaoguai cli` session.
//!
//! Stored as JSON at `~/.xiaoguai/cli.json` (honoring `XDG_DATA_HOME`),
//! alongside the embedded `data.db`. These are operator UX prefs — the prompt
//! marker and default reply language — remembered across restarts, separate
//! from the server `config.yaml`. Mutated in-session via `/config`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Recognised values for `language`.
pub const LANGUAGES: &[&str] = &["auto", "zh", "en"];

/// The default prompt marker. Users are invited to change it via `/config`.
pub const DEFAULT_PROMPT: &str = "My agent>";

/// Persistent interactive-CLI preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CliConfig {
    /// Interactive prompt marker (default [`DEFAULT_PROMPT`]).
    pub prompt: String,
    /// Default reply-language hint: `auto` | `zh` | `en`.
    pub language: String,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            prompt: DEFAULT_PROMPT.to_string(),
            language: "auto".to_string(),
        }
    }
}

/// `~/.xiaoguai/cli.json` (honors `XDG_DATA_HOME`), mirroring the `data.db` dir
/// so all per-owner state lives together.
#[must_use]
pub fn config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg).join("xiaoguai").join("cli.json");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".xiaoguai").join("cli.json");
    }
    PathBuf::from("cli.json")
}

/// Load prefs from disk; returns defaults if the file is absent or malformed
/// (a corrupt prefs file must never block the CLI — it self-heals on next save).
#[must_use]
pub fn load() -> CliConfig {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => CliConfig::default(),
    }
}

/// Persist prefs as pretty JSON (creates `~/.xiaoguai` if needed).
///
/// # Errors
/// Returns an error if the directory can't be created or the file can't be written.
pub fn save(cfg: &CliConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {}", parent.display()))?;
        }
    }
    let body = serde_json::to_string_pretty(cfg).context("serialize cli config")?;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Apply a `key value` mutation (pure — no IO). Recognised keys: `prompt`,
/// `language` (alias `lang`).
///
/// # Errors
/// Returns `Err(message)` for an unknown key, an empty prompt, or an invalid
/// language value — the caller prints the message to the operator.
pub fn apply_set(cfg: &CliConfig, key: &str, value: &str) -> Result<CliConfig, String> {
    let v = value.trim();
    match key {
        "prompt" => {
            if v.is_empty() {
                return Err("prompt cannot be empty".to_string());
            }
            Ok(CliConfig {
                prompt: v.to_string(),
                ..cfg.clone()
            })
        }
        "language" | "lang" => {
            if !LANGUAGES.contains(&v) {
                return Err(format!("language must be one of: {}", LANGUAGES.join(", ")));
            }
            Ok(CliConfig {
                language: v.to_string(),
                ..cfg.clone()
            })
        }
        other => Err(format!("unknown setting: {other}  (try: prompt, language)")),
    }
}

/// Human-readable view for `/config` with no arguments.
#[must_use]
pub fn render(cfg: &CliConfig) -> String {
    format!(
        "config — {path}\n  \
         prompt    = {prompt}\n  \
         language  = {lang}   (auto | zh | en)\n  \
         change:  /config set prompt <text>   ·   /config set language zh",
        path = config_path().display(),
        prompt = cfg.prompt,
        lang = cfg.language,
    )
}

/// The system-style directive to prepend to a session's first turn so the
/// agent replies in the configured language. `None` for `auto`.
#[must_use]
pub fn language_directive(language: &str) -> Option<&'static str> {
    match language {
        "zh" => Some("请始终用简体中文回复。"),
        "en" => Some("Always respond in English."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_my_agent_and_auto() {
        let c = CliConfig::default();
        assert_eq!(c.prompt, "My agent>");
        assert_eq!(c.language, "auto");
    }

    #[test]
    fn apply_set_prompt_and_language() {
        let c = CliConfig::default();
        let c = apply_set(&c, "prompt", "  小怪>  ").unwrap();
        assert_eq!(c.prompt, "小怪>"); // trimmed
        let c = apply_set(&c, "language", "zh").unwrap();
        assert_eq!(c.language, "zh");
        // `lang` alias works
        assert_eq!(apply_set(&c, "lang", "en").unwrap().language, "en");
        // prompt change preserved across the language change
        assert_eq!(c.prompt, "小怪>");
    }

    #[test]
    fn apply_set_rejects_bad_input() {
        let c = CliConfig::default();
        assert!(apply_set(&c, "prompt", "   ").is_err()); // empty
        assert!(apply_set(&c, "language", "fr").is_err()); // unsupported
        assert!(apply_set(&c, "nope", "x").is_err()); // unknown key
    }

    #[test]
    fn language_directive_only_for_known_langs() {
        assert!(language_directive("zh").unwrap().contains("中文"));
        assert!(language_directive("en").is_some());
        assert_eq!(language_directive("auto"), None);
        assert_eq!(language_directive(""), None);
    }

    #[test]
    fn render_lists_both_keys() {
        let r = render(&CliConfig::default());
        assert!(r.contains("prompt"));
        assert!(r.contains("language"));
        assert!(r.contains("/config set"));
    }

    #[test]
    fn config_serde_roundtrip_with_unknown_fields_tolerant() {
        // Defaults fill missing fields; this guards forward-compat of the file.
        let c: CliConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, CliConfig::default());
        let back = serde_json::to_string(&c).unwrap();
        assert!(back.contains("prompt"));
    }
}
