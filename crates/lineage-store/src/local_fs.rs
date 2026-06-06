use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::store::{ObjectStore, Result, StoredObject};
use lineage_core::LineageError;

pub struct LocalFsStore {
    root: PathBuf,
}

impl LocalFsStore {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn path_for_oid(&self, oid: &str) -> PathBuf {
        self.root.join(oid)
    }
}

impl ObjectStore for LocalFsStore {
    fn put(&self, data: &[u8]) -> Result<StoredObject> {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let oid = format!("{:x}", hasher.finalize());
        let path = self.path_for_oid(&oid);
        fs::write(&path, data).map_err(|e| LineageError::Other(e.to_string()))?;
        Ok(StoredObject {
            oid,
            size: data.len(),
        })
    }

    fn get(&self, oid: &str) -> Result<Vec<u8>> {
        let path = self.path_for_oid(oid);
        fs::read(&path).map_err(|e| LineageError::Other(e.to_string()))
    }

    fn exists(&self, oid: &str) -> bool {
        self.path_for_oid(oid).exists()
    }
}
