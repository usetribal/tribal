use std::collections::HashMap;
use std::time::Instant;

use git2::Repository;
use lineage_core::{
    generate_architecture_summary, session_chunks, Conversation, LineageId, DEFAULT_CHUNK_MAX_CHARS,
};
use lineage_embed::TextEmbedder;
use lineage_git::read_conversation;
use lineage_search::LineageIndex;

use crate::retriever::{IntentRetriever, Result, RetrievalError};
use crate::session::{attribution_for, is_private_or_private_ancestor};
use crate::types::{strength_for, Evidence, EvidenceTier, IntentQuery, Retrieval};

/// Cache-key / vector-tag component: bump on any change to what this retriever
/// would answer for an unchanged corpus (model, chunking, roll-up). The model
/// identity is part of it — a corpus embedded by one model must not be scored
/// against another's query vector, and it is what lets incremental embedding
/// know which stored vectors are current.
pub const DENSE_RETRIEVER_VERSION: &str = "1-jina-v2-code";

/// Embed a session's chunks and store the vectors — the dense index pass, run
/// at import/rebuild when the `dense` feature is on. Kept beside the retriever
/// so the chunking used to *store* vectors matches the retriever's model, and
/// so the CLI and the eval harness share one code path. Idempotent per session
/// (store replaces), and tags vectors with `DENSE_RETRIEVER_VERSION` so a later
/// pass can skip already-current sessions.
pub fn embed_and_store_session<E: TextEmbedder>(
    index: &LineageIndex,
    embedder: &E,
    conversation: &Conversation,
) -> Result<usize> {
    let chunks = session_chunks(conversation, DEFAULT_CHUNK_MAX_CHARS);
    if chunks.is_empty() {
        return Ok(0);
    }
    let vectors = embedder
        .embed_documents(&chunks)
        .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
    index
        .store_session_vectors(conversation.id.as_str(), &vectors, DENSE_RETRIEVER_VERSION)
        .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
    Ok(vectors.len())
}

/// How many top sessions to build evidence for. Over-retrieve relative to what
/// selection injects, for the same reason as the FTS leg (fusion depth, and
/// headroom for privacy filtering) — gotcha F.2.
const DEFAULT_TOP_SESSIONS: usize = 50;

/// Rung 2 — dense (semantic) intent retrieval. Chunks are embedded at index
/// time and stored in `lineage-search`; at query time the query is embedded and
/// scored against every chunk by cosine (a dot product, since vectors are
/// L2-normalized). A session's score is the **max** over its chunks (best
/// chunk), never a sum — summing would give long, many-chunk sessions a
/// structural advantage that later fusion would inherit.
pub struct DenseRetriever<'a, E: TextEmbedder> {
    repo: &'a Repository,
    index: &'a LineageIndex,
    embedder: &'a E,
    top_sessions: usize,
}

impl<'a, E: TextEmbedder> DenseRetriever<'a, E> {
    pub fn new(repo: &'a Repository, index: &'a LineageIndex, embedder: &'a E) -> Self {
        Self {
            repo,
            index,
            embedder,
            top_sessions: DEFAULT_TOP_SESSIONS,
        }
    }

    pub fn with_top_sessions(mut self, top: usize) -> Self {
        self.top_sessions = top;
        self
    }

    fn evidence_for_session(&self, session_id: &str) -> Result<Option<Evidence>> {
        let id = LineageId::from(session_id.to_string());
        let conversation = read_conversation(self.repo, &id)
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
        let Some(conversation) = conversation else {
            return Ok(None);
        };
        if is_private_or_private_ancestor(self.repo, &conversation)? {
            return Ok(None);
        }

        Ok(Some(Evidence {
            session_id: conversation.id.clone(),
            tier: EvidenceTier::IntentMatch,
            strength: strength_for(EvidenceTier::IntentMatch, None),
            match_confidence: None,
            line_ranges: Vec::new(),
            summary: generate_architecture_summary(&conversation),
            attribution: attribution_for(&conversation),
        }))
    }
}

/// Cosine similarity of two L2-normalized vectors is their dot product. A
/// dimension mismatch (corrupt or cross-model vector) scores 0 rather than
/// panicking — the retrieval path must never crash the hook.
fn cosine_normalized(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

impl<E: TextEmbedder> IntentRetriever for DenseRetriever<'_, E> {
    fn retrieve_intent(&self, query: &IntentQuery) -> Result<Retrieval> {
        let started = Instant::now();

        let query_vec = self
            .embedder
            .embed_query(&query.text)
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;

        let chunks = self
            .index
            .all_chunk_vectors()
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;

        // Roll chunk scores up to the session by max (best chunk). A session's
        // relevance is its single most relevant chunk, so a focused session is
        // not out-competed by a sprawling one that merely has more chances to
        // match (which summing would reward).
        let mut best_by_session: HashMap<String, f32> = HashMap::new();
        for chunk in &chunks {
            let score = cosine_normalized(&query_vec, &chunk.vector);
            best_by_session
                .entry(chunk.session_id.clone())
                .and_modify(|s| {
                    if score > *s {
                        *s = score;
                    }
                })
                .or_insert(score);
        }

        // Rank sessions by their best-chunk score, strongest first, and keep
        // the top slice as evidence candidates.
        let mut ranked: Vec<(String, f32)> = best_by_session.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranked.truncate(self.top_sessions);

        let mut evidence = Vec::new();
        let mut truncated = false;
        for (session_id, _score) in ranked {
            if let Some(budget_ms) = query.budget_ms {
                if started.elapsed().as_millis() >= u128::from(budget_ms) {
                    truncated = true;
                    break;
                }
            }
            if let Some(entry) = self.evidence_for_session(&session_id)? {
                evidence.push(entry);
            }
        }

        // Evidence is already in descending score order; every entry shares the
        // same tier/strength, so `from_evidence`'s strength-sort is stable and
        // preserves the dense ranking.
        let mut retrieval = Retrieval::from_evidence(evidence);
        retrieval.truncated = truncated;
        Ok(retrieval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_embed::{EmbedError, TextEmbedder};

    /// A deterministic stand-in embedder: no ONNX, no download. It maps text to
    /// a tiny vector by keyword presence, so tests exercise the retriever's
    /// scoring/roll-up/privacy logic without the real model. Vectors are
    /// L2-normalized to match the real embedder's contract.
    struct FakeEmbedder;

    fn normalize(mut v: Vec<f32>) -> Vec<f32> {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }

    impl FakeEmbedder {
        fn vector_for(text: &str) -> Vec<f32> {
            let t = text.to_lowercase();
            normalize(vec![
                t.contains("cache") as u8 as f32 + t.contains("caching") as u8 as f32,
                t.contains("auth") as u8 as f32,
                t.contains("metric") as u8 as f32,
            ])
        }
    }

    impl TextEmbedder for FakeEmbedder {
        fn dimensions(&self) -> usize {
            3
        }
        fn embed_documents(
            &self,
            texts: &[String],
        ) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
            Ok(texts.iter().map(|t| Self::vector_for(t)).collect())
        }
        fn embed_query(&self, text: &str) -> std::result::Result<Vec<f32>, EmbedError> {
            Ok(Self::vector_for(text))
        }
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    fn session_with_prompt(dir: &std::path::Path, prompt: &str) -> lineage_core::Conversation {
        let mut conv = lineage_core::Conversation::new(
            lineage_core::AgentKind::Claude,
            dir.display().to_string(),
        );
        conv.turns.push(lineage_core::Turn {
            id: LineageId::new(),
            role: lineage_core::Role::User,
            content: prompt.into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        conv
    }

    /// Store a session's chunk vectors via the real index pass, so the test
    /// exercises the same chunking + storage path the CLI will.
    fn embed_and_store(
        index: &LineageIndex,
        conv: &lineage_core::Conversation,
        embedder: &FakeEmbedder,
    ) {
        embed_and_store_session(index, embedder, conv).unwrap();
    }

    #[test]
    fn matches_semantically_closest_session() {
        let dir = init_repo();
        let repo = lineage_git::open_repo(dir.path()).unwrap();
        let embedder = FakeEmbedder;

        let cache_conv = session_with_prompt(dir.path(), "add a caching layer");
        let auth_conv = session_with_prompt(dir.path(), "fix the auth guard");
        for conv in [&cache_conv, &auth_conv] {
            lineage_git::persist_conversation(repo.inner(), conv).unwrap();
        }

        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        embed_and_store(&index, &cache_conv, &embedder);
        embed_and_store(&index, &auth_conv, &embedder);

        let retriever = DenseRetriever::new(repo.inner(), &index, &embedder);
        let retrieval = retriever
            .retrieve_intent(&IntentQuery {
                text: "how do we cache things".into(),
                budget_ms: None,
            })
            .unwrap();

        // The caching session must rank first for a caching query.
        assert!(!retrieval.evidence.is_empty());
        assert_eq!(retrieval.evidence[0].session_id, cache_conv.id);
    }

    #[test]
    fn private_sessions_are_never_evidence() {
        let dir = init_repo();
        let repo = lineage_git::open_repo(dir.path()).unwrap();
        let embedder = FakeEmbedder;

        let mut conv = session_with_prompt(dir.path(), "add a caching layer");
        conv.private = true;
        lineage_git::persist_conversation(repo.inner(), &conv).unwrap();

        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        embed_and_store(&index, &conv, &embedder);

        let retriever = DenseRetriever::new(repo.inner(), &index, &embedder);
        let retrieval = retriever
            .retrieve_intent(&IntentQuery {
                text: "caching".into(),
                budget_ms: None,
            })
            .unwrap();

        assert!(retrieval.evidence.is_empty());
    }

    #[test]
    fn empty_corpus_is_honest_nothing() {
        let dir = init_repo();
        let repo = lineage_git::open_repo(dir.path()).unwrap();
        let embedder = FakeEmbedder;
        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();

        let retriever = DenseRetriever::new(repo.inner(), &index, &embedder);
        let retrieval = retriever
            .retrieve_intent(&IntentQuery {
                text: "caching".into(),
                budget_ms: None,
            })
            .unwrap();

        assert!(retrieval.evidence.is_empty());
    }
}
