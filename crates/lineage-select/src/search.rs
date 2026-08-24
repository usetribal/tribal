//! How the selector reaches a search implementation.

use std::fmt;

/// One session that matched, in relevance order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMatch {
    pub id: String,
    /// The passage that matched, shown on the row's context line. Absent when
    /// the match was semantic rather than literal — a session can be relevant
    /// without containing the words, and inventing a passage would misreport
    /// why it is in the list.
    pub passage: Option<String>,
}

impl From<&str> for SessionMatch {
    /// A match with no passage — what a search that only knows ids produces.
    fn from(id: &str) -> Self {
        Self {
            id: id.to_string(),
            passage: None,
        }
    }
}

/// Searches session content, returning the sessions that matched in relevance
/// order.
///
/// The selector holds this rather than a retriever because the real
/// implementation needs a git repository and a search index, and dragging those
/// into a rendering crate would make it untestable without both. Relevance
/// order is carried by the order of the returned matches, so the selector never
/// has to understand how a match was scored.
pub trait SessionSearch {
    fn search(&self, query: &str) -> Result<Vec<SessionMatch>, SearchError>;
}

/// A search that could not answer.
///
/// Distinct from an empty result on purpose: a missing or broken index must not
/// render as "nothing matched", which would send a user looking for a session
/// that is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchError {
    pub message: String,
}

impl SearchError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SearchError {}
