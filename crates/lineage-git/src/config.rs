use git2::Repository;
use lineage_core::{LineageError, LineageRepoConfig, LINEAGE_CONFIG_SCHEMA};
use lineage_store::{GitBlobStore, ObjectStore};

pub const LINEAGE_CONFIG_REF: &str = "refs/lineage/config";

pub fn read_repo_config(repo: &Repository) -> Result<LineageRepoConfig, LineageError> {
    let oid = match super::refs::read_ref_oid(repo, LINEAGE_CONFIG_REF)? {
        Some(o) => o,
        None => return Ok(LineageRepoConfig::default()),
    };
    let store = GitBlobStore::new(repo);
    let data = store.get(&oid.to_string())?;
    let text = String::from_utf8(data).map_err(|e| LineageError::Other(e.to_string()))?;
    let config: LineageRepoConfig = serde_json::from_str(&text).map_err(LineageError::Serde)?;
    if config.schema_version != LINEAGE_CONFIG_SCHEMA {
        return Err(LineageError::SchemaVersion {
            expected: LINEAGE_CONFIG_SCHEMA.into(),
            actual: config.schema_version,
        });
    }
    Ok(config)
}

pub fn write_repo_config(repo: &Repository, config: &LineageRepoConfig) -> Result<(), LineageError> {
    let store = GitBlobStore::new(repo);
    let json = serde_json::to_string_pretty(config).map_err(LineageError::Serde)?;
    let stored = store.put(json.as_bytes())?;
    super::refs::write_ref_by_name(
        repo,
        LINEAGE_CONFIG_REF,
        git2::Oid::from_str(&stored.oid).map_err(|e| LineageError::Other(e.to_string()))?,
    )
}
