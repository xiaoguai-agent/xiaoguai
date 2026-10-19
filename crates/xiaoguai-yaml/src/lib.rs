//! YAML (de)serialization for Xiaoguai.
//!
//! Every crate parses and renders YAML through this one module so the parser
//! policy lives in a single place. The backend is `serde-saphyr` (safe Rust,
//! `#![forbid(unsafe_code)]`); it replaced the deprecated `serde_yaml`, whose
//! `unsafe-libyaml` dependency is unmaintained (RUSTSEC-2024-0449).
//!
//! The policy below keeps the behaviour pack, eval and skill YAML already
//! relies on from `serde_yaml` (YAML 1.2 core schema):
//!
//! - **Strict booleans** — only `true` / `false` are booleans. YAML 1.1 forms
//!   (`yes`, `no`, `on`, `off`, `y`, `n`) stay strings when the target is a
//!   string or an untyped value, and are rejected for a `bool` field.
//! - **Non-finite floats pass through** — `.inf` / `.nan` reach the visitor, so
//!   an untyped `serde_json::Value` gets `null`, as it did before, instead of a
//!   hard error.
//!
//! Deliberate differences from `serde_yaml`, kept because they are safer:
//!
//! - **Duplicate mapping keys are an error** everywhere. `serde_yaml` rejected
//!   them for struct fields but let the last one win in an untyped map.
//! - **Merge keys (`<<: *anchor`) are expanded.** `serde_yaml` 0.9 left `<<`
//!   as a literal key.
//! - **Errors are one line**, with a location and no source snippet, so YAML
//!   content is not copied into logs or API responses.
//!
//! Known differences that no option controls (none occur in the shipped
//! packs, eval suites or examples):
//!
//! - A quoted scalar is coerced to a typed `bool` / integer field
//!   (`n: '5'` gives `5`); `serde_yaml` rejected it.
//! - In an untyped value, `017` and `1_000` become numbers (`17.0`, `1000`)
//!   where `serde_yaml` kept them as strings — quote them to keep a string.
//! - Unknown tags (`!custom 5`) are ignored rather than rejected.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_saphyr::options::NonFiniteFloatPolicy;

/// Errors from parsing or rendering YAML.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The input was not valid YAML or did not match the target type.
    ///
    /// Displays as one line with the location and no source snippet: these
    /// messages reach logs, CLI output and API responses, and a snippet would
    /// copy document content into all of them. The parser error is boxed
    /// (it is large) and deliberately not exposed as `source()`, whose
    /// developer-oriented text would otherwise appear in `{:#}` error chains.
    #[error("{}", render(.0))]
    Deserialize(Box<serde_saphyr::Error>),
    /// The value could not be rendered as YAML.
    #[error(transparent)]
    Serialize(#[from] serde_saphyr::SerializeError),
}

fn render(err: &serde_saphyr::Error) -> String {
    err.render_with_options(serde_saphyr::render_options! {
        formatter: &serde_saphyr::UserMessageFormatter,
        snippets: serde_saphyr::SnippetMode::Off,
    })
}

/// Parse a YAML document into `T`.
///
/// # Errors
/// Returns [`Error::Deserialize`] if `input` is not valid YAML, has duplicate
/// keys, or does not match `T`.
pub fn from_str<T: DeserializeOwned>(input: &str) -> Result<T, Error> {
    let options = serde_saphyr::options! {
        strict_booleans: true,
        with_snippet: false,
        non_finite_float_policy: NonFiniteFloatPolicy::PassThrough,
    };
    serde_saphyr::from_str_with_options(input, options).map_err(|e| Error::Deserialize(Box::new(e)))
}

/// Render `value` as a YAML document.
///
/// # Errors
/// Returns [`Error::Serialize`] if `value` cannot be represented in YAML.
pub fn to_string<T: Serialize>(value: &T) -> Result<String, Error> {
    Ok(serde_saphyr::to_string(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::{json, Value};

    fn value(yaml: &str) -> Value {
        from_str(yaml).expect("valid yaml")
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Flag {
        on: bool,
    }

    #[test]
    fn yaml_1_1_booleans_stay_strings_in_untyped_values() {
        for word in ["yes", "no", "on", "off", "y", "n"] {
            assert_eq!(value(&format!("a: {word}")), json!({ "a": word }), "{word}");
        }
        assert_eq!(value("a: true\nb: False"), json!({ "a": true, "b": false }));
    }

    #[test]
    fn yaml_1_1_booleans_are_rejected_for_bool_fields() {
        assert!(from_str::<Flag>("on: yes").is_err());
        assert_eq!(from_str::<Flag>("on: true").unwrap(), Flag { on: true });
    }

    #[test]
    fn non_finite_floats_become_null_in_untyped_values() {
        assert_eq!(value("a: .inf\nb: .nan"), json!({ "a": null, "b": null }));
    }

    #[test]
    fn non_finite_floats_reach_typed_f64_fields() {
        #[derive(Deserialize)]
        struct F {
            x: f64,
        }
        assert!(from_str::<F>("x: .inf").unwrap().x.is_infinite());
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        assert!(from_str::<Value>("a: 1\na: 2").is_err());
    }

    #[test]
    fn merge_keys_are_expanded() {
        let v = value("base: &b {k: 1, j: 2}\nc:\n  <<: *b\n  j: 3");
        assert_eq!(v["c"], json!({ "k": 1, "j": 3 }));
    }

    #[test]
    fn core_schema_scalars_resolve_as_before() {
        assert_eq!(
            value("a: ~\nb: 0x1F\nc: 0o17\nd: 1e3\ne: '1'\nf: 2026-01-01\ng:"),
            json!({ "a": null, "b": 31, "c": 15, "d": 1000.0, "e": "1", "f": "2026-01-01", "g": null })
        );
    }

    #[test]
    fn rendered_ambiguous_strings_are_quoted_and_round_trip() {
        #[derive(Debug, Deserialize, Serialize, PartialEq)]
        struct S {
            a: String,
            b: String,
            c: String,
        }
        let s = S {
            a: "yes".into(),
            b: "017".into(),
            c: "a: b # c".into(),
        };
        let out = to_string(&s).unwrap();
        assert!(!out.contains("a: yes"), "unquoted yes in {out}");
        assert_eq!(from_str::<S>(&out).unwrap(), s);
    }

    #[test]
    fn errors_are_one_line_with_a_location_and_no_source_snippet() {
        let err = from_str::<Flag>("secret_token: hunter2\non: [")
            .unwrap_err()
            .to_string();
        assert!(err.contains("line 2"), "{err}");
        assert!(!err.contains('\n'), "multi-line: {err}");
        assert!(!err.contains("hunter2"), "source leaked into: {err}");
    }

    #[test]
    fn errors_expose_no_developer_oriented_source() {
        use std::error::Error as _;
        let err = from_str::<Value>("a: 1\na: 2").unwrap_err();
        assert!(err.source().is_none());
        assert!(!err.to_string().contains("DuplicateKeyPolicy"), "{err}");
    }

    #[test]
    fn missing_fields_are_named() {
        let err = from_str::<Flag>("{}").unwrap_err().to_string();
        assert!(err.contains("missing field `on`"), "{err}");
    }
}
