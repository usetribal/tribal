use git2::Repository;

use crate::store::{ObjectStore, Result, StoredObject};
use lineage_core::LineageError;

pub struct GitBlobStore<'repo> {
    repo: &'repo Repository,
}

impl<'repo> GitBlobStore<'repo> {
    pub fn new(repo: &'repo Repository) -> Self {
        Self { repo }
    }
}

impl ObjectStore for GitBlobStore<'_> {
    fn put(&self, data: &[u8]) -> Result<StoredObject> {
        let oid = self
            .repo
            .blob(data)
            .map_err(|e| LineageError::Other(e.to_string()))?;
        Ok(StoredObject {
            oid: oid.to_string(),
            size: data.len(),
        })
    }

    fn get(&self, oid: &str) -> Result<Vec<u8>> {
        let oid = git2::Oid::from_str(oid)
            .map_err(|e| LineageError::Other(format!("invalid oid: {e}")))?;
        let blob = self
            .repo
            .find_blob(oid)
            .map_err(|e| LineageError::Other(e.to_string()))?;
        Ok(blob.content().to_vec())
    }

    fn exists(&self, oid: &str) -> bool {
        git2::Oid::from_str(oid)
            .ok()
            .and_then(|o| self.repo.find_blob(o).ok())
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ObjectStore;
    use std::process::Command;

    #[test]
    fn git_blob_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let repo = Repository::open(dir.path()).unwrap();
        let store = GitBlobStore::new(&repo);
        let data = b"git blob store";
        let obj = store.put(data).unwrap();
        assert!(store.exists(&obj.oid));
        assert_eq!(store.get(&obj.oid).unwrap(), data);
    }
}
