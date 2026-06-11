use git2::{Oid, Repository};
use lineage_core::{GitNote, LineageError, LineageId};

use crate::refs::LINEAGE_NOTES_REF;

pub fn overwrite_note_for_commit(
    repo: &Repository,
    commit_sha: &str,
    session_ids: &[LineageId],
    line_object_ids: &[LineageId],
    patch_id: Option<&str>,
) -> Result<(), LineageError> {
    let mut note = GitNote::new(commit_sha);
    note.session_ids = session_ids.to_vec();
    note.line_object_ids = line_object_ids.to_vec();
    if let Some(patch_id) = patch_id {
        note.patch_id = Some(patch_id.to_string());
    }
    write_note_json(repo, commit_sha, &note)
}

pub fn write_note_for_commit(
    repo: &Repository,
    commit_sha: &str,
    session_ids: &[LineageId],
    line_object_ids: &[LineageId],
    patch_id: Option<&str>,
) -> Result<(), LineageError> {
    let mut note =
        read_note_for_commit(repo, commit_sha)?.unwrap_or_else(|| GitNote::new(commit_sha));
    for id in session_ids {
        if !note.session_ids.contains(id) {
            note.session_ids.push(id.clone());
        }
    }
    for id in line_object_ids {
        if !note.line_object_ids.contains(id) {
            note.line_object_ids.push(id.clone());
        }
    }
    if let Some(patch_id) = patch_id {
        note.patch_id = Some(patch_id.to_string());
    }

    write_note_json(repo, commit_sha, &note)
}

fn write_note_json(
    repo: &Repository,
    commit_sha: &str,
    note: &GitNote,
) -> Result<(), LineageError> {
    let commit_oid = Oid::from_str(commit_sha)
        .map_err(|e| LineageError::Other(format!("invalid commit: {e}")))?;
    let json = note.to_json()?;
    let sig = repo
        .signature()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    repo.note_delete(commit_oid, Some(LINEAGE_NOTES_REF), &sig, &sig)
        .ok();
    repo.note(&sig, &sig, Some(LINEAGE_NOTES_REF), commit_oid, &json, true)
        .map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(())
}

pub fn read_note_for_commit(
    repo: &Repository,
    commit_sha: &str,
) -> Result<Option<GitNote>, LineageError> {
    let commit_oid = Oid::from_str(commit_sha)
        .map_err(|e| LineageError::Other(format!("invalid commit: {e}")))?;

    match repo.find_note(Some(LINEAGE_NOTES_REF), commit_oid) {
        Ok(note) => {
            let text = note
                .message()
                .ok_or_else(|| LineageError::Other("empty note message".into()))?;
            Ok(Some(GitNote::from_json(text)?))
        }
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(e) => Err(LineageError::Other(e.to_string())),
    }
}

pub fn map_commit_to_sessions(
    repo: &Repository,
    commit_sha: &str,
) -> Result<Vec<LineageId>, LineageError> {
    Ok(read_note_for_commit(repo, commit_sha)?
        .map(|n| n.session_ids)
        .unwrap_or_default())
}
