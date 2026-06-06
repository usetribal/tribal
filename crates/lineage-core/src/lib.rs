pub mod config;
pub mod conversation_util;
pub mod error;
pub mod ids;
pub mod ingest_state;
pub mod types;

pub use config::{
    CommitMappingMode, LargeBlobBackend, LineageRepoConfig, LfsTransport, LINEAGE_CONFIG_SCHEMA,
};
pub use conversation_util::{
    conversation_modified_code, files_touched, generate_architecture_summary,
};
pub use error::{LineageError, Result};
pub use ids::{derive_line_object_id, derive_session_id, LineageId};
pub use ingest_state::{LastIngestState, LAST_INGEST_SCHEMA};
pub use types::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Confidence, Conversation, GitNote,
    LineObject, LineageManifest, ResolveStrategy, Role, SessionIndex, ToolCall, Turn,
    CONVERSATION_SCHEMA, GIT_NOTES_SCHEMA, LINE_OBJECT_SCHEMA,
};
