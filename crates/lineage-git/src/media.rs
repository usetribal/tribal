use base64::Engine;
use git2::Repository;
use lineage_core::{Artifact, ArtifactKind, Conversation, LineageError};
use lineage_store::{format_blob_ref, LfsStore};

use crate::lfs_refs::{write_lfs_data_ref, write_lfs_pointer_ref};
use crate::lfs_worktree::{ensure_gitattributes, worktree_media_path, write_worktree_pointer};

const DATA_URL_PREFIX: &str = "data:";

pub fn externalize_media_artifacts(repo: &Repository, conversation: &mut Conversation) -> Result<(), LineageError> {
    let _ = ensure_gitattributes(repo);
    let lfs = LfsStore::new(repo.path());

    for turn in &mut conversation.turns {
        for artifact in &mut turn.artifacts {
            if artifact.content_hash.is_some() || artifact.blob_ref.is_some() {
                continue;
            }
            if !matches!(
                artifact.kind,
                ArtifactKind::Image | ArtifactKind::Diagram | ArtifactKind::Screenshot
            ) {
                continue;
            }
            // Path may hold a data URL or file path reference from adapter
            if let Some(bytes) = decode_artifact_bytes(&artifact.path) {
                store_media_artifact(repo, &lfs, artifact, &bytes)?;
            }
        }

        // Scan turn content for embedded data URLs
        if turn.content.contains(DATA_URL_PREFIX) {
            if let Some((mime, bytes)) = extract_data_url(&turn.content) {
                let kind = if mime.starts_with("image/") {
                    ArtifactKind::Image
                } else {
                    ArtifactKind::Diagram
                };
                let mut artifact = Artifact {
                    kind,
                    path: "embedded".into(),
                    blob_ref: None,
                    content_hash: None,
                    mime_type: Some(mime),
                    preview_data_url: None,
                    line_range: None,
                    resolve: None,
                };
                store_media_artifact(repo, &lfs, &mut artifact, &bytes)?;
                turn.artifacts.push(artifact);
            }
        }
    }
    Ok(())
}

fn store_media_artifact(
    repo: &Repository,
    lfs: &LfsStore,
    artifact: &mut Artifact,
    bytes: &[u8],
) -> Result<(), LineageError> {
    let obj = lfs.put(bytes)?;
    artifact.content_hash = Some(obj.oid.clone());
    artifact.blob_ref = Some(format_blob_ref(&obj.oid));
    if artifact.mime_type.is_none() {
        artifact.mime_type = guess_mime(bytes);
    }
    write_lfs_pointer_ref(repo, &obj.oid, obj.size)?;
    write_lfs_data_ref(repo, &obj.oid, bytes)?;
    let _ = write_worktree_pointer(repo, &obj.oid, obj.size)?;
    artifact.path = worktree_media_path(&obj.oid);
    Ok(())
}

fn decode_artifact_bytes(path: &str) -> Option<Vec<u8>> {
    if path.starts_with(DATA_URL_PREFIX) {
        extract_data_url(path).map(|(_, b)| b)
    } else {
        std::fs::read(path).ok()
    }
}

fn extract_data_url(s: &str) -> Option<(String, Vec<u8>)> {
    let start = s.find(DATA_URL_PREFIX)?;
    let fragment = &s[start..];
    let comma = fragment.find(',')?;
    let header = &fragment[..comma];
    let data = &fragment[comma + 1..];
    if !header.contains(";base64") {
        return None;
    }
    let mime = header
        .strip_prefix(DATA_URL_PREFIX)?
        .split(';')
        .next()?
        .to_string();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .ok()?;
    Some((mime, bytes))
}

fn guess_mime(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("image/png".into())
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg".into())
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif".into())
    } else {
        Some("application/octet-stream".into())
    }
}
