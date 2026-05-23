//! Chunking presets — hides backend-specific knobs from operators.
//!
//! Roadmap principle: "give presets, not a settings ocean". Three
//! presets cover ~90% of real-world content types; advanced users can
//! still pass raw config to the backend if they really must.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChunkingPreset {
    /// 512-token chunks, 64-token overlap. Right for prose, markdown
    /// notes, knowledge-base articles.
    TextDefault,
    /// AST-aware splitting on function / class boundaries when the
    /// backend supports it, else 256-token chunks. Right for source
    /// code repositories.
    CodeAware,
    /// 1024-token chunks, 128-token overlap, page-anchored citations.
    /// Right for academic / report PDFs.
    PdfHeavy,
}

impl Default for ChunkingPreset {
    fn default() -> Self {
        Self::TextDefault
    }
}

impl ChunkingPreset {
    /// Render the preset as the JSON envelope R2R's `/v3/documents`
    /// endpoint expects. Other backends slot their own translation in
    /// their `ingest` impl. Kept here so the JSON shape lives in one
    /// place.
    #[must_use]
    pub fn r2r_chunking_config(self) -> serde_json::Value {
        match self {
            Self::TextDefault => serde_json::json!({
                "chunk_size": 512,
                "chunk_overlap": 64,
                "strategy": "by_title",
            }),
            Self::CodeAware => serde_json::json!({
                "chunk_size": 256,
                "chunk_overlap": 32,
                "strategy": "by_function",
            }),
            Self::PdfHeavy => serde_json::json!({
                "chunk_size": 1024,
                "chunk_overlap": 128,
                "strategy": "by_page",
            }),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngestOptions {
    pub preset: ChunkingPreset,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_render_distinct_r2r_configs() {
        let a = ChunkingPreset::TextDefault.r2r_chunking_config();
        let b = ChunkingPreset::CodeAware.r2r_chunking_config();
        let c = ChunkingPreset::PdfHeavy.r2r_chunking_config();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_eq!(a["chunk_size"], 512);
        assert_eq!(c["strategy"], "by_page");
    }
}
