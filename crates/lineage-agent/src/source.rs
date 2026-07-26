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
    /// The shell command that opens this transcript in its harness, ready to
    /// print or run. Adapter-supplied because the verb and flags are vendor
    /// knowledge (ARCHITECTURE.md invariant 4); a caller that assembled it would
    /// be the exact leak the invariant exists to stop.
    pub resume_command: String,
    /// Directory the resume command must be run from. Claude resolves a session
    /// through a key derived from the launch directory, so running it elsewhere
    /// silently finds nothing.
    pub resume_cwd: PathBuf,
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

/// How to reopen a session that already exists in its harness on this machine.
///
/// Deliberately not a `RenderedTranscript`: that type describes a session being
/// *created* — it mints a handle and carries bytes to write. Resume creates
/// nothing. It names a session the harness already holds, so the only thing a
/// caller needs is the invocation and where to run it.
#[derive(Debug, Clone)]
pub struct ResumeInvocation {
    /// The shell command that reopens the session, ready to print or run.
    /// Adapter-supplied because the verb, the flags, and the id convention are
    /// vendor knowledge (ARCHITECTURE.md invariant 4).
    pub command: String,
    /// Directory the command must be run from, when the harness resolves a
    /// session relative to one. `None` means the harness finds it from anywhere.
    pub cwd: Option<PathBuf>,
}

/// Reopening a session already on this machine, which is a different capability
/// from writing one out: an adapter can know the resume invocation for its
/// harness without being able to produce a transcript the harness will accept
/// (Codex is exactly that). Adapters that cannot resume must say so — a silent
/// no-op would leave the caller believing a session can be reopened.
pub trait SessionResumer: Send + Sync {
    fn resume_invocation(
        &self,
        conversation: &Conversation,
    ) -> Result<ResumeInvocation, LineageError>;
}

/// The error every adapter without a resume invocation returns, so the caller
/// gets one recognisable failure naming the agent rather than per-adapter prose.
pub fn resuming_unsupported(agent: AgentKind) -> LineageError {
    LineageError::Other(format!(
        "resuming is unsupported for {}: its sessions cannot be reopened from a session id",
        agent.as_str()
    ))
}

/// The error an adapter returns when its harness *can* resume but this session
/// carries no vendor id to resume by — a session imported from a teammate's
/// machine, or one whose id was never recorded. Distinct from
/// [`resuming_unsupported`] because the user's next move differs: fork it here,
/// rather than give up on the agent entirely.
pub fn no_vendor_session_id(agent: AgentKind) -> LineageError {
    LineageError::Other(format!(
        "this session carries no {} session id, so there is nothing on this machine to reopen. \
         `git lineage fork` writes a fresh session carrying its context instead",
        agent.as_str()
    ))
}
