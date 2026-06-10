use thiserror::Error;

pub type Result<T> = std::result::Result<T, LineageError>;

#[derive(Debug, Error)]
pub enum LineageError {
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("invalid id: {0}")]
    InvalidId(String),

    #[error("schema version mismatch: expected {expected}, got {actual}")]
    SchemaVersion { expected: String, actual: String },

    #[error("{0}")]
    Other(String),
}
