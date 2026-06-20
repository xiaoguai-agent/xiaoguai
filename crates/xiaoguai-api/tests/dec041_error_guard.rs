//! DEC-041 source guard: every HTTP handler maps onto the single canonical
//! [`crate::error::ApiError`] (uniform `{code, message}` envelope).
//!
//! Runs in the normal `cargo test` job (no CI workflow change needed) and fails
//! if any handler file reintroduces a per-module error type or envelope helper —
//! the exact "every feature rolls its own error shape" scatter the consolidation
//! removed. Scans the WHOLE `src/` tree (Phase A.2: not just `src/routes/`), so
//! a handler added directly under `src/` (skills/marketplace/workspaces/…) is
//! covered too. `error.rs` — the canonical definition home — is skipped.
//!
//! Scope: the two unambiguous, zero-false-positive smells. The broader
//! "no raw `{\"error\"}` envelope" rule is documented in DEC-041 and enforced
//! in review; intentional structured errors (the `hotl_decisions` rename
//! diagnostic `{error, field, message}` and the audit-export domain codes) are
//! deliberate, documented exceptions and keep their own shape.

use std::fs;
use std::path::{Path, PathBuf};

/// Recursively collect every `.rs` file under `dir`.
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Strip leading item qualifiers (`pub`, `pub(crate)`, `pub(super)`, `async`,
/// `const`, `unsafe`) from a source line so the smell match also catches
/// qualified forms like `pub(crate) struct ApiError` or
/// `pub async fn err_response(` — not just the bare declarations.
fn strip_item_quals(line: &str) -> &str {
    const QUALS: [&str; 6] = [
        "pub(crate)",
        "pub(super)",
        "pub",
        "async",
        "const",
        "unsafe",
    ];
    let mut rest = line.trim_start();
    loop {
        // Only strip a qualifier when it ends at a real token boundary, so we
        // never chop the prefix off an identifier (e.g. `pub` in `publish`).
        let next = QUALS.iter().find_map(|q| {
            let s = rest.strip_prefix(q)?;
            (s.is_empty() || s.starts_with(char::is_whitespace) || s.starts_with('(')).then_some(s)
        });
        match next {
            Some(s) => rest = s.trim_start(),
            None => return rest,
        }
    }
}

#[test]
fn handlers_use_canonical_api_error() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs(&src, &mut files);

    let mut offenders = Vec::new();
    for path in &files {
        // error.rs is the canonical home of `ApiError` — skip it.
        if path.file_name().and_then(|n| n.to_str()) == Some("error.rs") {
            continue;
        }
        let rel = path
            .strip_prefix(&src)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(path).expect("read source file");
        for (i, line) in text.lines().enumerate() {
            let normalized = strip_item_quals(line);
            if normalized.starts_with("struct ApiError") {
                offenders.push(format!(
                    "{rel}:{}: redefines `struct ApiError` — use crate::error::ApiError",
                    i + 1
                ));
            }
            if normalized.starts_with("fn err_response(") {
                offenders.push(format!(
                    "{rel}:{}: ad-hoc `err_response` envelope helper — return crate::error::ApiError",
                    i + 1
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "DEC-041: handlers must use the canonical crate::error::ApiError, not an \
         ad-hoc error envelope. Offenders:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn strip_item_quals_catches_qualified_forms() {
    // Bare *and* qualified declarations normalize to the same smell prefix.
    for line in [
        "struct ApiError {",
        "    pub struct ApiError {",
        "pub(crate) struct ApiError {",
        "    pub(super) struct ApiError;",
    ] {
        assert!(
            strip_item_quals(line).starts_with("struct ApiError"),
            "guard should catch: {line:?}"
        );
    }
    for line in [
        "fn err_response(",
        "    pub fn err_response(",
        "pub(crate) async fn err_response(",
        "    pub async fn err_response(",
    ] {
        assert!(
            strip_item_quals(line).starts_with("fn err_response("),
            "guard should catch: {line:?}"
        );
    }
    // Must NOT chop a qualifier prefix off an unrelated identifier.
    assert!(strip_item_quals("publish_event(&self)").starts_with("publish_event"));
    assert!(strip_item_quals("constants_table()").starts_with("constants_table"));
}
