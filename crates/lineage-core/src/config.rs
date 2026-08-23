use serde::{Deserialize, Serialize};

pub const LINEAGE_CONFIG_SCHEMA: &str = "lineage-config-v0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LargeBlobBackend {
    #[default]
    Lfs,
    Cache,
}

impl LargeBlobBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lfs => "lfs",
            Self::Cache => "cache",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LfsTransport {
    /// Try git-lfs CLI, then HTTP batch API, then ref-based fallback.
    #[default]
    Auto,
    /// Use git-lfs CLI only (requires git-lfs on PATH).
    GitCli,
    /// Push/fetch via refs/lineage/lfs-data only.
    Refs,
    /// Use Git LFS HTTP batch API (no git-lfs CLI required).
    Http,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CommitMappingMode {
    /// Multi-signal scoring across recent commits.
    #[default]
    Auto,
    /// Always link to HEAD (legacy behavior).
    Head,
    /// Do not auto-link; use hooks or `tribal link`.
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageRepoConfig {
    pub schema_version: String,
    #[serde(default = "default_true")]
    pub strip_private_on_export: bool,
    #[serde(default)]
    pub private_session_patterns: Vec<String>,
    #[serde(default = "default_large_threshold")]
    pub large_blob_threshold_bytes: usize,
    #[serde(default)]
    pub large_blob_backend: LargeBlobBackend,
    #[serde(default)]
    pub exclude_paths: Vec<String>,
    #[serde(default)]
    pub exclude_content_patterns: Vec<String>,
    /// Skip sessions with no detected file edits or write tools.
    #[serde(default = "default_true", alias = "ingest_only_code_sessions")]
    pub import_only_code_sessions: bool,
    /// How to link imported sessions to commits.
    #[serde(default)]
    pub commit_mapping: CommitMappingMode,
    /// LFS object transport strategy for push/fetch.
    #[serde(default)]
    pub lfs_transport: LfsTransport,
}

fn default_true() -> bool {
    true
}

fn default_large_threshold() -> usize {
    1024 * 1024
}

impl Default for LineageRepoConfig {
    fn default() -> Self {
        Self {
            schema_version: LINEAGE_CONFIG_SCHEMA.into(),
            strip_private_on_export: true,
            private_session_patterns: vec!["*private*".into()],
            large_blob_threshold_bytes: default_large_threshold(),
            large_blob_backend: LargeBlobBackend::Lfs,
            exclude_paths: vec![".env".into(), "*.pem".into(), "*credentials*".into()],
            exclude_content_patterns: vec![],
            import_only_code_sessions: true,
            commit_mapping: CommitMappingMode::Auto,
            lfs_transport: LfsTransport::Auto,
        }
    }
}
