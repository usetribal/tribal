use git2::Repository;
use lineage_core::{
    Confidence, Conversation, LineageError, LineageId,
};

use crate::compact::compact_large_content;
use crate::media::externalize_media_artifacts;
use crate::line_resolve::materialize_line_objects;
use crate::patch_id::patch_id_for_commit;
use crate::refs::{read_conversation_stored, read_manifest, write_conversation, write_line_object, write_manifest};

pub struct IngestWriteResult {
    pub session_id: LineageId,
    pub blob_oid: String,
    pub line_objects_written: usize,
    pub commits_linked: usize,
}

fn strip_ephemeral_fields(conversation: &mut Conversation) {
    for turn in &mut conversation.turns {
        for artifact in &mut turn.artifacts {
            artifact.preview_data_url = None;
        }
    }
}

pub fn persist_conversation(
    repo: &Repository,
    conversation: &Conversation,
) -> Result<IngestWriteResult, LineageError> {
    let mut conversation = conversation.clone();
    strip_ephemeral_fields(&mut conversation);
    externalize_media_artifacts(repo, &mut conversation)?;
    compact_large_content(repo, &mut conversation)?;

    let blob_oid = write_conversation(repo, &conversation)?;

    let mut manifest = read_manifest(repo)?;
    if !manifest.sessions.contains(&conversation.id) {
        manifest.sessions.push(conversation.id.clone());
    }
    manifest.schema_version = lineage_core::GIT_NOTES_SCHEMA.into();
    write_manifest(repo, &manifest)?;

    let mut line_objects_written = 0usize;
    let mut commits_linked = 0usize;

    for commit_sha in &conversation.commit_shas {
        let line_objects =
            materialize_line_objects(repo, &conversation, commit_sha, Confidence::Exact)?;
        let mut line_ids = Vec::new();
        for obj in &line_objects {
            write_line_object(repo, obj)?;
            line_ids.push(obj.id.clone());
            line_objects_written += 1;
        }
        write_note_for_commit_with_patch(
            repo,
            commit_sha,
            std::slice::from_ref(&conversation.id),
            &line_ids,
        )?;
        commits_linked += 1;
    }

    Ok(IngestWriteResult {
        session_id: conversation.id.clone(),
        blob_oid,
        line_objects_written,
        commits_linked,
    })
}

pub fn persist_ingest(
    repo: &Repository,
    conversations: &[Conversation],
) -> Result<Vec<IngestWriteResult>, LineageError> {
    conversations
        .iter()
        .map(|c| persist_conversation(repo, c))
        .collect()
}

pub fn link_session_to_commit(
    repo: &Repository,
    session_id: &LineageId,
    commit_sha: &str,
) -> Result<usize, LineageError> {
    let conversation = read_conversation_stored(repo, session_id)?
        .ok_or_else(|| LineageError::Other(format!("session not found: {session_id}")))?;

    let line_objects =
        materialize_line_objects(repo, &conversation, commit_sha, Confidence::Manual)?;
    let mut line_ids = Vec::new();
    for obj in &line_objects {
        write_line_object(repo, obj)?;
        line_ids.push(obj.id.clone());
    }
    write_note_for_commit_with_patch(
        repo,
        commit_sha,
        std::slice::from_ref(session_id),
        &line_ids,
    )?;
    Ok(line_objects.len())
}

pub fn materialize_session_at_commit(
    repo: &Repository,
    session_id: &LineageId,
    commit_sha: &str,
) -> Result<usize, LineageError> {
    let conversation = read_conversation_stored(repo, session_id)?
        .ok_or_else(|| LineageError::Other(format!("session not found: {session_id}")))?;

    let line_objects =
        materialize_line_objects(repo, &conversation, commit_sha, Confidence::Exact)?;
    let mut line_ids = Vec::new();
    for obj in &line_objects {
        write_line_object(repo, obj)?;
        line_ids.push(obj.id.clone());
    }

    let mut note_sessions = vec![session_id.clone()];
    if let Some(existing) = crate::notes::read_note_for_commit(repo, commit_sha)? {
        note_sessions = existing.session_ids.clone();
        if !note_sessions.iter().any(|id| id == session_id) {
            note_sessions.push(session_id.clone());
        }
        merge_line_ids(&mut line_ids, existing.line_object_ids);
    }

    write_note_for_commit_with_patch(repo, commit_sha, &note_sessions, &line_ids)?;
    Ok(line_objects.len())
}

fn write_note_for_commit_with_patch(
    repo: &Repository,
    commit_sha: &str,
    session_ids: &[LineageId],
    line_object_ids: &[LineageId],
) -> Result<(), LineageError> {
    let patch_id = commit_oid(repo, commit_sha).and_then(|oid| {
        repo.find_commit(oid)
            .ok()
            .and_then(|c| patch_id_for_commit(repo, &c).ok())
    });
    crate::notes::write_note_for_commit(
        repo,
        commit_sha,
        session_ids,
        line_object_ids,
        patch_id.as_deref(),
    )
}

fn commit_oid(repo: &Repository, sha: &str) -> Option<git2::Oid> {
    git2::Oid::from_str(sha).ok().filter(|oid| repo.find_commit(*oid).is_ok())
}

fn merge_line_ids(target: &mut Vec<LineageId>, more: Vec<LineageId>) {
    for id in more {
        if !target.iter().any(|existing| existing == &id) {
            target.push(id);
        }
    }
}
