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

/// Cap for verbatim turn text carried in evidence. Selection caps the whole
/// digest at 4 KiB across up to three entries; capping per entry here keeps
/// one long turn from starving the others while leaving render a pure
/// formatting step (spec: the selector derives nothing).
const VERBATIM_SUMMARY_MAX_CHARS: usize = 1200;

/// The evidence payload for a turn-grained match: the turn's own words,
/// bounded. Cuts on a char boundary with an ellipsis so a truncated payload is
/// visibly truncated.
pub(crate) fn verbatim_summary(body: &str) -> String {
    if body.chars().count() <= VERBATIM_SUMMARY_MAX_CHARS {
        return body.to_string();
    }
    let mut out: String = body.chars().take(VERBATIM_SUMMARY_MAX_CHARS).collect();
    out.push('…');
    out
}

/// Per-query memo of the session-level admission decision (privacy) and
/// attribution. Turn-grained retrieval visits many turns of the same session;
/// the conversation ref is read once per session, not once per turn. Reads the
/// stored (unhydrated) conversation — privacy and attribution live in
/// metadata, and turn text comes from the index.
pub(crate) struct SessionGate<'a> {
    repo: &'a Repository,
    verdicts: std::collections::HashMap<String, Option<String>>,
}

impl<'a> SessionGate<'a> {
    pub(crate) fn new(repo: &'a Repository) -> Self {
        Self {
            repo,
            verdicts: std::collections::HashMap::new(),
        }
    }

    /// `Some(attribution)` when the session may be emitted as evidence; `None`
    /// when it is private (or its fork chain is) or unreadable.
    pub(crate) fn attribution(&mut self, session_id: &str) -> Result<Option<String>> {
        if let Some(verdict) = self.verdicts.get(session_id) {
            return Ok(verdict.clone());
        }
        let id = lineage_core::LineageId::from(session_id.to_string());
        let conversation = read_conversation_stored(self.repo, &id)
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
        let verdict = match conversation {
            Some(conversation) if !is_private_or_private_ancestor(self.repo, &conversation)? => {
                Some(attribution_for(&conversation))
            }
            _ => None,
        };
        self.verdicts
            .insert(session_id.to_string(), verdict.clone());
        Ok(verdict)
    }
}
