//! `xiaoguai-personas` — agent personality / role profiles.
//!
//! A *persona* bundles three things an operator wants to pin to a specific
//! deployment context:
//!
//! 1. **System prompt** — injected as the leading system message in every chat
//!    turn so the agent consistently speaks as "Support Bot" or "Finance Analyst".
//! 2. **Tool allowlist** — an optional `Vec<String>` that gates which MCP /
//!    Toolbox tools the agent may invoke. `None` = unrestricted; `Some([])` =
//!    no tools; `Some([..])` = whitelist.
//! 3. **Escalation tier** — an opaque label consumed by the HOTL path (e.g.
//!    `"L1"`, `"L2"`, `"human"`).
//!
//! ## Quick start
//!
//! ```rust
//! use xiaoguai_personas::{
//!     InMemoryPersonaRepository, PersonaRepository,
//!     model::CreatePersonaRequest,
//!     enforcement::{filter_tools, tool_allowed},
//! };
//! use uuid::Uuid;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let repo = InMemoryPersonaRepository::new();
//! let tenant = Uuid::new_v4();
//!
//! let persona = repo.create(&CreatePersonaRequest {
//!     tenant_id: tenant,
//!     name: "Support Bot".to_string(),
//!     system_prompt: "You are a helpful support agent. Be concise.".to_string(),
//!     default_model: Some("gpt-4o-mini".to_string()),
//!     tool_allowlist: Some(vec!["web_search".to_string(), "read_kb".to_string()]),
//!     escalation_tier: Some("L1".to_string()),
//! }).await.unwrap();
//!
//! // Attach to a session.
//! repo.attach_persona_to_session("sess_abc", persona.id).await.unwrap();
//!
//! // At chat time: resolve and enforce.
//! let active = repo.get_session_persona("sess_abc").await.unwrap().unwrap();
//! assert!(tool_allowed(&active, "web_search"));
//! assert!(!tool_allowed(&active, "bash")); // not in allowlist
//!
//! let available = vec!["web_search".to_string(), "bash".to_string(), "read_kb".to_string()];
//! let permitted = filter_tools(&active, &available);
//! assert_eq!(permitted, vec!["web_search", "read_kb"]);
//! # }
//! ```
//!
//! ## Modules
//!
//! | Module         | Contents                                                    |
//! |----------------|-------------------------------------------------------------|
//! | [`model`]      | [`Persona`], [`CreatePersonaRequest`], [`UpdatePersonaRequest`], [`SessionPersona`] |
//! | [`traits`]     | [`PersonaRepository`] trait                                 |
//! | [`pg`]         | [`PgPersonaRepository`] — Postgres implementation           |
//! | [`memory`]     | [`InMemoryPersonaRepository`] — test / dev backend          |
//! | [`enforcement`]| [`tool_allowed`], [`filter_tools`], [`build_system_messages`] |
//! | [`routes`]     | axum REST route fragments for `/v1/personas` + `/v1/sessions/:id/persona` |
//! | [`error`]      | [`PersonaError`], [`PersonaResult`]                         |

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod enforcement;
pub mod error;
pub mod memory;
pub mod model;
pub mod pg;
pub mod routes;
pub mod traits;

// Public re-exports — the full API surface callers need.
pub use enforcement::{build_system_messages, filter_tools, tool_allowed};
pub use error::{PersonaError, PersonaResult};
pub use memory::InMemoryPersonaRepository;
pub use model::{CreatePersonaRequest, Persona, SessionPersona, UpdatePersonaRequest};
pub use pg::PgPersonaRepository;
pub use traits::PersonaRepository;
