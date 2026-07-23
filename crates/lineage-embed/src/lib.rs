//! Local, keyless text embedding for lineage retrieval.
//!
//! The [`TextEmbedder`] trait is the seam: the dense retriever depends on it,
//! not on any particular model runtime. The fastembed/ONNX implementation lives
//! behind the `dense` feature so a lexical-only build carries none of its weight
//! (the ONNX runtime + tokenizers + hf-hub are a large transitive tree). A
//! future static-embedding or server-side embedder implements the same trait.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("embedder unavailable: {0}")]
    Unavailable(String),

    #[error("embedding failed: {0}")]
    Embed(String),
}

pub type Result<T> = std::result::Result<T, EmbedError>;

/// Query and document text are embedded through separate methods because
/// asymmetric models (BGE/E5/Jina families) expect different instruction
/// prefixes on each side, and omitting the trained prefix silently degrades
/// retrieval (context-injection gotcha R2.1). Keeping the two sides distinct in
/// the trait means the prefix decision lives with the model, not the caller.
pub trait TextEmbedder {
    /// Dimensionality of the vectors this embedder produces. Storage and the
    /// cosine math need it up front, and a corpus embedded at one dimension
    /// must never be compared against a query at another.
    fn dimensions(&self) -> usize;

    /// Embed corpus documents (chunks). Returned vectors are L2-normalized so a
    /// dot product is cosine similarity — the retriever relies on this.
    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Embed a search query. Separate from documents so the query-side prefix
    /// is applied; see the trait note.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
}

#[cfg(feature = "dense")]
mod fastembed_impl;

#[cfg(feature = "dense")]
pub use fastembed_impl::FastEmbedder;
