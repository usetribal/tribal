use std::time::Instant;

use git2::Repository;
use lineage_core::{generate_architecture_summary, LineageId};
use lineage_git::read_conversation;
use lineage_search::LineageIndex;

use crate::retriever::{IntentRetriever, Result, RetrievalError};
use crate::session::{attribution_for, is_private_or_private_ancestor};
use crate::types::{strength_for, Evidence, EvidenceTier, IntentQuery, Retrieval};

/// Cache-key component for the intent path: bump on any change to what the FTS
/// retriever would answer for an unchanged corpus (tokenization, candidate
/// depth, evidence shape).
pub const FTS_RETRIEVER_VERSION: &str = "1";

/// How many FTS candidates to pull before building evidence. Over-retrieve
/// relative to what selection injects: fusion (later) needs depth so a strong
/// single-leg hit is not truncated away (gotcha F.2), and the intent path wants
/// headroom for privacy filtering to drop some without starving the result. A
/// tunable with an SE-domain default, not a hard limit on what selection shows.
const DEFAULT_CANDIDATE_DEPTH: usize = 50;

/// Rung 1 — lexical intent retrieval over the session FTS index (BM25). The
/// whole session is the FTS document (BM25 handles long docs), so a match means
/// "this session's text is about the query". Evidence is session-level and
/// ordered by the index's relevance ranking.
pub struct FtsRetriever<'a> {
    repo: &'a Repository,
    index: &'a LineageIndex,
    candidate_depth: usize,
}

impl<'a> FtsRetriever<'a> {
    pub fn new(repo: &'a Repository, index: &'a LineageIndex) -> Self {
        Self {
            repo,
            index,
            candidate_depth: DEFAULT_CANDIDATE_DEPTH,
        }
    }

    /// Override the candidate depth (tunable; see `DEFAULT_CANDIDATE_DEPTH`).
    pub fn with_candidate_depth(mut self, depth: usize) -> Self {
        self.candidate_depth = depth;
        self
    }

    /// Build intent-match evidence for one session, or `None` if it is private
    /// or unreadable. The FTS relevance order is preserved by the caller; this
    /// only shapes a single session's evidence.
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

impl IntentRetriever for FtsRetriever<'_> {
    fn retrieve_intent(&self, query: &IntentQuery) -> Result<Retrieval> {
        let started = Instant::now();

        let hits = self
            .index
            .search(&query.text, self.candidate_depth)
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;

        // The index returns hits already ordered by BM25 relevance. Preserve
        // that order in the evidence so a later fusion step sees a clean
        // session ranking (fusion is rank-based).
        let mut evidence = Vec::new();
        let mut truncated = false;
        for hit in hits {
            if let Some(budget_ms) = query.budget_ms {
                // Spec: return what we have rather than overrun the caller's
                // budget — partial evidence beats a blown deadline.
                if started.elapsed().as_millis() >= u128::from(budget_ms) {
                    truncated = true;
                    break;
                }
            }
            if let Some(entry) = self.evidence_for_session(&hit.session_id)? {
                evidence.push(entry);
            }
        }

        // Every entry is the same tier/strength, so `from_evidence`'s
        // strength-sort is order-preserving and the FTS relevance ranking
        // survives — the strongest lexical match stays first.
        let mut retrieval = Retrieval::from_evidence(evidence);
        retrieval.truncated = truncated;
        Ok(retrieval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{
        AgentKind, Artifact, ArtifactKind, ArtifactResolve, Conversation, ResolveStrategy, Role,
        Turn,
    };
    use lineage_git::{open_repo, persist_conversation};

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    fn session_about(
        dir: &std::path::Path,
        user_prompt: &str,
        tool: &str,
        path: &str,
    ) -> Conversation {
        let mut conv = Conversation::new(AgentKind::Claude, dir.display().to_string());
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: user_prompt.into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![lineage_core::ToolCall {
                id: "t".into(),
                name: tool.into(),
                arguments: format!(r#"{{"file_path": "{path}"}}"#),
                result: None,
            }],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
                kind: ArtifactKind::FileEdit,
                path: path.into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: Some(ArtifactResolve {
                    strategy: ResolveStrategy::OldString,
                    old_string: None,
                    new_string: Some("fn rebuild_index() {}".into()),
                    patch: None,
                }),
            }],
        });
        conv
    }

    #[test]
    fn matches_session_on_prose_intent() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let conv = session_about(
            dir.path(),
            "add a redis caching layer",
            "Edit",
            "src/cache.rs",
        );
        persist_conversation(repo.inner(), &conv).unwrap();

        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        index.index_conversation(&conv).unwrap();

        let retriever = FtsRetriever::new(repo.inner(), &index);
        let retrieval = retriever
            .retrieve_intent(&IntentQuery {
                text: "caching".into(),
                budget_ms: None,
            })
            .unwrap();

        assert_eq!(retrieval.evidence.len(), 1);
        assert_eq!(retrieval.evidence[0].session_id, conv.id);
        assert_eq!(retrieval.evidence[0].tier, EvidenceTier::IntentMatch);
    }

    #[test]
    fn matches_identifier_that_appears_only_in_tool_call() {
        // The dogfood failure: "rebuild-index" appears only as a tool path / edit
        // snippet, never in prose. Enriched indexing + identifier-preserving
        // tokenization must still surface it.
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let conv = session_about(
            dir.path(),
            "wire up the command",
            "Bash",
            "src/rebuild-index.rs",
        );
        persist_conversation(repo.inner(), &conv).unwrap();

        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        index.index_conversation(&conv).unwrap();

        let retriever = FtsRetriever::new(repo.inner(), &index);
        let retrieval = retriever
            .retrieve_intent(&IntentQuery {
                text: "rebuild-index".into(),
                budget_ms: None,
            })
            .unwrap();

        assert_eq!(
            retrieval.evidence.len(),
            1,
            "identifier-only match should surface"
        );
        assert_eq!(retrieval.evidence[0].session_id, conv.id);
    }

    #[test]
    fn private_sessions_are_never_evidence() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let mut conv = session_about(dir.path(), "add caching", "Edit", "src/cache.rs");
        conv.private = true;
        persist_conversation(repo.inner(), &conv).unwrap();

        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        index.index_conversation(&conv).unwrap();

        let retriever = FtsRetriever::new(repo.inner(), &index);
        let retrieval = retriever
            .retrieve_intent(&IntentQuery {
                text: "caching".into(),
                budget_ms: None,
            })
            .unwrap();

        assert!(retrieval.evidence.is_empty());
    }

    #[test]
    fn no_match_is_honest_nothing() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let conv = session_about(dir.path(), "add caching", "Edit", "src/cache.rs");
        persist_conversation(repo.inner(), &conv).unwrap();

        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        index.index_conversation(&conv).unwrap();

        let retriever = FtsRetriever::new(repo.inner(), &index);
        let retrieval = retriever
            .retrieve_intent(&IntentQuery {
                text: "kubernetes deployment yaml".into(),
                budget_ms: None,
            })
            .unwrap();

        assert!(retrieval.evidence.is_empty());
    }
}
