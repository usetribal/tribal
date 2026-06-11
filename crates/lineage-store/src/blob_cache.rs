use std::fs;
use std::path::{Path, PathBuf};

use lineage_core::{LineageError, Result};
use sha2::{Digest, Sha256};

pub const DEFAULT_LARGE_BLOB_THRESHOLD: usize = 1024 * 1024;

pub struct BlobCache {
    root: PathBuf,
}

impl BlobCache {
    pub fn new(git_dir: impl AsRef<Path>) -> Self {
        Self {
            root: git_dir.as_ref().join("lineage").join("blobs"),
        }
    }

    pub fn put(&self, data: &[u8]) -> Result<String> {
        fs::create_dir_all(&self.root).map_err(|e| LineageError::Other(e.to_string()))?;
        let hash = sha256_hex(data);
        let path = self.root.join(&hash);
        if !path.exists() {
            fs::write(&path, data).map_err(|e| LineageError::Other(e.to_string()))?;
        }
        Ok(format!("sha256:{hash}"))
    }

    pub fn get(&self, blob_ref: &str) -> Result<Vec<u8>> {
        let hash = blob_ref
            .strip_prefix("sha256:")
            .ok_or_else(|| LineageError::Other(format!("invalid blob ref: {blob_ref}")))?;
        let path = self.root.join(hash);
        fs::read(&path).map_err(|e| LineageError::Other(e.to_string()))
    }

    pub fn maybe_externalize(&self, content: &str, threshold: usize) -> (String, Option<String>) {
        if content.len() <= threshold {
            return (content.to_string(), None);
        }
        match self.put(content.as_bytes()) {
            Ok(blob_ref) => (format!("[blob:{blob_ref}]"), Some(blob_ref)),
            Err(_) => (content.chars().take(threshold).collect(), None),
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}
