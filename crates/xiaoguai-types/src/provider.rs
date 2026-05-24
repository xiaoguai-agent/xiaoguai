//! LLM provider domain type.
//!
//! Mirrors the `llm_providers` Postgres table. Secret values are **not**
//! stored — only the name of the environment variable from which the runtime
//! reads the API key (`api_key_env`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{ProviderId, TenantId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "ollama")]
    Ollama,
    #[serde(rename = "openai_compat")]
    OpenAiCompat,
    /// Anthropic Messages API (`/v1/messages`). Auth via `x-api-key` header +
    /// `anthropic-version: 2023-06-01`. Models: `claude-sonnet-4-6`,
    /// `claude-opus-4-7`, `claude-haiku-4-5`.
    #[serde(rename = "anthropic")]
    Anthropic,
    /// Google Gemini `generateContent` API. Auth via `key=` query param.
    /// Models: `gemini-2.0-flash`, `gemini-2.5-pro`.
    #[serde(rename = "gemini")]
    Gemini,
}

impl ProviderKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenAiCompat => "openai_compat",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }

    /// Parse the DB string back into a kind. Returns `None` for unknown values.
    ///
    /// Named `parse` rather than `from_str` to avoid confusion with the
    /// `std::str::FromStr` trait (which returns `Result`, not `Option`).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ollama" => Some(Self::Ollama),
            "openai_compat" => Some(Self::OpenAiCompat),
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProvider {
    pub id: ProviderId,
    /// `None` means a system-wide provider visible to every tenant.
    pub tenant_id: Option<TenantId>,
    pub name: String,
    pub kind: ProviderKind,
    pub endpoint: String,
    pub models: Vec<String>,
    pub default_for_models: Vec<String>,
    pub fallback_order: i32,
    /// Name of the env var holding the API key. None for unauthenticated
    /// endpoints (e.g. local Ollama).
    pub api_key_env: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_round_trips_through_string() {
        for k in [
            ProviderKind::Ollama,
            ProviderKind::OpenAiCompat,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
        ] {
            assert_eq!(ProviderKind::parse(k.as_str()), Some(k));
        }
    }

    #[test]
    fn unknown_kind_returns_none() {
        assert_eq!(ProviderKind::parse("vertexai"), None);
    }

    #[test]
    fn kind_serializes_snake_case() {
        let s = serde_json::to_string(&ProviderKind::OpenAiCompat).unwrap();
        assert_eq!(s, "\"openai_compat\"");
    }
}
