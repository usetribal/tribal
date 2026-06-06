use git2::Repository;
use lineage_core::{LastIngestState, LineageError, LAST_INGEST_SCHEMA};
use lineage_store::{GitBlobStore, ObjectStore};

pub const LAST_INGEST_REF: &str = "refs/lineage/last-ingest";

pub fn read_last_ingest(repo: &Repository) -> Result<LastIngestState, LineageError> {
    let oid = match super::refs::read_ref_oid(repo, LAST_INGEST_REF)? {
        Some(o) => o,
        None => return Ok(LastIngestState::default()),
    };
    let store = GitBlobStore::new(repo);
    let data = store.get(&oid.to_string())?;
    let text = String::from_utf8(data).map_err(|e| LineageError::Other(e.to_string()))?;
    let state: LastIngestState = serde_json::from_str(&text).map_err(LineageError::Serde)?;
    if state.schema_version != LAST_INGEST_SCHEMA {
        return Err(LineageError::SchemaVersion {
            expected: LAST_INGEST_SCHEMA.into(),
            actual: state.schema_version,
        });
    }
    Ok(state)
}

pub fn write_last_ingest(repo: &Repository, state: &LastIngestState) -> Result<(), LineageError> {
    let store = GitBlobStore::new(repo);
    let json = serde_json::to_string(state).map_err(LineageError::Serde)?;
    let stored = store.put(json.as_bytes())?;
    super::refs::write_ref_by_name(
        repo,
        LAST_INGEST_REF,
        git2::Oid::from_str(&stored.oid).map_err(|e| LineageError::Other(e.to_string()))?,
    )
}
