use lineage_core::{AgentKind, Conversation, LineageError};
use lineage_policy::{apply_policy, PolicyConfig};
use tracing::{info, warn};

use crate::source::{AgentSource, SessionReader, SessionRef};

#[derive(Debug)]
pub struct SessionError {
    pub source_path: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct IngestResult {
    pub conversations: Vec<Conversation>,
    pub errors: Vec<SessionError>,
    pub redactions_applied: usize,
    pub artifacts_removed: usize,
}

pub struct IngestPipeline {
    pub policy: PolicyConfig,
    pub link_head_commit: bool,
}

impl Default for IngestPipeline {
    fn default() -> Self {
        Self {
            policy: PolicyConfig::default_safe(),
            link_head_commit: true,
        }
    }
}

impl IngestPipeline {
    pub fn ingest<S, R>(&self, source: &S, reader: &R) -> IngestResult
    where
        S: AgentSource,
        R: SessionReader,
    {
        let mut result = IngestResult::default();

        let sessions = match source.discover() {
            Ok(s) => s,
            Err(e) => {
                result.errors.push(SessionError {
                    source_path: String::new(),
                    message: e.to_string(),
                });
                return result;
            }
        };

        info!(agent = %source.agent().as_str(), count = sessions.len(), "discovered sessions");

        for session_ref in sessions {
            match self.ingest_one(reader, &session_ref) {
                Ok(Some(conv)) => result.conversations.push(conv),
                Ok(None) => {}
                Err(e) => result.errors.push(SessionError {
                    source_path: session_ref.source_path.display().to_string(),
                    message: e.to_string(),
                }),
            }
        }

        result
    }

    pub fn ingest_one<R: SessionReader>(
        &self,
        reader: &R,
        session_ref: &SessionRef,
    ) -> Result<Option<Conversation>, LineageError> {
        let conversation = reader.read(session_ref)?;
        let policy_result = apply_policy(&self.policy, conversation);
        if policy_result.redactions_applied > 0 {
            warn!(
                session = %policy_result.conversation.id,
                redactions = policy_result.redactions_applied,
                "applied redactions"
            );
        }
        Ok(Some(policy_result.conversation))
    }

    pub fn ingest_all<S, R>(&self, sources: &[(S, R)]) -> IngestResult
    where
        S: AgentSource,
        R: SessionReader,
    {
        let mut combined = IngestResult::default();
        for (source, reader) in sources {
            let partial = self.ingest(source, reader);
            combined.conversations.extend(partial.conversations);
            combined.errors.extend(partial.errors);
            combined.redactions_applied += partial.redactions_applied;
            combined.artifacts_removed += partial.artifacts_removed;
        }
        combined
    }

    pub fn filter_agent(sources: &[AgentKind], agent: AgentKind) -> bool {
        sources.is_empty() || sources.contains(&agent)
    }
}
