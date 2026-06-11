use lineage_core::Result;

use crate::blob_cache::BlobCache;
use crate::lfs::{format_blob_ref, normalize_oid, LfsStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargeBlobBackend {
    Lfs,
    Cache,
}

impl LargeBlobBackend {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "cache" => Self::Cache,
            _ => Self::Lfs,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lfs => "lfs",
            Self::Cache => "cache",
        }
    }
}

pub struct LargeContentStore<'a> {
    pub backend: LargeBlobBackend,
    lfs: LfsStore,
    cache: BlobCache,
    _git_dir: std::marker::PhantomData<&'a ()>,
}

impl<'a> LargeContentStore<'a> {
    pub fn new(git_dir: &'a std::path::Path, backend: LargeBlobBackend) -> Self {
        Self {
            backend,
            lfs: LfsStore::new(git_dir),
            cache: BlobCache::new(git_dir),
            _git_dir: std::marker::PhantomData,
        }
    }

    pub fn maybe_externalize(&self, content: &str, threshold: usize) -> (String, Option<String>) {
        if content.len() <= threshold {
            return (content.to_string(), None);
        }
        match self.backend {
            LargeBlobBackend::Lfs => match self.lfs.put(content.as_bytes()) {
                Ok(obj) => {
                    let blob_ref = format_blob_ref(&obj.oid);
                    (format!("[blob:{blob_ref}]"), Some(blob_ref))
                }
                Err(_) => (content.chars().take(threshold).collect(), None),
            },
            LargeBlobBackend::Cache => self.cache.maybe_externalize(content, threshold),
        }
    }

    pub fn get(&self, blob_ref: &str) -> Result<Vec<u8>> {
        let oid = normalize_oid(blob_ref);
        if self.lfs.exists(&oid) {
            return self.lfs.get(&oid);
        }
        if blob_ref.starts_with("sha256:") || !blob_ref.starts_with("lfs:") {
            return self.cache.get(blob_ref);
        }
        self.lfs.get(&oid)
    }

    pub fn exists(&self, blob_ref: &str) -> bool {
        let oid = normalize_oid(blob_ref);
        self.lfs.exists(&oid) || self.cache.get(blob_ref).is_ok()
    }

    pub fn lfs(&self) -> &LfsStore {
        &self.lfs
    }
}

pub fn parse_blob_placeholder(content: &str) -> Option<String> {
    let inner = content.strip_prefix("[blob:")?.strip_suffix(']')?;
    Some(inner.to_string())
}

pub fn extract_tool_blob_ref(arguments: &str) -> Option<String> {
    arguments
        .split_whitespace()
        .find_map(|part| part.strip_prefix("blob_ref=").map(str::to_string))
}
