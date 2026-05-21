//! Configuration loader for Xiaoguai.
//!
//! Layering (highest precedence first):
//!   1. CLI flags (`clap`)
//!   2. Environment variables (`XIAOGUAI_*` prefix)
//!   3. `config.yaml` next to the binary
//!   4. `/etc/xiaoguai/config.yaml`
//!   5. Compiled-in defaults

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub cache: CacheSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSettings {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSettings {
    pub url: String,
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
            },
            cache: CacheSettings {
                url: "redis://localhost:6379".into(),
            },
        }
    }
}
