use std::collections::HashSet;

use git2::Repository;
use lineage_core::{LineageError, LineageId};

use crate::blob_gc::{delete_session_ref, list_line_objects_for_session, purge_session_blobs};
use crate::lfs_refs::collect_blob_refs_from_conversation;
use crate::notes::{overwrite_note_for_commit, read_note_for_commit};
use crate::refs::{read_conversation_stored, read_line_object, read_manifest, write_manifest};

#[derive(Debug, Default)]
pub struct DeleteReport {
    pub session_id: String,
    pub notes_updated: usize,
    pub line_objects_deleted: usize,
    pub blobs_purged: usize,
}

pub fn delete_session(
    repo: &Repository,
    session_id: &LineageId,
    purge_blobs: bool,
) -> Result<DeleteReport, LineageError> {
    let conv = read_conversation_stored(repo, session_id)?
        .ok_or_else(|| LineageError::Other(format!("session not found: {session_id}")))?;

    let mut report = DeleteReport {
        session_id: session_id.to_string(),
        ..Default::default()
    };

    let blob_refs = if purge_blobs {
        collect_blob_refs_from_conversation(&conv)
    } else {
        Vec::new()
    };

    let line_object_ids = list_line_objects_for_session(repo, session_id)?;
    let mut commits_to_update: HashSet<String> = conv.commit_shas.iter().cloned().collect();
    for id in &line_object_ids {
        if let Some(obj) = read_line_object(repo, id)? {
            commits_to_update.insert(obj.commit_sha);
        }
    }

    let line_object_set: HashSet<LineageId> = line_object_ids.iter().cloned().collect();

    for sha in commits_to_update {
        if let Some(note) = read_note_for_commit(repo, &sha)? {
            let before_sessions = note.session_ids.len();
            let before_lines = note.line_object_ids.len();
            let sessions: Vec<LineageId> = note
                .session_ids
                .into_iter()
                .filter(|id| id != session_id)
                .collect();
            let line_objects: Vec<LineageId> = note
                .line_object_ids
                .into_iter()
                .filter(|id| !line_object_set.contains(id))
                .collect();
            if sessions.len() != before_sessions || line_objects.len() != before_lines {
                overwrite_note_for_commit(
                    repo,
                    &sha,
                    &sessions,
                    &line_objects,
                    note.patch_id.as_deref(),
                )?;
                report.notes_updated += 1;
            }
        }
    }

    for id in &line_object_ids {
        let ref_name = crate::refs::line_object_ref(id);
        if let Ok(mut reference) = repo.find_reference(&ref_name) {
            reference
                .delete()
                .map_err(|e| LineageError::Other(e.to_string()))?;
            report.line_objects_deleted += 1;
        }
    }

    delete_session_ref(repo, session_id)?;

    let mut manifest = read_manifest(repo)?;
    manifest.sessions.retain(|id| id != session_id);
    write_manifest(repo, &manifest)?;

    if purge_blobs {
        report.blobs_purged = purge_session_blobs(repo, session_id, &blob_refs)?;
    }

    Ok(report)
}
