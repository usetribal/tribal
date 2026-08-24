//! Content search for the session selector.
//!
//! Wraps the retrieval stack `context query` uses. Nothing is ranked here: the
//! retriever decides relevance and this only folds turn-level evidence into the
//! sessions the evidence belongs to, so the selector can order rows by it.

use std::path::{Path, PathBuf};

use lineage_embed::Model2VecEmbedder;
use lineage_git::open_repo;
use lineage_retrieval::{
    DenseRetriever, FtsRetriever, FusedRetriever, IntentQuery, IntentRetriever, Retrieval,
};
use lineage_search::LineageIndex;
use lineage_select::{SearchError, SessionMatch, SessionSearch};

use crate::retrieval_cmd::embed_cache_dir;

/// The budget a keystroke-driven search runs under. Matches the by-hand
/// `context query` budget: a selector that hesitates is worse than one that
/// answers from the lexical leg alone.
const BUDGET_MS: u64 = 200;

/// Searches the sessions of one repository by what was said in them.
///
/// Opens the repository and index once and holds them, because a selector
/// searches on every pause in typing and reopening per keystroke would dominate
/// the budget.
pub struct RepoSessionSearch {
    repo_path: PathBuf,
    index_path: PathBuf,
    /// Absent when no model is cached. Building one downloads ~130 MB, which is
    /// not something to do in front of a user waiting for a picker, so the
    /// dense leg is simply skipped until some other command has fetched it.
    embedder: Option<Model2VecEmbedder>,
}

impl RepoSessionSearch {
    pub fn open(repo_path: &Path) -> Result<Self, SearchError> {
        let repo = open_repo(repo_path).map_err(|e| SearchError::new(e.to_string()))?;
        let index_path = repo.git_dir().join("lineage").join("index.db");
        let cache_dir = embed_cache_dir();
        let embedder = Model2VecEmbedder::is_cached(&cache_dir)
            .then(|| Model2VecEmbedder::new(cache_dir).ok())
            .flatten();
        Ok(Self {
            repo_path: repo_path.to_path_buf(),
            index_path,
            embedder,
        })
    }

    /// Whether the dense leg is available. The lexical leg always is.
    pub fn is_fused(&self) -> bool {
        self.embedder.is_some()
    }
}

impl SessionSearch for RepoSessionSearch {
    fn search(&self, query: &str) -> Result<Vec<SessionMatch>, SearchError> {
        let repo = open_repo(&self.repo_path).map_err(|e| SearchError::new(e.to_string()))?;
        let index =
            LineageIndex::open(&self.index_path).map_err(|e| SearchError::new(e.to_string()))?;
        let intent = IntentQuery {
            text: query.to_string(),
            budget_ms: Some(BUDGET_MS),
        };

        let fts = FtsRetriever::new(repo.inner(), &index);
        let retrieval = match self.embedder.as_ref() {
            Some(embedder) => {
                let dense = DenseRetriever::new(repo.inner(), &index, embedder);
                FusedRetriever::new(fts, dense)
                    .retrieve_intent(&intent)
                    .map_err(|e| SearchError::new(e.to_string()))?
            }
            None => fts
                .retrieve_intent(&intent)
                .map_err(|e| SearchError::new(e.to_string()))?,
        };
        Ok(sessions_in_order(&retrieval))
    }
}

/// Sessions in the order their first piece of evidence appeared, each carrying
/// the text of its best match.
///
/// Evidence is per-turn, so one session can match many times; the retriever's
/// ordering is the ranking, so first appearance is that session's rank and its
/// passage, and later duplicates say nothing new.
fn sessions_in_order(retrieval: &Retrieval) -> Vec<SessionMatch> {
    let mut found: Vec<SessionMatch> = Vec::new();
    for evidence in &retrieval.evidence {
        let id = evidence.session_id.to_string();
        if found.iter().any(|match_| match_.id == id) {
            continue;
        }
        found.push(SessionMatch {
            id,
            passage: passage_of(&evidence.summary),
        });
    }
    found
}

/// A one-line passage from a matched turn.
///
/// Evidence summaries are verbatim turn text, so they arrive with the turn's
/// own newlines and indentation. A row has one line to spend, and leading
/// whitespace in it reads as a rendering fault.
fn passage_of(summary: &str) -> Option<String> {
    let flattened = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    (!flattened.is_empty()).then_some(flattened)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::LineageId;
    use lineage_retrieval::{Evidence, EvidenceTier, Strength};

    fn evidence(session: &str) -> Evidence {
        evidence_saying(session, "")
    }

    fn evidence_saying(session: &str, summary: &str) -> Evidence {
        Evidence {
            session_id: LineageId::from(session),
            turn_id: None,
            tier: EvidenceTier::IntentMatch,
            strength: Strength::Medium,
            match_confidence: None,
            line_ranges: vec![],
            summary: summary.to_string(),
            attribution: String::new(),
        }
    }

    #[test]
    fn a_session_carries_the_text_of_its_best_match() {
        let retrieval = Retrieval {
            evidence: vec![
                evidence_saying("a", "  the login\n  endpoint accepts  an empty password "),
                evidence_saying("a", "a later, worse match"),
            ],
            strength: Strength::Medium,
            truncated: false,
        };
        let found = sessions_in_order(&retrieval);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].passage.as_deref(),
            Some("the login endpoint accepts an empty password")
        );
    }

    #[test]
    fn evidence_with_no_text_carries_no_passage() {
        let retrieval = Retrieval {
            evidence: vec![evidence_saying("a", "   ")],
            strength: Strength::Medium,
            truncated: false,
        };
        assert_eq!(sessions_in_order(&retrieval)[0].passage, None);
    }

    #[test]
    fn a_session_keeps_the_rank_of_its_first_match() {
        let retrieval = Retrieval {
            evidence: vec![evidence("b"), evidence("a"), evidence("b"), evidence("c")],
            strength: Strength::Medium,
            truncated: false,
        };
        let found = sessions_in_order(&retrieval);
        let ids: Vec<&str> = found.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a", "c"]);
    }

    #[test]
    fn no_evidence_means_no_sessions() {
        let retrieval = Retrieval {
            evidence: vec![],
            strength: Strength::None,
            truncated: false,
        };
        assert!(sessions_in_order(&retrieval).is_empty());
    }
}
