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

    /// Seal turn-text-bearing output as [`Gated`]. Takes `&mut self` so a caller
    /// can only reach it while holding a live gate — the same gate it must have
    /// asked about every session whose text it is sealing.
    pub(crate) fn seal<T>(&mut self, value: T) -> Gated<T> {
        Gated(value)
    }

    /// Gate a batch of session-keyed items in one pass: drop what the gate
    /// refuses, hand each survivor its attribution, and seal the result. Every
    /// traversal verb has this shape, and routing them all through one method
    /// means a new verb cannot gate *some* of its rows and still compile.
    pub(crate) fn admit_all<In, Out>(
        &mut self,
        items: Vec<In>,
        session_id_of: impl Fn(&In) -> &str,
        build: impl Fn(In, &str) -> Out,
    ) -> Result<Gated<Vec<Out>>> {
        let mut out = Vec::new();
        for item in items {
            let Some(attribution) = self.attribution(session_id_of(&item))? else {
                continue;
            };
            out.push(build(item, &attribution));
        }
        Ok(self.seal(out))
    }
}

/// A payload carrying turn *text*, proven to have passed [`SessionGate`].
///
/// The gate used to be structurally safe because `materialize_turns` was its
/// single exit; agent-facing traversal adds more exits, so the invariant is
/// restated as a type instead of a convention. The inner field is private to
/// this module and [`SessionGate::seal`] is the only constructor, so a
/// primitive cannot return turn text without having run the gate — a future
/// author who forgets gets a compile error, not a leak.
///
/// The guarantee is a compile error, so it is proven by one:
///
/// ```compile_fail
/// let leaked = lineage_retrieval::Gated("private turn text");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gated<T>(T);

impl<T> Gated<T> {
    /// Unwrap at the emit boundary. Reading is unrestricted by design: the
    /// guarantee is about what may be *constructed*, and a consumer that holds a
    /// `Gated` already holds gated data.
    pub fn into_inner(self) -> T {
        self.0
    }

    pub fn get(&self) -> &T {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use lineage_core::{AgentKind, Conversation, LineageId, Role, Turn};
    use lineage_git::{open_repo, persist_conversation};

    use super::*;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    fn session(dir: &std::path::Path, private: bool) -> Conversation {
        let mut conv = Conversation::new(AgentKind::Claude, dir.display().to_string());
        conv.private = private;
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "the private words".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        conv
    }

    #[test]
    fn gate_refuses_private_sessions_and_admits_the_rest() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let public = session(dir.path(), false);
        let private = session(dir.path(), true);
        persist_conversation(repo.inner(), &public).unwrap();
        persist_conversation(repo.inner(), &private).unwrap();

        let mut gate = SessionGate::new(repo.inner());
        assert!(gate.attribution(private.id.as_str()).unwrap().is_none());
        let admitted = gate.attribution(public.id.as_str()).unwrap();
        assert!(admitted.unwrap().contains("claude session"));
    }

    /// A fork of a private session is private too, so text reached through a
    /// child id must not escape either.
    #[test]
    fn gate_refuses_a_fork_of_a_private_session() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let parent = session(dir.path(), true);
        let mut child = session(dir.path(), false);
        child.parent_session_id = Some(parent.id.clone());
        persist_conversation(repo.inner(), &parent).unwrap();
        persist_conversation(repo.inner(), &child).unwrap();

        let mut gate = SessionGate::new(repo.inner());
        assert!(gate.attribution(child.id.as_str()).unwrap().is_none());
    }

    /// An unknown session id has no readable privacy verdict, so it is refused
    /// rather than assumed public.
    #[test]
    fn gate_refuses_an_unknown_session() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let mut gate = SessionGate::new(repo.inner());
        assert!(gate.attribution("nope").unwrap().is_none());
    }

    /// The only way to a `Gated` payload is through a live gate — the
    /// compile-fail doctest on `Gated` proves the negative, this proves the
    /// positive path still works.
    #[test]
    fn seal_round_trips_through_a_live_gate() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let mut gate = SessionGate::new(repo.inner());
        let sealed = gate.seal(vec!["turn text"]);
        assert_eq!(sealed.get().len(), 1);
        assert_eq!(sealed.into_inner(), vec!["turn text"]);
    }
}
