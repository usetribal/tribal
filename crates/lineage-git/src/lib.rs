mod ancestry;
mod blame;
mod blob_gc;
mod commit_map;
mod compact;
mod config;
mod coverage;
mod delete;
mod doctor;
mod funnel;
mod hooks;
mod hydrate;
mod identity;
mod import_state;
pub mod lfs_batch;
mod lfs_ops;
mod lfs_refs;
mod lfs_worktree;
mod line_resolve;
mod media;
mod notes;
pub mod patch_id;
mod rebuild;
mod refs;
mod remap;
mod repo;
mod session_resolve;
mod sync;
mod write;

pub use ancestry::{
    commit_time, resolve_anchor, walk_line_ancestry, walk_line_ancestry_shared, AncestryHop,
    AncestryParent,
};
pub use blame::{blame_with_lineage, BlameMatch, BlameResult};
pub use blob_gc::{purge_orphans, PurgeReport};
pub use commit_map::{best_commit_for_conversation, map_conversation_to_commits, CommitMatch};
pub use config::{read_repo_config, write_repo_config, LINEAGE_CONFIG_REF};
pub use coverage::{
    commit_note_coverage, summarize_coverage, tracked_file_line_counts, CoverageReport,
    COVERAGE_BUCKETS,
};
pub use delete::{delete_session, DeleteReport};
pub use doctor::{run_doctor, run_doctor_refs, DoctorReport};
pub use funnel::{audit_materialization, MaterializationFunnel};
pub use hooks::{
    link_all_sessions_to_head, link_recent_sessions_to_head, link_sessions_to_commit, LinkBasis,
    LinkReport, LinkedSession,
};
pub use hydrate::{hydrate_conversation, hydrate_media_artifacts, HydrateReport};
pub use identity::{
    repo_git_identity, stamp_prompted_by, GitIdentity, PROMPTED_BY_EMAIL, PROMPTED_BY_NAME,
};
pub use import_state::{read_last_import, write_last_import, LAST_IMPORT_REF};
pub use lfs_ops::{lfs_fetch, lfs_push, lfs_status, LfsStatusReport, LfsTransferReport};
pub use lfs_refs::{
    collect_all_blob_refs, collect_blob_refs_from_conversation, lfs_data_ref, lfs_pointer_ref,
    list_lfs_data_refs, read_lfs_data_from_ref, read_lfs_pointer_ref, write_lfs_data_ref,
    write_lfs_pointer_ref, LFS_DATA_REF_PREFIX, LFS_POINTER_REF_PREFIX,
};
pub use lfs_worktree::{ensure_gitattributes, LINEAGE_MEDIA_DIR};
pub use line_resolve::{
    materialize_line_objects, materialize_line_objects_with_paths, resolve_old_string,
};
pub use notes::{list_notes, map_commit_to_sessions, read_note_for_commit, write_note_for_commit};
pub use rebuild::{rebuild_links, rebuild_links_with_progress, RebuildReport};
pub use refs::{
    list_line_objects, list_session_ids, read_conversation, read_conversation_stored,
    read_line_object, read_manifest, session_ref, write_conversation, write_line_object,
    write_manifest, LINEAGE_INDEX_REF, LINEAGE_NOTES_REF,
};
pub use remap::{remap_orphaned_commits, RemapReport};
pub use repo::{find_repo, open_repo, repo_paths, repo_paths_for_conversation, LineageRepo};
pub use session_resolve::{load_sessions, resolve_session, ResolveError, SessionCandidate};
pub use sync::{
    assemble_batch, chunk_batch, normalize_remote_url, read_git_config, remove_git_config,
    resolve_repo_binding, sync_push, sync_push_with_progress, write_git_config, SyncOutcome,
    SyncReport, SERVER_REPO_ID_KEY, SYNC_CONVERSATIONS_PER_CHUNK,
};
pub use write::{
    link_session_to_commit, materialize_session_at_commit, persist_conversation, persist_import,
};
