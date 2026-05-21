//! Configuration loader for Xiaoguai.
//!
//! Layering (highest precedence first):
//!   1. Environment variables (`XIAOGUAI_*` prefix, double-underscore separator
//!      maps to nested keys — e.g. `XIAOGUAI_DATABASE__URL` overrides `database.url`)
//!   2. `config.yaml` (path passed to `Settings::load_from_file`)
//!   3. Compiled-in defaults
//!
//! CLI flags layer on top via the binary's own `clap` parser.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub cache: CacheSettings,
    pub auth: AuthSettings,
    pub audit: AuditSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSettings {
    pub url: String,
    #[serde(default = "default_pg_max_connections")]
    pub max_connections: u32,
}

const fn default_pg_max_connections() -> u32 {
    16
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSettings {
    pub url: String,
    #[serde(default = "default_cache_prefix")]
    pub key_prefix: String,
}

fn default_cache_prefix() -> String {
    "xiaoguai:".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSettings {
    /// Expected JWT `iss` value.
    pub issuer: String,
    /// Expected JWT `aud` value.
    pub audience: String,
    /// JWKS URL (e.g. `https://idp.example.com/.well-known/jwks.json`).
    pub jwks_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditSettings {
    /// HMAC-SHA256 signing key for the audit chain. **NEVER** check in a real key.
    /// In production load via env or external secrets manager.
    pub hmac_key: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: ServerSettings {
                host: "0.0.0.0".into(),
                port: 7600,
            },
            database: DatabaseSettings {
                url: "postgres://xiaoguai:xiaoguai@localhost:5432/xiaoguai".into(),
                max_connections: default_pg_max_connections(),
            },
            cache: CacheSettings {
                url: "redis://localhost:6379".into(),
                key_prefix: default_cache_prefix(),
            },
            auth: AuthSettings {
                issuer: "https://idp.example.com".into(),
                audience: "xiaoguai-core".into(),
                jwks_url: "https://idp.example.com/.well-known/jwks.json".into(),
            },
            audit: AuditSettings {
                hmac_key: "dev-only-change-me-32-bytes-min".into(),
            },
        }
    }
}

impl Settings {
    /// Load settings from a YAML file + environment overrides.
    ///
    /// Environment variables use the `XIAOGUAI_` prefix and `__` separator,
    /// e.g. `XIAOGUAI_DATABASE__URL=postgres://...`.
    ///
    /// # Errors
    /// Returns a textual error if the file cannot be read or parsed.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let cfg = ::config::Config::builder()
            .add_source(::config::File::from(path.as_ref()))
            .add_source(::config::Environment::with_prefix("XIAOGUAI").separator("__"))
            .build()
            .map_err(|e| e.to_string())?;
        cfg.try_deserialize().map_err(|e| e.to_string())
    }

    /// Load settings from environment overrides only (uses defaults as base).
    ///
    /// # Errors
    /// Returns a textual error if env vars fail to deserialize.
    pub fn load_from_env() -> Result<Self, String> {
        let defaults_yaml = serde_yaml::to_string(&Self::default()).map_err(|e| e.to_string())?;
        let cfg = ::config::Config::builder()
            .add_source(::config::File::from_str(
                &defaults_yaml,
                ::config::FileFormat::Yaml,
            ))
            .add_source(::config::Environment::with_prefix("XIAOGUAI").separator("__"))
            .build()
            .map_err(|e| e.to_string())?;
        cfg.try_deserialize().map_err(|e| e.to_string())
    }
}
