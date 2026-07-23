use crate::types::{ContextQuery, IntentQuery, Retrieval};

#[derive(Debug, thiserror::Error)]
pub enum RetrievalError {
    #[error("retrieval failed: {0}")]
    Retrieval(String),
    #[error("cache failed: {0}")]
    Cache(String),
}

pub type Result<T> = std::result::Result<T, RetrievalError>;

/// Where retrieval runs is a deployment detail (in-process over local data
/// now, a server endpoint in team mode); callers only ever see this trait.
/// Synchronous by design: the hook is a one-shot process and honors
/// `budget_ms` by failing open, not by cancelling.
pub trait Retriever {
    fn retrieve(&self, query: &ContextQuery) -> Result<Retrieval>;
}

/// Intent (prompt-keyed) retrieval. A separate trait from `Retriever` because
/// the query shape differs (free text, no file anchor) and each mechanism —
/// lexical (`FtsRetriever`), dense, later fused — is its own impl so the legs
/// can be measured independently before fusion. Evidence is always at session
/// granularity; a dense retriever that matches sub-session chunks rolls them up
/// to the session before returning.
pub trait IntentRetriever {
    fn retrieve_intent(&self, query: &IntentQuery) -> Result<Retrieval>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NothingRetriever;

    impl Retriever for NothingRetriever {
        fn retrieve(&self, _query: &ContextQuery) -> Result<Retrieval> {
            Ok(Retrieval::empty())
        }
    }

    #[test]
    fn trait_is_object_safe_and_usable_behind_dyn() {
        // The hook selects an implementation at runtime (local vs remote), so
        // object safety is part of the contract, not an accident.
        let retriever: Box<dyn Retriever> = Box::new(NothingRetriever);
        let query = ContextQuery {
            file_path: "src/lib.rs".into(),
            file_blob_sha: "00".repeat(32),
            repo: lineage_core::RepoBinding {
                normalized_remote_url: "github.com/acme/widgets".into(),
                root_commit_sha: "11".repeat(20),
                server_repo_id: None,
            },
            budget_ms: None,
        };
        let retrieval = retriever.retrieve(&query).unwrap();
        assert!(retrieval.evidence.is_empty());
    }
}
