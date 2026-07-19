use git2::Repository;
use lineage_core::{Confidence, LineageError, LineageId};

use crate::line_resolve::materialize_line_objects;
use crate::notes::write_note_for_commit;
use crate::patch_id::patch_id_for_commit;
use crate::refs::{list_session_ids, read_conversation_stored};

/// Per-session outcome of a link-to-HEAD pass, carried back so callers (the
/// post-commit hook's event-log entry, diagnostics-v0 `link`) can report which
/// sessions were linked and how many line objects each materialized.
#[derive(Debug, Clone)]
pub struct LinkedSession {
    pub session_id: LineageId,
    pub line_objects: usize,
}

pub fn link_all_sessions_to_head(repo: &Repository) -> Result<Vec<LinkedSession>, LineageError> {
    let ids = list_session_ids(repo)?;
    link_sessions_to_head(repo, &ids)
}

pub fn link_recent_sessions_to_head(repo: &Repository) -> Result<Vec<LinkedSession>, LineageError> {
    let state = crate::import_state::read_last_import(repo)?;
    if state.session_ids.is_empty() {
        return link_all_sessions_to_head(repo);
    }
    link_sessions_to_head(repo, &state.session_ids)
}

fn link_sessions_to_head(
    repo: &Repository,
    ids: &[LineageId],
) -> Result<Vec<LinkedSession>, LineageError> {
    let head = repo
        .head()
        .map_err(|e| LineageError::Other(e.to_string()))?
        .peel_to_commit()
        .map_err(|e| LineageError::Other(e.to_string()))?;

    let sha = head.id().to_string();
    let mut linked = Vec::new();

    for id in ids {
        if let Some(line_objects) = link_session_to_head(repo, &sha, id)? {
            linked.push(LinkedSession {
                session_id: id.clone(),
                line_objects,
            });
        }
    }
    Ok(linked)
}

fn link_session_to_head(
    repo: &Repository,
    commit_sha: &str,
    session_id: &LineageId,
) -> Result<Option<usize>, LineageError> {
    let Some(conversation) = read_conversation_stored(repo, session_id)? else {
        return Ok(None);
    };

    let line_objects =
        materialize_line_objects(repo, &conversation, commit_sha, Confidence::Heuristic)?;
    let mut line_ids: Vec<LineageId> = line_objects.iter().map(|o| o.id.clone()).collect();

    for obj in &line_objects {
        crate::refs::write_line_object(repo, obj)?;
    }

    let mut session_ids = vec![session_id.clone()];
    if let Some(existing) = crate::notes::read_note_for_commit(repo, commit_sha)? {
        session_ids = existing.session_ids.clone();
        if !session_ids.iter().any(|id| id == session_id) {
            session_ids.push(session_id.clone());
        }
        merge_line_ids(&mut line_ids, existing.line_object_ids);
    }

    let patch_id = repo
        .find_commit(
            git2::Oid::from_str(commit_sha).map_err(|e| LineageError::Other(e.to_string()))?,
        )
        .ok()
        .and_then(|c| patch_id_for_commit(repo, &c).ok());

    write_note_for_commit(
        repo,
        commit_sha,
        &session_ids,
        &line_ids,
        patch_id.as_deref(),
    )?;
    Ok(Some(line_objects.len()))
}

fn merge_line_ids(target: &mut Vec<LineageId>, more: Vec<LineageId>) {
    for id in more {
        if !target.iter().any(|existing| existing == &id) {
            target.push(id);
        }
    }
}
