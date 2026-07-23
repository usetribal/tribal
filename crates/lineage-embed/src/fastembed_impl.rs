use std::cell::RefCell;
use std::path::PathBuf;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::{EmbedError, Result, TextEmbedder};

/// Dimensionality of `jina-embeddings-v2-base-code` output.
const JINA_CODE_DIM: usize = 768;

/// Chunks per embedding batch. Bounds peak memory: without a batch size
/// fastembed materializes activations for every chunk at once, which on a
/// many-chunk session runs to many gigabytes. A modest batch keeps memory flat
/// at a negligible throughput cost on CPU.
const EMBED_BATCH_SIZE: usize = 32;

/// Default ONNX intra-op thread cap (the query/hook path). The ORT default
/// spawns one thread per core; for a small model that adds contention, not
/// speed, and the hook must not monopolize a box the user is actively coding on.
const DEFAULT_INTRA_THREADS: usize = 4;

/// Thread cap for a backfill: the user is waiting on it, so using more of the
/// box is the right trade there (unlike the hook path). Still bounded so it does
/// not spawn one-per-core on a 100-core machine.
const BACKFILL_INTRA_THREADS: usize = 16;

/// fastembed-backed embedder using `jina-embeddings-v2-base-code` — a
/// code-trained model, because a general-text embedder on code-session
/// transcripts is out-of-distribution and can underperform BM25 (gotcha R2.4).
///
/// Unlike the BGE/E5 families, the Jina v2 code model is **symmetric**: it was
/// not trained with `query:`/`passage:` instruction prefixes, so both sides are
/// embedded verbatim. Adding a prefix here would hurt, which is why the
/// query/document split in the trait maps to identical handling for this model
/// (the split still matters for a future asymmetric embedder — gotcha R2.1).
pub struct FastEmbedder {
    // fastembed's `embed` takes `&mut self` (it runs the ONNX session), but the
    // retriever holds the embedder behind a shared `&`. A RefCell keeps the
    // mutation internal; the embedder is single-threaded per hook process.
    model: RefCell<TextEmbedding>,
}

impl FastEmbedder {
    /// Loads the model for the latency-sensitive path (query/hook), with a
    /// conservative thread cap. Downloads the model once into `cache_dir` if
    /// absent — this can touch the network on first use, so callers on the hook
    /// path must treat construction as fallible and fail open, never block the
    /// agent on a download (context-injection: fail open).
    pub fn new(cache_dir: PathBuf) -> Result<Self> {
        Self::with_threads(cache_dir, DEFAULT_INTRA_THREADS)
    }

    /// Loads the model tuned for a backfill (more threads) — use when the user
    /// is explicitly waiting on a bulk embed rather than a single query.
    pub fn new_for_backfill(cache_dir: PathBuf) -> Result<Self> {
        Self::with_threads(cache_dir, BACKFILL_INTRA_THREADS)
    }

    fn with_threads(cache_dir: PathBuf, intra_threads: usize) -> Result<Self> {
        let options = InitOptions::new(EmbeddingModel::JinaEmbeddingsV2BaseCode)
            .with_cache_dir(cache_dir)
            // No progress bar: this runs inside a hook, not an interactive shell.
            .with_show_download_progress(false)
            .with_intra_threads(intra_threads);
        let model =
            TextEmbedding::try_new(options).map_err(|e| EmbedError::Unavailable(e.to_string()))?;
        Ok(Self {
            model: RefCell::new(model),
        })
    }

    fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        // fastembed L2-normalizes every output vector, so a dot product of two
        // results is their cosine similarity — the retriever depends on this.
        // A batch size bounds peak memory (see `EMBED_BATCH_SIZE`).
        self.model
            .borrow_mut()
            .embed(texts, Some(EMBED_BATCH_SIZE))
            .map_err(|e| EmbedError::Embed(e.to_string()))
    }
}

impl TextEmbedder for FastEmbedder {
    fn dimensions(&self) -> usize {
        JINA_CODE_DIM
    }

    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.embed(texts.to_vec())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let mut out = self.embed(vec![text.to_string()])?;
        out.pop()
            .ok_or_else(|| EmbedError::Embed("embedder returned no vector for query".into()))
    }
}
