//! Shared session-evidence helpers used by every retriever: privacy filtering
//! and the summary/attribution a session contributes. Kept in one place so the
//! privacy guarantee (never emit a private session, or one whose fork chain
//! reaches a private one) is enforced identically by the file-keyed and the
//! intent retrievers.

use std::collections::HashSet;

use git2::Repository;
use lineage_core::Conversation;
use lineage_git::read_conversation_stored;

use crate::retriever::{Result, RetrievalError};

/// Privacy is enforced before caching or selection: a private conversation — or
/// one whose parent chain reaches a private one — is never evidence
/// (spec: Privacy). A corrupt fork ref degrades to "not private", never an
/// infinite loop.
pub(crate) fn is_private_or_private_ancestor(
    repo: &Repository,
    conversation: &Conversation,
) -> Result<bool> {
    if conversation.private {
        return Ok(true);
    }

    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(conversation.id.as_str().to_string());
    let mut next = conversation.parent_session_id.clone();
    while let Some(parent_id) = next {
        if !seen.insert(parent_id.as_str().to_string()) {
            return Ok(false);
        }
        let parent = read_conversation_stored(repo, &parent_id)
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
        let Some(parent) = parent else {
            // An unsynced/absent parent is unknown, and unknown does not reach a
            // private conversation.
            return Ok(false);
        };
        if parent.private {
            return Ok(true);
        }
        next = parent.parent_session_id;
    }
    Ok(false)
}

/// Display-only source label for a session: who/when/agent. Never an
/// authorization identity (spec: attribution).
pub(crate) fn attribution_for(conversation: &Conversation) -> String {
    format!(
        "{} session {}, {}",
        conversation.agent.as_str(),
        conversation.id.as_str(),
        conversation.started_at.format("%Y-%m-%d"),
    )
}
