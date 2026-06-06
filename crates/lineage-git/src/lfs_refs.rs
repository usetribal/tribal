use git2::Repository;
use lineage_core::LineageError;
use lineage_store::{GitBlobStore, LfsStore, ObjectStore};

pub const LFS_POINTER_REF_PREFIX: &str = "refs/lineage/lfs/";
pub const LFS_DATA_REF_PREFIX: &str = "refs/lineage/lfs-data/";

pub fn lfs_pointer_ref(oid: &str) -> String {
    format!("{LFS_POINTER_REF_PREFIX}{oid}")
}

pub fn lfs_data_ref(oid: &str) -> String {
    format!("{LFS_DATA_REF_PREFIX}{oid}")
}

pub fn write_lfs_pointer_ref(
    repo: &Repository,
    oid: &str,
    size: usize,
) -> Result<(), LineageError> {
    let pointer = LfsStore::pointer_text(oid, size);
    let store = GitBlobStore::new(repo);
    let stored = store.put(pointer.as_bytes())?;
    super::refs::write_ref_by_name(
        repo,
        &lfs_pointer_ref(oid),
        git2::Oid::from_str(&stored.oid).map_err(|e| LineageError::Other(e.to_string()))?,
    )
}

pub fn write_lfs_data_ref(repo: &Repository, oid: &str, data: &[u8]) -> Result<(), LineageError> {
    let store = GitBlobStore::new(repo);
    let stored = store.put(data)?;
    super::refs::write_ref_by_name(
        repo,
        &lfs_data_ref(oid),
        git2::Oid::from_str(&stored.oid).map_err(|e| LineageError::Other(e.to_string()))?,
    )
}

pub fn read_lfs_pointer_ref(repo: &Repository, oid: &str) -> Result<Option<String>, LineageError> {
    let oid_ref = match super::refs::read_ref_oid(repo, &lfs_pointer_ref(oid))? {
        Some(o) => o,
        None => return Ok(None),
    };
    let store = GitBlobStore::new(repo);
    let data = store.get(&oid_ref.to_string())?;
    Ok(Some(String::from_utf8_lossy(&data).into_owned()))
}

pub fn read_lfs_data_from_ref(repo: &Repository, oid: &str) -> Result<Option<Vec<u8>>, LineageError> {
    let oid_ref = match super::refs::read_ref_oid(repo, &lfs_data_ref(oid))? {
        Some(o) => o,
        None => return Ok(None),
    };
    let store = GitBlobStore::new(repo);
    Ok(Some(store.get(&oid_ref.to_string())?))
}

pub fn list_lfs_data_refs(repo: &Repository) -> Result<Vec<String>, LineageError> {
    Ok(repo
        .references_glob(&format!("{LFS_DATA_REF_PREFIX}*"))
        .map_err(|e| LineageError::Other(e.to_string()))?
        .filter_map(|r| r.ok())
        .filter_map(|r| r.name().map(|n| n.strip_prefix(LFS_DATA_REF_PREFIX).unwrap_or(n).to_string()))
        .collect())
}

pub fn collect_blob_refs_from_conversation(
    conversation: &lineage_core::Conversation,
) -> Vec<String> {
    use lineage_store::{extract_tool_blob_ref, parse_blob_placeholder};

    let mut refs = Vec::new();
    for turn in &conversation.turns {
        if let Some(blob_ref) = parse_blob_placeholder(&turn.content) {
            refs.push(blob_ref);
        }
        for artifact in &turn.artifacts {
            if let Some(blob_ref) = &artifact.blob_ref {
                refs.push(blob_ref.clone());
            }
        }
        for tc in &turn.tool_calls {
            if let Some(blob_ref) = extract_tool_blob_ref(&tc.arguments) {
                refs.push(blob_ref);
            }
        }
    }
    refs.sort();
    refs.dedup();
    refs
}

pub fn collect_all_blob_refs(repo: &Repository) -> Result<Vec<String>, LineageError> {
    let mut refs = Vec::new();
    for id in super::refs::list_session_ids(repo)? {
        if let Some(conv) = super::refs::read_conversation_stored(repo, &id)? {
            refs.extend(collect_blob_refs_from_conversation(&conv));
        }
    }
    refs.sort();
    refs.dedup();
    Ok(refs)
}
