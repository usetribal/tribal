mod citations;
mod content;
mod metadata;
mod path_util;

pub use citations::{enrich_turn_with_citations, extract_citations_from_text};

#[cfg(feature = "claude")]
pub mod claude;
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

use lineage_agent::{AgentSource, SessionReader};
use lineage_core::AgentKind;

pub trait ErasedAdapter: Send + Sync {
    fn agent(&self) -> AgentKind;
    fn discover(&self) -> lineage_core::Result<Vec<lineage_agent::SessionRef>>;
    fn read(
        &self,
        session: &lineage_agent::SessionRef,
    ) -> lineage_core::Result<lineage_core::Conversation>;
}

struct ErasedAdapterImpl<A>(A);

impl<A> ErasedAdapter for ErasedAdapterImpl<A>
where
    A: AgentSource + SessionReader + Send + Sync,
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
}
