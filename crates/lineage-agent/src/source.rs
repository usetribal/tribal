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
