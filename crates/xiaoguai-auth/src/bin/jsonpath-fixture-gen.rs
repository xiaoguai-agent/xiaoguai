//! sprint-14 S14-2: canonical `JSONPath` redaction fixtures.
//!
//! Emits a single JSON file containing (`jsonpath`, input, `expected_masked`)
//! triples that both the Rust `RedactionRules::apply` tests and the
//! sprint-14 S14-6 frontend parity tests will read. By generating from
//! one source the backend mask and the operator-facing preview can
//! never drift.
//!
//! The expected outputs were verified by hand-running
//! `jsonpath_lib::replace_with(...)` for each case; they are stored as
//! literals here so the fixture file is content-addressable (no runtime
//! engine dependency at generation time).
//!
//! ## Usage
//!
//! ```bash
//! cargo run -p xiaoguai-auth --bin jsonpath-fixture-gen
//! # Writes crates/xiaoguai-auth/tests/jsonpath_fixtures.json relative to
//! # the workspace root. If invoked from a different cwd, pass the path:
//! cargo run -p xiaoguai-auth --bin jsonpath-fixture-gen -- /tmp/out.json
//! ```
//!
//! The mask placeholder matches `redaction::REDACTED_PLACEHOLDER` (`"***"`).

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

/// Canonical mask string — must match `redaction::REDACTED_PLACEHOLDER`.
const MASK: &str = "***";

/// One fixture row.
fn case(name: &str, jsonpaths: &[&str], input: Value, expected: Value, notes: &str) -> Value {
    json!({
        "name": name,
        "jsonpaths": jsonpaths,
        "input": input,
        "expected_masked": expected,
        "notes": notes,
    })
}

fn build_fixtures() -> Value {
    let m = MASK;
    json!({
        "$schema_version": 1,
        "$mask": m,
        "$generator": "xiaoguai-auth/src/bin/jsonpath-fixture-gen.rs",
        "cases": [
            case(
                "top_level_scalar",
                &["$.password"],
                json!({ "password": "x", "other": "y" }),
                json!({ "password": m, "other": "y" }),
                "Top-level field mask; siblings untouched.",
            ),
            case(
                "nested_object",
                &["$.user.email"],
                json!({ "user": { "email": "a@b.com", "name": "Alice" } }),
                json!({ "user": { "email": m, "name": "Alice" } }),
                "Nested-object field mask.",
            ),
            case(
                "array_element",
                &["$.items[0].secret"],
                json!({ "items": [
                    { "secret": "s1", "label": "a" },
                    { "secret": "s2", "label": "b" }
                ] }),
                json!({ "items": [
                    { "secret": m, "label": "a" },
                    { "secret": "s2", "label": "b" }
                ] }),
                "Array index targets exactly one element; others untouched.",
            ),
            case(
                "wildcard_flat",
                &["$.*"],
                json!({ "a": 1, "b": 2, "c": 3 }),
                json!({ "a": m, "b": m, "c": m }),
                "Wildcard on flat object masks every top-level value.",
            ),
            case(
                "recursive_descent",
                &["$..token"],
                json!({
                    "outer": { "token": "t1", "child": { "token": "t2", "ok": 1 } },
                    "token": "t0"
                }),
                json!({
                    "outer": { "token": m, "child": { "token": m, "ok": 1 } },
                    "token": m
                }),
                "Recursive descent matches every `token` at any depth.",
            ),
            case(
                "non_matching_path",
                &["$.does_not_exist"],
                json!({ "a": 1, "b": 2 }),
                json!({ "a": 1, "b": 2 }),
                "No-op when the path doesn't match anything.",
            ),
            case(
                "empty_input",
                &["$.anything"],
                json!({}),
                json!({}),
                "Empty object: no fields to mask, no-op.",
            ),
            case(
                "two_paths_simultaneously",
                &["$.a", "$.b"],
                json!({ "a": 1, "b": 2, "c": 3 }),
                json!({ "a": m, "b": m, "c": 3 }),
                "Apply two paths in sequence; both fields masked, `c` preserved.",
            ),
            case(
                "array_wildcard_field",
                &["$.users[*].api_key"],
                json!({ "users": [
                    { "name": "a", "api_key": "k1" },
                    { "name": "b", "api_key": "k2" }
                ] }),
                json!({ "users": [
                    { "name": "a", "api_key": m },
                    { "name": "b", "api_key": m }
                ] }),
                "Wildcard array index + field selector masks every element's field.",
            ),
            case(
                "deeply_nested",
                &["$.a.b.c.d.password"],
                json!({ "a": { "b": { "c": { "d": { "password": "p", "other": "o" } } } } }),
                json!({ "a": { "b": { "c": { "d": { "password": m, "other": "o" } } } } }),
                "Mask survives arbitrarily deep nesting.",
            ),
        ]
    })
}

/// Default output path relative to the workspace root.
const DEFAULT_OUT: &str = "crates/xiaoguai-auth/tests/jsonpath_fixtures.json";

fn main() -> Result<(), Box<dyn Error>> {
    let out: PathBuf = env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(DEFAULT_OUT), PathBuf::from);

    let fixtures = build_fixtures();
    let pretty = serde_json::to_string_pretty(&fixtures)?;
    // Ensure trailing newline for git-friendliness.
    let mut bytes = pretty.into_bytes();
    bytes.push(b'\n');

    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(&out, &bytes)?;
    eprintln!("wrote {} ({} bytes)", out.display(), bytes.len());
    Ok(())
}
