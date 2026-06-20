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
            let trimmed = line.trim_start();
            if trimmed.starts_with("struct ApiError") {
                offenders.push(format!(
                    "{rel}:{}: redefines `struct ApiError` — use crate::error::ApiError",
                    i + 1
                ));
            }
            if trimmed.starts_with("fn err_response(") {
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
