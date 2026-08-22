mod citations;
mod content;
mod metadata;
mod path_util;
mod shell_writes;

pub use citations::{enrich_turn_with_citations, extract_citations_from_text};

/// Exported so a caller can assert against the same derivation the writer and
/// reader share, instead of restating the substitution rule and agreeing with a
/// bug. The rest of `path_util` stays private.
pub use path_util::claude_project_dir;

#[cfg(feature = "claude")]
pub mod claude;
#[cfg(feature = "claude")]
pub mod claude_transcript;
#[cfg(feature = "codex")]
pub mod codex;
#[cfg(feature = "cursor")]
pub mod cursor;

#[cfg(feature = "claude")]
pub use claude::ClaudeAdapter;
#[cfg(feature = "codex")]
pub use codex::CodexAdapter;
#[cfg(feature = "cursor")]
pub use cursor::CursorAdapter;

use lineage_agent::{
    AgentSource, RenderedTranscript, ResumeInvocation, SessionReader, SessionResumer,
    TranscriptWriter,
};
use lineage_core::AgentKind;

pub trait ErasedAdapter: Send + Sync {
    fn agent(&self) -> AgentKind;
    fn discover(&self) -> lineage_core::Result<Vec<lineage_agent::SessionRef>>;
    fn read(
        &self,
        session: &lineage_agent::SessionRef,
    ) -> lineage_core::Result<lineage_core::Conversation>;
    /// Errors for every agent but Claude. Kept on the erased trait rather than
    /// behind a downcast so a caller holding `Box<dyn ErasedAdapter>` gets the
    /// explicit refusal instead of having to know which concrete adapter can.
    fn render_transcript(
        &self,
        conversation: &lineage_core::Conversation,
    ) -> lineage_core::Result<RenderedTranscript>;
    /// Errors for Cursor, and for any session carrying no vendor id. Kept on the
    /// erased trait for the same reason as `render_transcript`: a caller holding
    /// `Box<dyn ErasedAdapter>` gets the explicit refusal instead of having to
    /// know which concrete adapter can reopen a session.
    fn resume_invocation(
        &self,
        conversation: &lineage_core::Conversation,
    ) -> lineage_core::Result<ResumeInvocation>;
}

struct ErasedAdapterImpl<A>(A);

impl<A> ErasedAdapter for ErasedAdapterImpl<A>
where
    A: AgentSource + SessionReader + TranscriptWriter + SessionResumer + Send + Sync,
{
    fn agent(&self) -> AgentKind {
        self.0.agent()
    }

    fn discover(&self) -> lineage_core::Result<Vec<lineage_agent::SessionRef>> {
        self.0.discover()
    }

    fn read(
        &self,
        session: &lineage_agent::SessionRef,
    ) -> lineage_core::Result<lineage_core::Conversation> {
        self.0.read(session)
    }

    fn render_transcript(
        &self,
        conversation: &lineage_core::Conversation,
    ) -> lineage_core::Result<RenderedTranscript> {
        self.0.render_transcript(conversation)
    }

    fn resume_invocation(
        &self,
        conversation: &lineage_core::Conversation,
    ) -> lineage_core::Result<ResumeInvocation> {
        self.0.resume_invocation(conversation)
    }
}

pub fn all_adapters(workspace_root: &std::path::Path) -> Vec<(AgentKind, Box<dyn ErasedAdapter>)> {
    let mut out: Vec<(AgentKind, Box<dyn ErasedAdapter>)> = Vec::new();
    #[cfg(feature = "cursor")]
    {
        let a = CursorAdapter::new(workspace_root);
        out.push((AgentKind::Cursor, Box::new(ErasedAdapterImpl(a))));
    }
    #[cfg(feature = "claude")]
    {
        let a = ClaudeAdapter::new(workspace_root);
        out.push((AgentKind::Claude, Box::new(ErasedAdapterImpl(a))));
    }
    #[cfg(feature = "codex")]
    {
        let a = CodexAdapter::new(workspace_root);
        out.push((AgentKind::Codex, Box::new(ErasedAdapterImpl(a))));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_adapters_returns_enabled_agents() {
        let adapters = all_adapters(std::path::Path::new("/tmp"));
        assert!(!adapters.is_empty());
        let kinds: Vec<_> = adapters.iter().map(|(k, _)| *k).collect();
        assert!(kinds.contains(&AgentKind::Cursor));
    }

    #[test]
    fn adapters_without_a_writer_decline_by_name_rather_than_no_op() {
        let conversation = lineage_core::Conversation::new(AgentKind::Cursor, "/tmp");
        for (kind, adapter) in all_adapters(std::path::Path::new("/tmp")) {
            if kind == AgentKind::Claude {
                continue;
            }
            let err = adapter
                .render_transcript(&conversation)
                .expect_err("only claude can write a resumable transcript");
            // The agent has to be named: a caller that cannot tell which
            // adapter refused cannot tell the user what to do instead.
            assert!(
                err.to_string().contains(kind.as_str()),
                "{kind:?} declined without naming itself: {err}"
            );
            assert!(err.to_string().contains("unsupported"));
        }
    }

    /// Resuming and transcript writing are separate capabilities: Codex can
    /// reopen a session it already holds but cannot be handed a written one. A
    /// test that only checked "claude yes, everything else no" would pass on an
    /// implementation that collapsed the two.
    #[test]
    fn resume_capability_is_independent_of_transcript_writing() {
        let mut conversation = lineage_core::Conversation::new(AgentKind::Codex, "/tmp");
        conversation.metadata.insert(
            "codex_session_id".into(),
            serde_json::Value::String("codex-abc".into()),
        );

        let adapters = all_adapters(std::path::Path::new("/tmp"));
        let codex = adapters
            .iter()
            .find(|(kind, _)| *kind == AgentKind::Codex)
            .map(|(_, adapter)| adapter)
            .expect("codex adapter is compiled in");

        assert!(codex.render_transcript(&conversation).is_err());
        assert_eq!(
            codex.resume_invocation(&conversation).unwrap().command,
            "codex resume codex-abc"
        );
    }

    #[test]
    fn an_adapter_that_cannot_resume_declines_by_name() {
        let conversation = lineage_core::Conversation::new(AgentKind::Cursor, "/tmp");
        let adapters = all_adapters(std::path::Path::new("/tmp"));
        let cursor = adapters
            .iter()
            .find(|(kind, _)| *kind == AgentKind::Cursor)
            .map(|(_, adapter)| adapter)
            .expect("cursor adapter is compiled in");

        let err = cursor
            .resume_invocation(&conversation)
            .expect_err("cursor sessions cannot be reopened from an id");
        assert!(err.to_string().contains("cursor"), "{err}");
        assert!(err.to_string().contains("unsupported"), "{err}");
    }

    /// A session with no vendor id has to fail differently from an agent that
    /// cannot resume at all: the user's next move is `git lineage fork`, not
    /// "give up on this harness".
    #[test]
    fn a_session_without_a_vendor_id_points_at_fork_instead() {
        let conversation = lineage_core::Conversation::new(AgentKind::Claude, "/tmp");
        let adapters = all_adapters(std::path::Path::new("/tmp"));
        let claude = adapters
            .iter()
            .find(|(kind, _)| *kind == AgentKind::Claude)
            .map(|(_, adapter)| adapter)
            .expect("claude adapter is compiled in");

        let err = claude
            .resume_invocation(&conversation)
            .expect_err("nothing on this machine to reopen");
        assert!(err.to_string().contains("git lineage fork"), "{err}");
    }
}
