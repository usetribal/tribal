use git2::Repository;
use lineage_core::{is_valid_import_schema, LastImportState, LineageError, LAST_IMPORT_SCHEMA};
use lineage_store::{GitBlobStore, ObjectStore};

pub const LAST_IMPORT_REF: &str = "refs/lineage/last-import";
const LAST_IMPORT_REF_LEGACY: &str = "refs/lineage/last-ingest";

pub fn read_last_import(repo: &Repository) -> Result<LastImportState, LineageError> {
    if let Some(state) = read_last_import_at(repo, LAST_IMPORT_REF)? {
        return Ok(state);
    }
    Ok(read_last_import_at(repo, LAST_IMPORT_REF_LEGACY)?.unwrap_or_default())
}

fn read_last_import_at(
    repo: &Repository,
    ref_name: &str,
) -> Result<Option<LastImportState>, LineageError> {
    let oid = match super::refs::read_ref_oid(repo, ref_name)? {
        Some(o) => o,
        None => return Ok(None),
    };
    let store = GitBlobStore::new(repo);
    let data = store.get(&oid.to_string())?;
    let text = String::from_utf8(data).map_err(|e| LineageError::Other(e.to_string()))?;
    let state: LastImportState = serde_json::from_str(&text).map_err(LineageError::Serde)?;
    if !is_valid_import_schema(&state.schema_version) {
        return Err(LineageError::SchemaVersion {
            expected: LAST_IMPORT_SCHEMA.into(),
            actual: state.schema_version,
        });
    }
    Ok(Some(state))
}

pub fn write_last_import(repo: &Repository, state: &LastImportState) -> Result<(), LineageError> {
    let store = GitBlobStore::new(repo);
    let mut to_write = state.clone();
    to_write.schema_version = LAST_IMPORT_SCHEMA.into();
    let json = serde_json::to_string(&to_write).map_err(LineageError::Serde)?;
    let stored = store.put(json.as_bytes())?;
    super::refs::write_ref_by_name(
        repo,
        LAST_IMPORT_REF,
        git2::Oid::from_str(&stored.oid).map_err(|e| LineageError::Other(e.to_string()))?,
    )
}
