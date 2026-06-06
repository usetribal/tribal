use base64::Engine;
use git2::Repository;
use lineage_core::{ArtifactKind, Conversation, LineageError};
use lineage_store::{
    extract_tool_blob_ref, normalize_oid, parse_blob_placeholder, LargeBlobBackend,
    LargeContentStore, LfsStore,
};

use crate::config::read_repo_config;
use crate::lfs_refs::read_lfs_data_from_ref;

#[derive(Debug, Default)]
pub struct HydrateReport {
    pub hydrated_turns: usize,
    pub hydrated_tool_results: usize,
    pub hydrated_media: usize,
    pub missing_blobs: Vec<String>,
}

pub fn hydrate_conversation(
    repo: &Repository,
    conversation: &mut Conversation,
) -> Result<HydrateReport, LineageError> {
    let config = read_repo_config(repo)?;
    let store = LargeContentStore::new(repo.path(), backend_from_config(&config));
    let mut report = HydrateReport::default();

    for turn in &mut conversation.turns {
        if let Some(blob_ref) = parse_blob_placeholder(&turn.content) {
            match load_blob(repo, &store, &blob_ref) {
                Ok(data) => {
                    turn.content = String::from_utf8_lossy(&data).into_owned();
                    report.hydrated_turns += 1;
                }
                Err(_) => report.missing_blobs.push(blob_ref),
            }
        }

        for tc in &mut turn.tool_calls {
            if let Some(blob_ref) = extract_tool_blob_ref(&tc.arguments) {
                match load_blob(repo, &store, &blob_ref) {
                    Ok(data) => {
                        tc.result = Some(String::from_utf8_lossy(&data).into_owned());
                        tc.arguments = tc
                            .arguments
                            .split_whitespace()
                            .filter(|p| !p.starts_with("blob_ref="))
                            .collect::<Vec<_>>()
                            .join(" ");
                        report.hydrated_tool_results += 1;
                    }
                    Err(_) => report.missing_blobs.push(blob_ref),
                }
            }
        }
    }

    report.missing_blobs.sort();
    report.missing_blobs.dedup();
    Ok(report)
}

pub fn hydrate_media_artifacts(
    repo: &Repository,
    conversation: &mut Conversation,
) -> Result<HydrateReport, LineageError> {
    let config = read_repo_config(repo)?;
    let store = LargeContentStore::new(repo.path(), backend_from_config(&config));
    let lfs = LfsStore::new(repo.path());
    let mut report = HydrateReport::default();

    for turn in &mut conversation.turns {
        for artifact in &mut turn.artifacts {
            if !matches!(
                artifact.kind,
                ArtifactKind::Image | ArtifactKind::Diagram | ArtifactKind::Screenshot
            ) {
                continue;
            }
            if artifact.preview_data_url.is_some() {
                continue;
            }
            if artifact.path.starts_with("data:") {
                artifact.preview_data_url = Some(artifact.path.clone());
                report.hydrated_media += 1;
                continue;
            }
            let blob_ref = artifact.blob_ref.as_deref();
            if let Some(bytes) = load_media_bytes(repo, &store, &lfs, blob_ref, artifact.content_hash.as_deref()) {
                let mime = artifact
                    .mime_type
                    .as_deref()
                    .unwrap_or("application/octet-stream");
                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                artifact.preview_data_url = Some(format!("data:{mime};base64,{encoded}"));
                report.hydrated_media += 1;
            } else if let Some(blob_ref) = blob_ref {
                report.missing_blobs.push(blob_ref.to_string());
            }
        }
    }

    report.missing_blobs.sort();
    report.missing_blobs.dedup();
    Ok(report)
}

fn load_media_bytes(
    repo: &Repository,
    store: &LargeContentStore<'_>,
    lfs: &LfsStore,
    blob_ref: Option<&str>,
    content_hash: Option<&str>,
) -> Option<Vec<u8>> {
    if let Some(blob_ref) = blob_ref {
        if store.exists(blob_ref) {
            return store.get(blob_ref).ok();
        }
        let oid = normalize_oid(blob_ref);
        if lfs.exists(&oid) {
            return lfs.get(&oid).ok();
        }
        if let Ok(Some(data)) = read_lfs_data_from_ref(repo, &oid) {
            return Some(data);
        }
    }
    if let Some(hash) = content_hash {
        let oid = normalize_oid(hash);
        if lfs.exists(&oid) {
            return lfs.get(&oid).ok();
        }
    }
    None
}

fn load_blob(
    repo: &Repository,
    store: &LargeContentStore<'_>,
    blob_ref: &str,
) -> Result<Vec<u8>, LineageError> {
    if store.exists(blob_ref) {
        return store.get(blob_ref);
    }
    let oid = lineage_store::normalize_oid(blob_ref);
    if let Some(data) = read_lfs_data_from_ref(repo, &oid)? {
        let _ = store.lfs().put(&data);
        return Ok(data);
    }
    store.get(blob_ref)
}

fn backend_from_config(config: &lineage_core::LineageRepoConfig) -> LargeBlobBackend {
    match config.large_blob_backend {
        lineage_core::LargeBlobBackend::Lfs => LargeBlobBackend::Lfs,
        lineage_core::LargeBlobBackend::Cache => LargeBlobBackend::Cache,
    }
}

pub fn indexable_body(conversation: &Conversation) -> String {
    conversation
        .turns
        .iter()
        .map(|t| t.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, LineageId, Role, Turn};

    #[test]
    fn indexable_body_joins_turn_content() {
        let mut conv = Conversation::new(AgentKind::Cursor, "/tmp");
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "hello".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: "world".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        assert_eq!(indexable_body(&conv), "hello\nworld");
    }
}
