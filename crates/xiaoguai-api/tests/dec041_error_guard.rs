//! DEC-041 source guard: every `/v1` route handler maps onto the single
//! canonical [`crate::error::ApiError`] (uniform `{code, message}` envelope).
//!
//! This runs in the normal `cargo test` job (no CI workflow change needed) and
//! fails if a route file reintroduces a per-module error type or envelope
//! helper — the exact "every feature rolls its own error shape" scatter the
//! consolidation removed.
//!
//! Scope: the two unambiguous, zero-false-positive smells. The broader
//! "no raw `{\"error\"}` envelope" rule is documented in DEC-041 and enforced
//! in review; intentional structured errors (the `hotl_decisions` rename
//! diagnostic `{error, field, message}` and the audit-export domain codes) are
//! deliberate, documented exceptions and keep their own shape.

use std::fs;
use std::path::Path;

#[test]
fn routes_use_canonical_api_error() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/routes");
    let mut offenders = Vec::new();

    for entry in fs::read_dir(&dir).expect("read src/routes") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let src = fs::read_to_string(&path).expect("read route file");
        for (i, line) in src.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("struct ApiError") {
                offenders.push(format!(
                    "{file}:{}: redefines `struct ApiError` — use crate::error::ApiError",
                    i + 1
                ));
            }
            if trimmed.starts_with("fn err_response(") {
                offenders.push(format!(
                    "{file}:{}: ad-hoc `err_response` envelope helper — return crate::error::ApiError",
                    i + 1
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "DEC-041: route handlers must use the canonical crate::error::ApiError, not an \
         ad-hoc error envelope. Offenders:\n{}",
        offenders.join("\n")
    );
}
