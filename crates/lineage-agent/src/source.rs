use std::path::PathBuf;

use chrono::{DateTime, Utc};
use lineage_core::{AgentKind, Conversation, LineageError};

pub struct SessionRef {
    pub id_hint: String,
    pub agent: AgentKind,
    pub source_path: PathBuf,
    pub started_at: Option<DateTime<Utc>>,
}

pub trait AgentSource: Send + Sync {
    fn agent(&self) -> AgentKind;
    fn discover(&self) -> Result<Vec<SessionRef>, LineageError>;
}

pub trait SessionReader: Send + Sync {
    fn read(&self, session: &SessionRef) -> Result<Conversation, LineageError>;
}

/// A vendor-native transcript rendered from a `Conversation`, plus everything
/// the harness needs to open it. Callers write `contents` to `path` and hand
/// `session_handle` to the harness; they never construct either themselves,
/// because the path encoding and the id convention are adapter knowledge
/// (ARCHITECTURE.md invariant 4).
#[derive(Debug, Clone)]
pub struct RenderedTranscript {
    /// Absolute path the harness will look for this session at.
    pub path: PathBuf,
    pub contents: String,
    /// The vendor id the harness resolves the session by. Freshly minted, never
    /// the source session's id: two users sharing a machine would otherwise
    /// collide, and the id would stop identifying one session.
    pub session_handle: String,
}

/// Writing a transcript back is a separate capability from reading one: most
/// adapters can parse a vendor format without being able to produce one the
/// harness will accept. Adapters that cannot must say so — a silent no-op
/// would leave the caller believing a session is resumable when it is not.
pub trait TranscriptWriter: Send + Sync {
    fn render_transcript(
        &self,
        conversation: &Conversation,
    ) -> Result<RenderedTranscript, LineageError>;
}

/// The error every adapter without a transcript writer returns, so the caller
/// gets one recognisable failure naming the agent rather than per-adapter prose.
pub fn transcript_writing_unsupported(agent: AgentKind) -> LineageError {
    LineageError::Other(format!(
        "writing a resumable transcript is unsupported for {}: only claude sessions can be continued in their harness",
        agent.as_str()
    ))
}
