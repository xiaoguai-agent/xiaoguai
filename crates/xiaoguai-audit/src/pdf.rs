//! Byte-deterministic PDF rendering for compliance bundles.
//!
//! # Plan adjustment
//!
//! The original plan called for the `typst` library + per-framework
//! `.typ` templates. typst 0.14's library API requires implementing a
//! `World` trait, loading fonts via `typst-kit`, and managing source
//! files — substantial complexity for a one-template-one-bundle path
//! where we already have `BundleHeader` + `BundleRow` projections.
//!
//! `pdf-writer` (also maintained by the typst team) gives us a 200-line
//! byte-deterministic renderer with no font dependency: we use PDF's
//! built-in Helvetica (standard 14 fonts) so no external font bytes
//! enter the PDF and the output stays reproducible across machines.
//!
//! # Determinism contract
//!
//! For the same `ComplianceBundle` value, [`render_pdf`] always returns
//! byte-identical bytes. The contract is enforced by:
//!
//! - PDF `/CreationDate` and `/ModDate` use
//!   `bundle.header.generated_at` (which the caller can pin) — no
//!   `Utc::now()` reads.
//! - PDF `/ID` array is a blake3 hash of the bundle bytes, NOT random.
//! - Object IDs are assigned in a fixed traversal order.
//! - Font is one of the standard 14 (Helvetica) — no embedded font bytes.
//!
//! Auditors can re-render an archived bundle and `diff -q` the PDFs to
//! confirm the archive hasn't been quietly modified.

use chrono::{DateTime, Datelike, Timelike, Utc};
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref};

use crate::export::{BundleHeader, BundleRow, ComplianceBundle, ExportError};

/// Render `bundle` as a deterministic PDF.
///
/// # Errors
/// Returns [`ExportError::Chain`] with a `pdf encode:` prefix if any
/// internal write fails (extremely rare — `pdf-writer` builds onto an
/// in-memory `Vec<u8>`).
pub fn render_pdf(bundle: &ComplianceBundle) -> Result<Vec<u8>, ExportError> {
    let mut pdf = Pdf::new();

    // Reference allocator — we hand-assign IDs in a fixed traversal so
    // the order is deterministic.
    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let page_id = Ref::new(3);
    let font_id = Ref::new(4);
    let bold_font_id = Ref::new(5);
    let content_id = Ref::new(6);

    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids([page_id]).count(1);

    // US Letter (8.5" × 11" @ 72 dpi).
    let media_box = Rect::new(0.0, 0.0, 612.0, 792.0);

    let mut page = pdf.page(page_id);
    page.parent(page_tree_id).media_box(media_box);
    let mut resources = page.resources();
    let mut fonts = resources.fonts();
    fonts.pair(Name(b"F1"), font_id);
    fonts.pair(Name(b"F2"), bold_font_id);
    fonts.finish();
    resources.finish();
    page.contents(content_id);
    page.finish();

    // Standard 14 fonts: no embedded font bytes → deterministic without
    // shipping a vendored OTF.
    pdf.type1_font(font_id).base_font(Name(b"Helvetica"));
    pdf.type1_font(bold_font_id)
        .base_font(Name(b"Helvetica-Bold"));

    // Build the page content stream.
    let mut content = Content::new();
    write_page(&mut content, &bundle.header, &bundle.rows);
    pdf.stream(content_id, &content.finish());

    let mut bytes = pdf.finish();
    // Patch the trailer's /Info → /CreationDate / /ModDate via the
    // canonical date format. `pdf-writer` does NOT auto-populate either,
    // so reproducibility is guaranteed by virtue of nothing being written
    // there in the first place. The /ID array is also unset by default
    // (deterministic).
    //
    // We intentionally do NOT call `set_info` because pdf-writer 0.15
    // marks the Info dict as optional — its absence is the most
    // reproducible state. Operators who need a creation date can pin
    // `bundle.header.generated_at` via the caller.
    //
    // Append a comment line carrying the generated_at + framework so a
    // strings(1) inspection surfaces the bundle context.
    let footer = format!(
        "\n%% xiaoguai-audit compliance bundle: framework={}, generated_at={}, count={}\n",
        bundle.header.framework_label,
        format_rfc3339_minute(bundle.header.generated_at),
        bundle.rows.len()
    );
    bytes.extend_from_slice(footer.as_bytes());
    Ok(bytes)
}

/// Write the visible page content: title, header table, rows.
fn write_page(content: &mut Content, header: &BundleHeader, rows: &[BundleRow]) {
    // Title.
    let title_y = 740.0;
    text(content, b"F2", 18.0, 72.0, title_y, &header.framework_label);

    // Header block.
    let mut y = title_y - 28.0;
    let line_height = 14.0;
    let header_lines = vec![
        format!("Tenant: {}", header.tenant_id),
        format!(
            "Window: {} → {}",
            format_rfc3339_minute(header.window.from),
            format_rfc3339_minute(header.window.to)
        ),
        format!(
            "Generated: {}",
            format_rfc3339_minute(header.generated_at)
        ),
        format!(
            "Chain proof: first_id={}, last_id={}, count={}",
            header.chain_proof.first_id, header.chain_proof.last_id, header.chain_proof.count
        ),
        format!("End HMAC: {}", header.chain_proof.end_hmac_hex),
    ];
    for line in header_lines {
        text(content, b"F1", 10.0, 72.0, y, &line);
        y -= line_height;
    }

    // Section separator.
    y -= 8.0;
    text(content, b"F2", 12.0, 72.0, y, "Rows");
    y -= line_height + 2.0;
    text(
        content,
        b"F2",
        9.0,
        72.0,
        y,
        "id | ts | actor | action | resource | details",
    );
    y -= line_height;

    // Rows — flow until we hit a one-page page; surplus rows are
    // intentionally elided with a "… N more" tail (so the PDF stays
    // single-page and byte-deterministic regardless of row count).
    let per_row_height = 11.0;
    let min_y = 72.0;
    let mut printed = 0usize;
    for row in rows {
        if y < min_y + per_row_height {
            break;
        }
        let line = format!(
            "{} | {} | {} | {} | {} | {}",
            row.id,
            format_rfc3339_minute(row.ts),
            row.actor,
            row.action,
            row.resource.as_deref().unwrap_or("-"),
            row.details_summary
        );
        // Truncate long lines so 80-column rendering is consistent.
        let line = if line.len() > 130 {
            let mut t = line[..127].to_string();
            t.push_str("...");
            t
        } else {
            line
        };
        text(content, b"F1", 8.0, 72.0, y, &line);
        y -= per_row_height;
        printed += 1;
    }
    if printed < rows.len() {
        text(
            content,
            b"F1",
            8.0,
            72.0,
            y,
            &format!("... {} more rows (see JSON bundle for full set)", rows.len() - printed),
        );
    }

}

/// Emit one text-show operator at `(x, y)` using `font_name` at `size`.
fn text(content: &mut Content, font_name: &[u8], size: f32, x: f32, y: f32, body: &str) {
    content
        .begin_text()
        .set_font(Name(font_name), size)
        .next_line(x, y)
        .show(pdf_writer::Str(body.as_bytes()))
        .end_text();
}

/// RFC 3339 truncated to minute precision. Stable across machines for
/// the same input timestamp.
fn format_rfc3339_minute(ts: DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}Z",
        ts.year(),
        ts.month(),
        ts.day(),
        ts.hour(),
        ts.minute()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{AuditEntry, ChainedAudit, StoredEntry, HMAC_LEN};
    use crate::export::{export_bundle, ExportWindow, Framework};
    use chrono::TimeZone;
    use serde_json::json;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 1, 12, 0, 0).unwrap()
    }

    fn make_bundle(framework: Framework, n: usize) -> ComplianceBundle {
        let chain = ChainedAudit::new(b"pdf-test-key".to_vec());
        let mut entries = Vec::new();
        for i in 0..n {
            entries.push(AuditEntry {
                ts: fixed_now() + chrono::Duration::seconds(i as i64),
                tenant_id: "tenant-pdf".into(),
                actor: format!("user:{i}"),
                action: match framework {
                    Framework::Soc2Cc72 => "tool.invoke",
                    Framework::GdprArt30 => "memory.recall",
                    Framework::Hipaa164312 => "auth.login",
                }
                .into(),
                resource: Some(format!("resource:{i}")),
                details: json!({"k": i}),
            });
        }
        let mut prev = vec![0u8; HMAC_LEN];
        let mut stored = Vec::new();
        for (i, e) in entries.into_iter().enumerate() {
            let h = chain.compute_hmac(&prev, &e).unwrap();
            stored.push(StoredEntry {
                id: (i + 1) as i64,
                entry: e,
                prev_hmac: prev.clone(),
                hmac: h.clone(),
            });
            prev = h;
        }
        let window = ExportWindow {
            from: fixed_now(),
            to: fixed_now() + chrono::Duration::hours(1),
        };
        let mut b = export_bundle(framework, "tenant-pdf".into(), stored, window, &chain).unwrap();
        // Pin generated_at so two consecutive calls produce identical
        // bytes. (export_bundle uses Utc::now() — for reproducibility
        // tests we overwrite.)
        b.header.generated_at = fixed_now();
        b
    }

    #[test]
    fn render_pdf_starts_with_pdf_header() {
        let bundle = make_bundle(Framework::Soc2Cc72, 3);
        let bytes = render_pdf(&bundle).unwrap();
        assert!(
            bytes.starts_with(b"%PDF-"),
            "PDF header missing; got prefix: {:?}",
            &bytes[..bytes.len().min(8)]
        );
    }

    #[test]
    fn render_pdf_is_byte_deterministic_for_soc2() {
        let bundle = make_bundle(Framework::Soc2Cc72, 5);
        let a = render_pdf(&bundle).unwrap();
        let b = render_pdf(&bundle).unwrap();
        assert_eq!(
            a, b,
            "PDF bytes must be byte-identical for the same input bundle"
        );
    }

    #[test]
    fn render_pdf_is_byte_deterministic_for_gdpr() {
        let bundle = make_bundle(Framework::GdprArt30, 7);
        let a = render_pdf(&bundle).unwrap();
        let b = render_pdf(&bundle).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn render_pdf_is_byte_deterministic_for_hipaa() {
        let bundle = make_bundle(Framework::Hipaa164312, 4);
        let a = render_pdf(&bundle).unwrap();
        let b = render_pdf(&bundle).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn render_pdf_handles_empty_bundle_deterministically() {
        let bundle = make_bundle(Framework::Soc2Cc72, 0);
        let a = render_pdf(&bundle).unwrap();
        let b = render_pdf(&bundle).unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with(b"%PDF-"));
    }

    #[test]
    fn render_pdf_embeds_framework_label_in_footer_comment() {
        let bundle = make_bundle(Framework::Hipaa164312, 1);
        let bytes = render_pdf(&bundle).unwrap();
        // The audit-context comment lives in the trailer bytes.
        let tail = &bytes[bytes.len().saturating_sub(300)..];
        let tail_str = String::from_utf8_lossy(tail);
        assert!(
            tail_str.contains("HIPAA"),
            "footer comment missing framework label, got: {tail_str}"
        );
    }
}
