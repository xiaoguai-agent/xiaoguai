//! Auto-detection of document format from magic bytes + file extension.
//!
//! [`detect`] is the recommended entry point for callers that do not know
//! the format in advance. It:
//!
//! 1. Sniffs the first few bytes against known magic byte sequences.
//! 2. Falls back to the file extension (case-insensitive) if the magic
//!    bytes are ambiguous or absent (e.g. plain-text formats).
//! 3. Returns a [`LoadedDoc`] or a [`LoadError`] — the caller never needs
//!    to know which concrete loader ran.
//!
//! # Supported formats
//!
//! | Magic bytes               | Extension(s)        | Loader        |
//! |---------------------------|---------------------|---------------|
//! | `%PDF`                    | `.pdf`              | [`PdfLoader`] |
//! | `PK\x03\x04` + DOCX hint  | `.docx`             | [`DocxLoader`]|
//! | `PK\x03\x04` + PPTX hint  | `.pptx`             | [`PptxLoader`]|
//! | `PK\x03\x04` (generic ZIP)| `.docx` / `.pptx`  | via extension |
//! | `<!DOCTYPE` / `<html`     | `.html` / `.htm`    | [`HtmlLoader`]|
//! | UTF-8 text with `---`     | `.md` / `.markdown` | [`MarkdownLoader`]|
//! | (none)                    | `.md` / `.markdown` | [`MarkdownLoader`]|
//! | (none)                    | `.html` / `.htm`    | [`HtmlLoader`] |

use std::io::Cursor;

use super::{
    DocxLoader, HtmlLoader, LoadError, LoadResult, Loader, MarkdownLoader, PdfLoader, PptxLoader,
};

// Magic byte sequences.
const PDF_MAGIC: &[u8] = b"%PDF";
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
const HTML_DOCTYPE: &[u8] = b"<!DOCTYPE";
const HTML_TAG: &[u8] = b"<html";

/// Detect the format of `bytes` and extract text using the appropriate loader.
///
/// `hint_ext` is an optional file extension (with or without leading `.`) used
/// as a tiebreaker when the magic bytes alone are ambiguous (e.g. ZIP-based
/// formats like DOCX vs PPTX).
///
/// # Errors
///
/// Returns [`LoadError::Malformed`] if no format could be detected or the
/// detected loader fails. Returns [`LoadError::NoText`] if the document
/// contains no extractable text.
pub fn detect(bytes: &[u8], hint_ext: Option<&str>) -> LoadResult {
    let ext = hint_ext
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .unwrap_or_default();

    // PDF — first 4 bytes are `%PDF`.
    if bytes.starts_with(PDF_MAGIC) {
        return PdfLoader::new().load(bytes);
    }

    // ZIP container (DOCX, PPTX, XLSX …).
    if bytes.starts_with(ZIP_MAGIC) {
        return dispatch_zip(bytes, &ext);
    }

    // HTML — look for DOCTYPE or <html at the very start (allow BOM / whitespace).
    let start = bytes
        .iter()
        .position(|&b| !b.is_ascii_whitespace())
        .unwrap_or(0);
    let trimmed = &bytes[start..];
    let upper: Vec<u8> = trimmed.iter().map(u8::to_ascii_uppercase).collect();
    if upper.starts_with(HTML_DOCTYPE)
        || upper.starts_with(HTML_TAG)
        || ext == "html"
        || ext == "htm"
    {
        return HtmlLoader::new().load(bytes);
    }

    // Markdown — no magic bytes; rely on extension.
    if matches!(ext.as_str(), "md" | "markdown" | "mdx" | "mdown") {
        return MarkdownLoader::new().load(bytes);
    }

    // Last-resort: if the bytes look like UTF-8 text, try Markdown.
    if std::str::from_utf8(bytes).is_ok() && !bytes.is_empty() {
        return MarkdownLoader::new().load(bytes);
    }

    Err(LoadError::Malformed {
        format: "unknown",
        reason: format!(
            "could not detect document format (ext={ext:?}, first 4 bytes={:02X?})",
            &bytes[..bytes.len().min(4)]
        ),
    })
}

/// Dispatch a ZIP-magic document to DOCX or PPTX based on internal entries.
fn dispatch_zip(bytes: &[u8], ext: &str) -> LoadResult {
    // Peek at the ZIP central directory to decide between DOCX and PPTX.
    let cursor = Cursor::new(bytes);
    let archive_result = zip::ZipArchive::new(cursor);

    let format = match archive_result {
        Err(_) => {
            return Err(LoadError::Malformed {
                format: "zip",
                reason: "ZIP container is corrupt".into(),
            });
        }
        Ok(mut archive) => {
            // Look for characteristic entries.
            let has_word_doc = (0..archive.len()).any(|i| {
                archive
                    .by_index(i)
                    .map(|e| e.name().starts_with("word/"))
                    .unwrap_or(false)
            });
            let has_ppt = (0..archive.len()).any(|i| {
                archive
                    .by_index(i)
                    .map(|e| e.name().starts_with("ppt/"))
                    .unwrap_or(false)
            });

            if has_word_doc {
                "docx"
            } else if has_ppt {
                "pptx"
            } else {
                // Fall back to extension.
                match ext {
                    "docx" => "docx",
                    "pptx" => "pptx",
                    _ => {
                        return Err(LoadError::Malformed {
                            format: "zip",
                            reason: "ZIP container is not a recognized Office document".into(),
                        });
                    }
                }
            }
        }
    };

    match format {
        "docx" => DocxLoader::new().load(bytes),
        "pptx" => PptxLoader::new().load(bytes),
        _ => unreachable!(),
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
    fn detects_pdf_by_magic_bytes() {
        let bytes = fixture("sample_simple.pdf");
        // No extension hint — must detect via `%PDF` magic.
        let doc = detect(&bytes, None).expect("PDF detected");
        assert!(
            doc.metadata.get("format").map(String::as_str) == Some("pdf"),
            "wrong format: {:?}",
            doc.metadata
        );
    }

    #[test]
    fn detects_docx_by_zip_content() {
        let bytes = fixture("sample.docx");
        // No extension hint — must detect via ZIP + word/ entries.
        let doc = detect(&bytes, None).expect("DOCX detected");
        assert_eq!(doc.metadata.get("format").map(String::as_str), Some("docx"));
    }

    #[test]
    fn detects_pptx_by_zip_content() {
        let bytes = fixture("sample.pptx");
        // No extension hint — must detect via ZIP + ppt/ entries.
        let doc = detect(&bytes, None).expect("PPTX detected");
        assert_eq!(doc.metadata.get("format").map(String::as_str), Some("pptx"));
    }

    #[test]
    fn detects_html_by_doctype() {
        let bytes = fixture("sample.html");
        let doc = detect(&bytes, None).expect("HTML detected");
        assert_eq!(doc.metadata.get("format").map(String::as_str), Some("html"));
    }

    #[test]
    fn detects_markdown_by_extension() {
        let bytes = fixture("sample.md");
        let doc = detect(&bytes, Some("md")).expect("Markdown detected");
        assert_eq!(
            doc.metadata.get("format").map(String::as_str),
            Some("markdown")
        );
    }

    #[test]
    fn detect_docx_with_extension_hint() {
        let bytes = fixture("sample.docx");
        let doc = detect(&bytes, Some(".docx")).expect("DOCX via hint");
        assert_eq!(doc.metadata.get("format").map(String::as_str), Some("docx"));
    }

    #[test]
    fn detect_pptx_with_extension_hint() {
        let bytes = fixture("sample.pptx");
        let doc = detect(&bytes, Some(".pptx")).expect("PPTX via hint");
        assert_eq!(doc.metadata.get("format").map(String::as_str), Some("pptx"));
    }

    #[test]
    fn detect_pdf_with_extension_hint() {
        let bytes = fixture("sample_multipage.pdf");
        let doc = detect(&bytes, Some("pdf")).expect("PDF via hint");
        assert_eq!(doc.metadata.get("format").map(String::as_str), Some("pdf"));
    }
}
