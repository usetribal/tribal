use git2::Repository;
use lineage_core::Conversation;
use lineage_store::{normalize_oid, LargeBlobBackend, LargeContentStore};

use crate::config::read_repo_config;
use crate::lfs_refs::{write_lfs_data_ref, write_lfs_pointer_ref};

pub fn compact_large_content(repo: &Repository, conversation: &mut Conversation) -> Result<(), lineage_core::LineageError> {
    let config = read_repo_config(repo)?;
    let backend = match config.large_blob_backend {
        lineage_core::LargeBlobBackend::Lfs => LargeBlobBackend::Lfs,
        lineage_core::LargeBlobBackend::Cache => LargeBlobBackend::Cache,
    };
    let store = LargeContentStore::new(repo.path(), backend);
    let threshold = config.large_blob_threshold_bytes;

    for turn in &mut conversation.turns {
        if turn.content.len() > threshold {
            let (compact, blob_ref) = store.maybe_externalize(&turn.content, threshold);
            turn.content = compact;
            if let Some(blob_ref) = blob_ref {
                if backend == LargeBlobBackend::Lfs {
                    let oid = normalize_oid(&blob_ref);
                    let data = store.lfs().get(&oid).unwrap_or_default();
                    write_lfs_pointer_ref(repo, &oid, data.len())?;
                    write_lfs_data_ref(repo, &oid, &data)?;
                }
                turn.artifacts.push(lineage_core::Artifact {
                    kind: lineage_core::ArtifactKind::FileEdit,
                    path: format!("turn-{}.blob", turn.id.as_str()),
                    blob_ref: Some(blob_ref),
                    content_hash: None,
                    mime_type: None,
                    preview_data_url: None,
                    line_range: None,
                    resolve: None,
                });
            }
        }
        for tc in &mut turn.tool_calls {
            if let Some(ref result) = tc.result {
                if result.len() > threshold {
                    let (compact, blob_ref) = store.maybe_externalize(result, threshold);
                    tc.result = Some(compact);
                    if let Some(blob_ref) = blob_ref {
                        if backend == LargeBlobBackend::Lfs {
                            let oid = normalize_oid(&blob_ref);
                            if let Ok(data) = store.lfs().get(&oid) {
                                write_lfs_pointer_ref(repo, &oid, data.len())?;
                                write_lfs_data_ref(repo, &oid, &data)?;
                            }
                        }
                        tc.arguments = format!("{} blob_ref={blob_ref}", tc.arguments);
                    }
                }
            }
        }
    }
    Ok(())
}
