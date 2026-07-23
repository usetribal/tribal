use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use safetensors::SafeTensors;
use tokenizers::Tokenizer;

use crate::{EmbedError, Result, TextEmbedder};

/// Hugging Face repo the model files are fetched from on first use.
const MODEL_REPO: &str = "minishlab/potion-retrieval-32M";

/// Files that make up a model2vec model: the token->vector matrix and the
/// tokenizer. No config is needed at inference — this model normalizes and has
/// no token weighting/mapping (verified from its `config.json`), so the two
/// files below are the whole model.
const MODEL_FILE: &str = "model.safetensors";
const TOKENIZER_FILE: &str = "tokenizer.json";

/// The single tensor a model2vec safetensors file holds: the [vocab, dim]
/// embedding matrix.
const EMBEDDINGS_TENSOR: &str = "embeddings";

/// Reject an absurd `content-length` before allocating the download buffer, so
/// a misrouted response can't drive an unbounded allocation. The real model is
/// ~130 MB; this leaves generous headroom without being unbounded.
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// A model2vec static embedder: tokenize, look up each token id's row in the
/// embedding matrix, mean-pool, L2-normalize. There is no transformer and no
/// ONNX runtime, so construction is a matrix + tokenizer load (no model
/// session) and a query embeds in microseconds — which is why this replaced the
/// ONNX embedder that could never fit the hook's latency budget.
///
/// The model is **symmetric** (model2vec has no query/document instruction
/// prefixes), so both trait methods embed text verbatim; the split stays for a
/// future asymmetric embedder (gotcha R2.1).
pub struct Model2VecEmbedder {
    tokenizer: Tokenizer,
    embeddings: Vec<f32>,
    dim: usize,
    vocab: usize,
}

impl Model2VecEmbedder {
    /// Load the model, downloading its files into `cache_dir` on first use.
    /// Fetches can touch the network, so callers on the hook path must treat
    /// construction as fallible and fail open, never block the agent on a
    /// download (context-injection: fail open).
    pub fn new(cache_dir: PathBuf) -> Result<Self> {
        let model_path = ensure_cached(&cache_dir, MODEL_FILE)?;
        let tokenizer_path = ensure_cached(&cache_dir, TOKENIZER_FILE)?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| EmbedError::Unavailable(format!("tokenizer load failed: {e}")))?;

        let bytes = fs::read(&model_path)
            .map_err(|e| EmbedError::Unavailable(format!("read {MODEL_FILE}: {e}")))?;
        let (embeddings, vocab, dim) = load_embedding_matrix(&bytes)?;

        Ok(Self {
            tokenizer,
            embeddings,
            dim,
            vocab,
        })
    }

    /// Embed one text: mean-pool the rows of its content tokens, then
    /// L2-normalize. A text with no in-vocabulary tokens yields a zero vector,
    /// which scores 0 against everything — the honest answer for "no signal".
    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        // add_special_tokens = false: model2vec embeds content tokens only, so
        // [CLS]/[SEP] must not enter the mean-pool.
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| EmbedError::Embed(format!("tokenize failed: {e}")))?;
        let ids = encoding.get_ids();

        let mut sum = vec![0.0f32; self.dim];
        let mut counted = 0usize;
        for &id in ids {
            let row = id as usize;
            if row >= self.vocab {
                continue;
            }
            let start = row * self.dim;
            for (acc, v) in sum
                .iter_mut()
                .zip(&self.embeddings[start..start + self.dim])
            {
                *acc += *v;
            }
            counted += 1;
        }
        if counted > 0 {
            let inv = 1.0 / counted as f32;
            for v in &mut sum {
                *v *= inv;
            }
        }
        l2_normalize(&mut sum);
        Ok(sum)
    }
}

impl TextEmbedder for Model2VecEmbedder {
    fn dimensions(&self) -> usize {
        self.dim
    }

    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_one(text)
    }
}

/// Parse the [vocab, dim] f32 embedding matrix out of a model2vec safetensors
/// blob, returned as a flat row-major buffer plus its shape.
fn load_embedding_matrix(bytes: &[u8]) -> Result<(Vec<f32>, usize, usize)> {
    let tensors = SafeTensors::deserialize(bytes)
        .map_err(|e| EmbedError::Unavailable(format!("parse safetensors: {e}")))?;
    let view = tensors
        .tensor(EMBEDDINGS_TENSOR)
        .map_err(|e| EmbedError::Unavailable(format!("no `{EMBEDDINGS_TENSOR}` tensor: {e}")))?;

    if view.dtype() != safetensors::Dtype::F32 {
        return Err(EmbedError::Unavailable(format!(
            "expected F32 embeddings, got {:?}",
            view.dtype()
        )));
    }
    let shape = view.shape();
    if shape.len() != 2 {
        return Err(EmbedError::Unavailable(format!(
            "expected a 2-D embedding matrix, got shape {shape:?}"
        )));
    }
    let (vocab, dim) = (shape[0], shape[1]);

    let raw = view.data();
    let embeddings: Vec<f32> = raw
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if embeddings.len() != vocab * dim {
        return Err(EmbedError::Unavailable(format!(
            "embedding matrix size {} != {vocab}*{dim}",
            embeddings.len()
        )));
    }
    Ok((embeddings, vocab, dim))
}

fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        let inv = 1.0 / norm;
        for x in v.iter_mut() {
            *x *= inv;
        }
    }
}

/// Return the cached path for `file`, downloading it from Hugging Face on first
/// use. Offline after the first successful download. A partial write can never
/// be mistaken for a complete file: the download lands in a temp path and is
/// renamed into place only after the full body is read (rename is atomic on the
/// same filesystem).
fn ensure_cached(cache_dir: &Path, file: &str) -> Result<PathBuf> {
    let dest = cache_dir.join(file);
    if dest.exists() {
        return Ok(dest);
    }
    fs::create_dir_all(cache_dir)
        .map_err(|e| EmbedError::Unavailable(format!("create cache dir: {e}")))?;

    let url = format!("https://huggingface.co/{MODEL_REPO}/resolve/main/{file}");
    let body = download(&url)?;

    let tmp = cache_dir.join(format!("{file}.partial"));
    fs::write(&tmp, &body).map_err(|e| EmbedError::Unavailable(format!("write {file}: {e}")))?;
    fs::rename(&tmp, &dest)
        .map_err(|e| EmbedError::Unavailable(format!("finalize {file}: {e}")))?;
    Ok(dest)
}

fn download(url: &str) -> Result<Vec<u8>> {
    let response = ureq::get(url)
        .call()
        .map_err(|e| EmbedError::Unavailable(format!("download {url}: {e}")))?;

    let capacity = response
        .header("content-length")
        .and_then(|v| v.parse::<u64>().ok());
    if let Some(len) = capacity {
        if len > MAX_DOWNLOAD_BYTES {
            return Err(EmbedError::Unavailable(format!(
                "refusing {len}-byte download from {url} (over cap)"
            )));
        }
    }

    let mut body = Vec::with_capacity(capacity.unwrap_or(0) as usize);
    response
        .into_reader()
        .take(MAX_DOWNLOAD_BYTES)
        .read_to_end(&mut body)
        .map_err(|e| EmbedError::Unavailable(format!("read body {url}: {e}")))?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_from_a_synthetic_matrix() {
        // A hand-built 3-token, 2-dim model exercises tokenize -> lookup ->
        // mean-pool -> normalize without the network or the real 130 MB model,
        // so CI needs no download and the pure math is driven directly.
        let embeddings = vec![
            1.0, 0.0, // row 0
            0.0, 2.0, // row 1
            3.0, 4.0, // row 2
        ];
        let embedder = Model2VecEmbedder {
            tokenizer: minimal_tokenizer(),
            embeddings,
            dim: 2,
            vocab: 3,
        };

        // "a a" -> ids [0, 0]; mean is row 0 = (1, 0); normalized stays (1, 0).
        let v = embedder.embed_query("a a").unwrap();
        assert_eq!(v.len(), 2);
        assert!((v[0] - 1.0).abs() < 1e-6);
        assert!(v[1].abs() < 1e-6);
    }

    #[test]
    fn unknown_tokens_yield_a_zero_vector() {
        let embedder = Model2VecEmbedder {
            tokenizer: minimal_tokenizer(),
            embeddings: vec![1.0, 0.0, 0.0, 1.0],
            dim: 2,
            vocab: 2,
        };
        // The empty string tokenizes to no ids -> zero vector, not a panic.
        let v = embedder.embed_query("").unwrap();
        assert_eq!(v, vec![0.0, 0.0]);
    }

    /// A tiny whitespace WordLevel tokenizer mapping "a"->0, "b"->1, "c"->2,
    /// built from a minimal `tokenizer.json` so the tests exercise the
    /// embedder's math with deterministic ids and no downloaded vocabulary.
    fn minimal_tokenizer() -> Tokenizer {
        let json = r#"{
            "version": "1.0",
            "truncation": null,
            "padding": null,
            "added_tokens": [],
            "normalizer": null,
            "pre_tokenizer": {"type": "Whitespace"},
            "post_processor": null,
            "decoder": null,
            "model": {
                "type": "WordLevel",
                "vocab": {"a": 0, "b": 1, "c": 2},
                "unk_token": "a"
            }
        }"#;
        Tokenizer::from_bytes(json.as_bytes()).unwrap()
    }
}
