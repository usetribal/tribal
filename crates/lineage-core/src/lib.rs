pub mod config;
pub mod conversation_util;
pub mod error;
pub mod ids;
pub mod import_state;
pub mod path_util;
pub mod salience;
pub mod sync;
pub mod types;

pub use config::{
    CommitMappingMode, LargeBlobBackend, LfsTransport, LineageRepoConfig, LINEAGE_CONFIG_SCHEMA,
};
pub use conversation_util::{
    conversation_modified_code, enriched_indexable_body, files_touched, files_written,
    generate_architecture_summary, session_chunks, turn_indexable_text, SessionChunk,
    DEFAULT_CHUNK_MAX_CHARS,
};
pub use error::{LineageError, Result};
pub use ids::{derive_line_object_id, derive_session_id, LineageId};
pub use import_state::{
    is_valid_import_schema, LastImportState, LAST_IMPORT_SCHEMA, LAST_IMPORT_SCHEMA_LEGACY,
};
pub use path_util::{normalize_repo_path_unscoped, workspace_root_for, PathOrigin, RepoPaths};
pub use salience::{turn_is_salient, turn_salience, SalienceClass};
pub use sync::{
    BlobManifestEntry, RepoBinding, SessionCommitLink, SyncBatch, SyncObjectKind, SyncObjectResult,
    SyncObjectStatus, SyncRejectReason, SyncResponse, SYNC_BATCH_SCHEMA, SYNC_RESPONSE_SCHEMA,
};
pub use types::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Confidence, Conversation, GitNote,
    LineObject, LineageManifest, ResolveStrategy, Role, SessionIndex, ToolCall, Turn,
    CONVERSATION_SCHEMA, GIT_NOTES_SCHEMA, LINE_OBJECT_SCHEMA,
};
