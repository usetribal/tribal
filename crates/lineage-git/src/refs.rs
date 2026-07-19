use git2::Repository;
use lineage_core::{Conversation, LineObject, LineageError, LineageId, LineageManifest};
use lineage_store::{GitBlobStore, ObjectStore};

pub const LINEAGE_INDEX_REF: &str = "refs/lineage/index";
pub const LINEAGE_NOTES_REF: &str = "refs/notes/lineage";

pub fn read_ref_oid(repo: &Repository, ref_name: &str) -> Result<Option<git2::Oid>, LineageError> {
    match repo.find_reference(ref_name) {
        Ok(r) => Ok(Some(r.target().ok_or_else(|| {
            LineageError::Other(format!("ref {ref_name} has no target"))
        })?)),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(e) => Err(LineageError::Other(e.to_string())),
    }
}

pub fn write_ref_by_name(
    repo: &Repository,
    ref_name: &str,
    oid: git2::Oid,
) -> Result<(), LineageError> {
    write_ref(repo, ref_name, oid)
}

pub fn session_ref(session_id: &LineageId) -> String {
    format!("refs/lineage/sessions/{}", session_id.as_str())
}

pub fn line_object_ref(object_id: &LineageId) -> String {
    format!("refs/lineage/lines/{}", object_id.as_str())
}

fn write_ref(repo: &Repository, ref_name: &str, oid: git2::Oid) -> Result<(), LineageError> {
    let mut reference = if let Ok(r) = repo.find_reference(ref_name) {
        r
    } else {
        repo.reference(ref_name, oid, true, "lineage: update ref")
            .map_err(|e| LineageError::Other(e.to_string()))?
    };
    reference
        .set_target(oid, "lineage: update ref")
        .map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(())
}

pub fn write_conversation(
    repo: &Repository,
    conversation: &Conversation,
) -> Result<String, LineageError> {
    let store = GitBlobStore::new(repo);
    let json = conversation.to_json()?;
    let stored = store.put(json.as_bytes())?;
    write_ref(
        repo,
        &session_ref(&conversation.id),
        git2::Oid::from_str(&stored.oid).map_err(|e| LineageError::Other(e.to_string()))?,
    )?;
    Ok(stored.oid)
}

pub fn read_conversation_stored(
    repo: &Repository,
    session_id: &LineageId,
) -> Result<Option<Conversation>, LineageError> {
    let oid = match read_ref_oid(repo, &session_ref(session_id))? {
        Some(o) => o,
        None => return Ok(None),
    };
    let store = GitBlobStore::new(repo);
    let data = store.get(&oid.to_string())?;
    let text = String::from_utf8(data).map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(Some(Conversation::from_json(&text)?))
}

pub fn read_conversation(
    repo: &Repository,
    session_id: &LineageId,
) -> Result<Option<Conversation>, LineageError> {
    let mut conv = match read_conversation_stored(repo, session_id)? {
        Some(c) => c,
        None => return Ok(None),
    };
    let _ = crate::hydrate::hydrate_conversation(repo, &mut conv)?;
    Ok(Some(conv))
}

pub fn write_line_object(repo: &Repository, object: &LineObject) -> Result<String, LineageError> {
    let store = GitBlobStore::new(repo);
    let json = object.to_json()?;
    let stored = store.put(json.as_bytes())?;
    write_ref(
        repo,
        &line_object_ref(&object.id),
        git2::Oid::from_str(&stored.oid).map_err(|e| LineageError::Other(e.to_string()))?,
    )?;
    Ok(stored.oid)
}

pub fn read_line_object(
    repo: &Repository,
    object_id: &LineageId,
) -> Result<Option<LineObject>, LineageError> {
    let oid = match read_ref_oid(repo, &line_object_ref(object_id))? {
        Some(o) => o,
        None => return Ok(None),
    };
    let store = GitBlobStore::new(repo);
    let data = store.get(&oid.to_string())?;
    let text = String::from_utf8(data).map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(Some(LineObject::from_json(&text)?))
}

pub fn read_manifest(repo: &Repository) -> Result<LineageManifest, LineageError> {
    let oid = match read_ref_oid(repo, LINEAGE_INDEX_REF)? {
        Some(o) => o,
        None => return Ok(LineageManifest::default()),
    };
    let store = GitBlobStore::new(repo);
    let data = store.get(&oid.to_string())?;
    let text = String::from_utf8(data).map_err(|e| LineageError::Other(e.to_string()))?;
    serde_json::from_str(&text).map_err(LineageError::Serde)
}

pub fn write_manifest(repo: &Repository, manifest: &LineageManifest) -> Result<(), LineageError> {
    let store = GitBlobStore::new(repo);
    let json = serde_json::to_string_pretty(manifest).map_err(LineageError::Serde)?;
    let stored = store.put(json.as_bytes())?;
    write_ref(
        repo,
        LINEAGE_INDEX_REF,
        git2::Oid::from_str(&stored.oid).map_err(|e| LineageError::Other(e.to_string()))?,
    )?;
    Ok(())
}

pub fn list_session_ids(repo: &Repository) -> Result<Vec<LineageId>, LineageError> {
    Ok(read_manifest(repo)?.sessions)
}

/// Every stored line object. Unreadable refs are skipped, not errors, so
/// diagnostics over a partially broken repo still see the rest.
pub fn list_line_objects(repo: &Repository) -> Result<Vec<LineObject>, LineageError> {
    const PREFIX: &str = "refs/lineage/lines/";
    let mut out = Vec::new();
    for reference in repo
        .references_glob(&format!("{PREFIX}*"))
        .map_err(|e| LineageError::Other(e.to_string()))?
        .filter_map(|r| r.ok())
    {
        let Some(suffix) = reference.name().and_then(|n| n.strip_prefix(PREFIX)) else {
            continue;
        };
        if let Ok(Some(obj)) = read_line_object(repo, &LineageId::from(suffix)) {
            out.push(obj);
        }
    }
    Ok(out)
}
