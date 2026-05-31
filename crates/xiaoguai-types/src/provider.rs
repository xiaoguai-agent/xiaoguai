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
    /// AWS Bedrock `InvokeModelWithResponseStream`. Auth via `SigV4`.
    /// Models: `anthropic.claude-sonnet-4-6-v1:0`, `meta.llama3-70b-instruct-v1:0`.
    /// `endpoint` field stores the AWS region (e.g. `us-east-1`).
    #[serde(rename = "bedrock")]
    Bedrock,
    /// Azure OpenAI `chat/completions` API. Auth via `api-key` header.
    /// `endpoint` stores the full deployment URL:
    /// `https://{resource}.openai.azure.com/openai/deployments/{deployment}`.
    #[serde(rename = "azure_openai")]
    AzureOpenAi,
    /// Mistral La Plateforme `v1/chat/completions`. Auth via Bearer token.
    /// Models: `mistral-large-latest`, `codestral-latest`.
    #[serde(rename = "mistral")]
    Mistral,
    /// Groq fast-inference `openai/v1/chat/completions`. Auth via Bearer token.
    /// Models: `llama-3.3-70b-versatile`, `mixtral-8x7b-32768`.
    #[serde(rename = "groq")]
    Groq,
    /// `MiniMax` OpenAI-compatible `/v1/chat/completions`. Auth via Bearer
    /// token. Models: `MiniMax-M1`, `MiniMax-M2`, `MiniMax-M2.5`,
    /// `MiniMax-M2.7`, `abab6.5-chat`. M1/M2 series stream
    /// `reasoning_content` deltas on chunks; we surface them via
    /// `ChatChunk.reasoning_delta`. See DEC-024.
    #[serde(rename = "minimax")]
    MiniMax,
}

impl ProviderKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenAiCompat => "openai_compat",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Bedrock => "bedrock",
            Self::AzureOpenAi => "azure_openai",
            Self::Mistral => "mistral",
            Self::Groq => "groq",
            Self::MiniMax => "minimax",
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
            "bedrock" => Some(Self::Bedrock),
            "azure_openai" => Some(Self::AzureOpenAi),
            "mistral" => Some(Self::Mistral),
            "groq" => Some(Self::Groq),
            "minimax" => Some(Self::MiniMax),
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
    /// v1.1.1.1 — 2026-Q2 list pricing per provider docs.
    /// `None` means no rate configured; the Usage pane shows "—".
    /// USD per 1,000 input tokens.
    pub cost_per_1k_input_usd: Option<f64>,
    /// USD per 1,000 output tokens.
    pub cost_per_1k_output_usd: Option<f64>,
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
            ProviderKind::Bedrock,
            ProviderKind::AzureOpenAi,
            ProviderKind::Mistral,
            ProviderKind::Groq,
            ProviderKind::MiniMax,
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

    #[test]
    fn new_kinds_serialize_correctly() {
        assert_eq!(
            serde_json::to_string(&ProviderKind::Bedrock).unwrap(),
            "\"bedrock\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderKind::AzureOpenAi).unwrap(),
            "\"azure_openai\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderKind::Mistral).unwrap(),
            "\"mistral\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderKind::Groq).unwrap(),
            "\"groq\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderKind::MiniMax).unwrap(),
            "\"minimax\""
        );
    }
}
