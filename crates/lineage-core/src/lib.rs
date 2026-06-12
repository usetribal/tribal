pub mod config;
pub mod conversation_util;
pub mod error;
pub mod ids;
pub mod import_state;
pub mod sync;
pub mod types;

pub use config::{
    CommitMappingMode, LargeBlobBackend, LfsTransport, LineageRepoConfig, LINEAGE_CONFIG_SCHEMA,
};
pub use conversation_util::{
    conversation_modified_code, files_touched, generate_architecture_summary,
};
pub use error::{LineageError, Result};
pub use ids::{derive_line_object_id, derive_session_id, LineageId};
pub use import_state::{
    is_valid_import_schema, LastImportState, LAST_IMPORT_SCHEMA, LAST_IMPORT_SCHEMA_LEGACY,
};
pub use sync::{
    BlobManifestEntry, RepoBinding, SessionCommitLink, SyncBatch, SyncObjectKind, SyncObjectResult,
    SyncObjectStatus, SyncRejectReason, SyncResponse, SYNC_BATCH_SCHEMA, SYNC_RESPONSE_SCHEMA,
};
pub use types::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Confidence, Conversation, GitNote,
    LineObject, LineageManifest, ResolveStrategy, Role, SessionIndex, ToolCall, Turn,
    CONVERSATION_SCHEMA, GIT_NOTES_SCHEMA, LINE_OBJECT_SCHEMA,
};
