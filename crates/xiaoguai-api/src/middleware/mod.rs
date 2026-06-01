//! Reusable axum middleware + extractors.
//!
//! Sprint-14 S14-1: [`require_scope`] introduces `RequireScope<S>` — a
//! marker-trait-based axum extractor that gates a handler on the
//! presence of a named OAuth-style scope inside the request's
//! `Claims`. See `docs/lld/lld-agent.md` §4.7 (DEC-HLD-018) for the
//! design rationale (in particular: why marker traits and NOT
//! `const SCOPE: &'static str` const generics — stable Rust 1.93
//! does not allow `&'static str` as a const-generic parameter).

pub mod require_scope;
