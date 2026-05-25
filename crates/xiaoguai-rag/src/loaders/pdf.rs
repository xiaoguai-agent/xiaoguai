//! PDF loader — extracts embedded text via [`pdf_extract`].
//!
//! Text-only PDFs (Type1 / TrueType fonts with `ToUnicode` maps) work well.
//! Scanned PDFs (image-only) return an empty text string; OCR is deferred
//! to v1.3.
//!
//! Per-page breakdown is derived from the page objects in the PDF; each
//! [`PageMeta`] contains the text extracted from that page.

use std::collections::HashMap;

use super::{LoadError, LoadResult, LoadedDoc, Loader, PageMeta};

/// Stateless PDF loader. Create once, call [`Loader::load`] many times.
#[derive(Debug, Default, Clone)]
pub struct PdfLoader;

impl PdfLoader {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Loader for PdfLoader {
    fn load(&self, bytes: &[u8]) -> LoadResult {
        // pdf_extract expects an in-memory buffer via its Output trait.
        // We collect all page texts via the per-page API.
        let bytes_owned = bytes.to_vec();

        // Use the per-page variant to get a Vec<String> directly.
        let page_texts_vec: Vec<String> = {
            match pdf_extract::extract_text_from_mem_by_pages(&bytes_owned) {
                Ok(pages) => pages
                    .into_iter()
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect(),
                Err(e) => {
                    return Err(LoadError::Malformed {
                        format: "pdf",
                        reason: e.to_string(),
                    });
                }
            }
        };

        let pages: Vec<PageMeta> = page_texts_vec
            .iter()
            .enumerate()
            .map(|(i, t)| PageMeta {
                page_number: u32::try_from(i + 1).unwrap_or(u32::MAX),
                text: t.trim().to_string(),
            })
            .collect();

        let full_text = pages
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let mut metadata = HashMap::new();
        metadata.insert("page_count".to_string(), pages.len().to_string());
        metadata.insert("format".to_string(), "pdf".to_string());

        Ok(LoadedDoc {
            text: full_text,
            pages,
            metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/loaders")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
    }

    #[test]
    fn simple_pdf_extracts_text() {
        let bytes = fixture("sample_simple.pdf");
        let doc = PdfLoader::new().load(&bytes).expect("load ok");
        assert!(!doc.text.is_empty(), "text must not be empty");
        assert!(
            doc.text.to_lowercase().contains("quick brown fox"),
            "expected phrase not found in: {:?}",
            doc.text
        );
    }

    #[test]
    fn multipage_pdf_has_multiple_pages() {
        let bytes = fixture("sample_multipage.pdf");
        let doc = PdfLoader::new().load(&bytes).expect("load ok");
        assert!(
            doc.pages.len() >= 2,
            "expected >= 2 pages, got {}",
            doc.pages.len()
        );
        assert!(
            doc.text.to_lowercase().contains("lorem ipsum"),
            "expected 'lorem ipsum' in: {:?}",
            doc.text
        );
    }

    #[test]
    fn table_pdf_extracts_cell_text() {
        let bytes = fixture("sample_table.pdf");
        let doc = PdfLoader::new().load(&bytes).expect("load ok");
        // The table fixture has "Alice" and "Bob" as row labels.
        let lower = doc.text.to_lowercase();
        assert!(
            lower.contains("alice") || lower.contains("bob") || lower.contains("table"),
            "expected table content in: {:?}",
            doc.text
        );
    }

    #[test]
    fn malformed_bytes_return_error() {
        let err = PdfLoader::new()
            .load(b"this is not a pdf")
            .expect_err("must error on junk");
        assert!(matches!(err, LoadError::Malformed { .. }));
    }

    #[test]
    fn metadata_includes_page_count() {
        let bytes = fixture("sample_multipage.pdf");
        let doc = PdfLoader::new().load(&bytes).expect("load ok");
        assert!(doc.metadata.contains_key("page_count"));
        let count: usize = doc.metadata["page_count"].parse().unwrap();
        assert_eq!(count, doc.pages.len());
    }
}
