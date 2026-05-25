//! Document-extraction adapters for the RAG ingest pipeline.
//!
//! Each loader converts raw bytes of a specific document format into a
//! [`LoadedDoc`], which the ingest pipeline can then chunk, embed, and
//! index via any [`crate::RagClient`] backend.
//!
//! # Loader selection
//!
//! The recommended entry point is [`detect::detect`], which sniffs the
//! magic bytes of the input and falls back to the file extension. It
//! returns a [`LoadedDoc`] directly without the caller needing to know
//! which loader ran.
//!
//! # Out of scope (v1.3)
//!
//! OCR for scanned PDFs. [`pdf::PdfLoader`] extracts embedded text only;
//! image-only PDFs return an empty [`LoadedDoc::text`].

use std::collections::HashMap;

use thiserror::Error;

pub mod detect;
pub mod docx;
pub mod html;
pub mod markdown;
pub mod pdf;
pub mod pptx;

// ── Public re-exports ───────────────────────────────────────────────────────

pub use detect::detect;
pub use docx::DocxLoader;
pub use html::HtmlLoader;
pub use markdown::MarkdownLoader;
pub use pdf::PdfLoader;
pub use pptx::PptxLoader;

// ── Core types ──────────────────────────────────────────────────────────────

/// Per-page metadata attached to each extracted page / slide.
#[derive(Debug, Clone, PartialEq)]
pub struct PageMeta {
    /// 1-indexed page / slide number.
    pub page_number: u32,
    /// Extracted text for this page, with embedded newlines preserved.
    pub text: String,
}

/// The output of a [`Loader`] call — document text + per-page breakdown +
/// document-level metadata.
#[derive(Debug, Clone)]
pub struct LoadedDoc {
    /// Full concatenated text of all pages / slides, separated by `\n\n`.
    pub text: String,
    /// Per-page / per-slide breakdown. Always in ascending page-number order.
    pub pages: Vec<PageMeta>,
    /// Document-level metadata (title, author, headings, etc.).
    /// Keys are lowercase, kebab-case strings (e.g. `"title"`, `"headings"`).
    pub metadata: HashMap<String, String>,
}

/// Errors returned by document loaders.
#[derive(Debug, Error)]
pub enum LoadError {
    /// The input bytes are not a valid document of the expected format.
    #[error("malformed {format}: {reason}")]
    Malformed {
        format: &'static str,
        reason: String,
    },
    /// An I/O error occurred while reading the document (e.g. ZIP entry).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The format is valid but no text could be extracted (e.g. image-only PDF).
    #[error("no extractable text in {format}")]
    NoText { format: &'static str },
}

pub type LoadResult = Result<LoadedDoc, LoadError>;

/// Synchronous document loader trait.
///
/// Loaders are inherently CPU-bound (parsing, decompression) and do not
/// perform I/O beyond what is contained in the input `bytes`. They are
/// therefore sync, not async. Callers in async contexts should wrap with
/// [`tokio::task::spawn_blocking`] if they need to loader large documents
/// without blocking the executor.
pub trait Loader: Send + Sync {
    /// Extract text and metadata from `bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`LoadError::Malformed`] if the bytes are not a valid
    /// document of this format, or [`LoadError::NoText`] if the document
    /// contains no extractable text (e.g. image-only PDF).
    fn load(&self, bytes: &[u8]) -> LoadResult;
}
