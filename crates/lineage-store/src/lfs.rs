use std::fs;
use std::path::{Path, PathBuf};

use lineage_core::{LineageError, Result};
use sha2::{Digest, Sha256};

pub const LFS_POINTER_VERSION: &str = "https://git-lfs.github.com/spec/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsObject {
    pub oid: String,
    pub size: usize,
}

pub struct LfsStore {
    objects_dir: PathBuf,
}

impl LfsStore {
    pub fn new(git_dir: impl AsRef<Path>) -> Self {
        Self {
            objects_dir: git_dir.as_ref().join("lfs").join("objects"),
        }
    }

    pub fn object_path(&self, oid: &str) -> PathBuf {
        let oid = normalize_oid(oid);
        self.objects_dir
            .join(&oid[0..2])
            .join(&oid[2..4])
            .join(&oid)
    }

    pub fn put(&self, data: &[u8]) -> Result<LfsObject> {
        let oid = sha256_hex(data);
        let path = self.object_path(&oid);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| LineageError::Other(e.to_string()))?;
            }
            fs::write(&path, data).map_err(|e| LineageError::Other(e.to_string()))?;
        }
        Ok(LfsObject {
            oid,
            size: data.len(),
        })
    }

    pub fn get(&self, oid: &str) -> Result<Vec<u8>> {
        let path = self.object_path(oid);
        fs::read(&path).map_err(|e| LineageError::Other(format!("LFS object missing ({oid}): {e}")))
    }

    pub fn exists(&self, oid: &str) -> bool {
        self.object_path(oid).exists()
    }

    pub fn pointer_text(oid: &str, size: usize) -> String {
        format!(
            "version {LFS_POINTER_VERSION}\noid sha256:{}\nsize {size}\n",
            normalize_oid(oid)
        )
    }

    pub fn parse_pointer(text: &str) -> Option<(String, usize)> {
        let mut oid = None;
        let mut size = None;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("oid sha256:") {
                oid = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("size ") {
                size = rest.trim().parse().ok();
            }
        }
        match (oid, size) {
            (Some(oid), Some(size)) => Some((oid, size)),
            _ => None,
        }
    }
}

pub fn normalize_oid(oid: &str) -> String {
    oid.trim()
        .strip_prefix("lfs:sha256:")
        .or_else(|| oid.strip_prefix("sha256:"))
        .unwrap_or(oid)
        .to_string()
}

pub fn format_blob_ref(oid: &str) -> String {
    format!("lfs:sha256:{}", normalize_oid(oid))
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lfs_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = LfsStore::new(dir.path());
        let data = b"large session content";
        let obj = store.put(data).unwrap();
        assert!(store.exists(&obj.oid));
        assert_eq!(store.get(&obj.oid).unwrap(), data);
    }

    #[test]
    fn pointer_round_trip() {
        let text = LfsStore::pointer_text("abcd", 42);
        let (oid, size) = LfsStore::parse_pointer(&text).unwrap();
        assert_eq!(oid, "abcd");
        assert_eq!(size, 42);
    }
}
