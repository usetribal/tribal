//! Content search for the session selector.
//!
//! Wraps the retrieval stack `context query` uses. Nothing is ranked here: the
//! retriever decides relevance and this only folds turn-level evidence into the
//! sessions the evidence belongs to, so the selector can order rows by it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

#[derive(Default)]
enum DenseLeg {
    #[default]
    Idle,
    Loading,
    Ready(Box<Option<Model2VecEmbedder>>),
}

impl DenseLeg {
    fn ready_embedder(&self) -> Option<&Model2VecEmbedder> {
        match self {
            DenseLeg::Ready(embedder) => embedder.as_ref().as_ref(),
            _ => None,
        }
    }
}

/// Searches the sessions of one repository by what was said in them.
///
/// Opens the repository and index once and holds them, because a selector
/// searches on every pause in typing and reopening per keystroke would dominate
/// the budget.
pub struct RepoSessionSearch {
    repo_path: PathBuf,
    index_path: PathBuf,
    cache_dir: PathBuf,
    dense: Mutex<DenseLeg>,
}

impl RepoSessionSearch {
    pub fn open(repo_path: &Path) -> Result<Arc<Self>, SearchError> {
        let repo = open_repo(repo_path).map_err(|e| SearchError::new(e.to_string()))?;
        let index_path = repo.git_dir().join("lineage").join("index.db");
        let cache_dir = embed_cache_dir();
        let search = Arc::new(Self {
            repo_path: repo_path.to_path_buf(),
            index_path,
            cache_dir: cache_dir.clone(),
            dense: Mutex::new(DenseLeg::Idle),
        });
        if Model2VecEmbedder::is_cached(&cache_dir) {
            search.begin_dense_load();
        }
        Ok(search)
    }

    fn begin_dense_load(self: &Arc<Self>) {
        let mut guard = self.dense.lock().expect("dense leg lock");
        if !matches!(*guard, DenseLeg::Idle) {
            return;
        }
        *guard = DenseLeg::Loading;
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || {
            let Some(search) = weak.upgrade() else {
                return;
            };
            let loaded = Model2VecEmbedder::new(search.cache_dir.clone()).ok();
            let mut guard = search.dense.lock().expect("dense leg lock");
            *guard = DenseLeg::Ready(Box::new(loaded));
        });
    }

    /// Whether fused search can run right now (dense model loaded).
    pub fn is_fused(&self) -> bool {
        self.dense
            .lock()
            .expect("dense leg lock")
            .ready_embedder()
            .is_some()
    }
}

impl SessionSearch for RepoSessionSearch {
    fn search(&self, query: &str) -> Result<Vec<SessionMatch>, SearchError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let repo = open_repo(&self.repo_path).map_err(|e| SearchError::new(e.to_string()))?;
        let index =
            LineageIndex::open(&self.index_path).map_err(|e| SearchError::new(e.to_string()))?;
        let intent = IntentQuery {
            text: query.to_string(),
            budget_ms: Some(BUDGET_MS),
        };

        let fts = FtsRetriever::new(repo.inner(), &index);
        let dense_guard = self.dense.lock().expect("dense leg lock");
        let retrieval = if let Some(embedder) = dense_guard.ready_embedder() {
            let dense = DenseRetriever::new(repo.inner(), &index, embedder);
            FusedRetriever::new(fts, dense)
                .retrieve_intent(&intent)
                .map_err(|e| SearchError::new(e.to_string()))?
        } else {
            fts.retrieve_intent(&intent)
                .map_err(|e| SearchError::new(e.to_string()))?
        };
        Ok(sessions_in_order(&retrieval))
    }

    fn leg_label(&self) -> &str {
        match &*self.dense.lock().expect("dense leg lock") {
            DenseLeg::Loading => "lex…",
            DenseLeg::Ready(embedder) if embedder.is_some() => "fused",
            _ => "lex",
        }
    }
}

/// Sessions in the order their first piece of evidence appeared, each carrying
/// the text of its best match.
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
