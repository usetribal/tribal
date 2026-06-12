use sha2::{Digest, Sha256};
use ulid::Ulid;

use crate::AgentKind;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct LineageId(String);

impl LineageId {
    pub fn new() -> Self {
        Self(Ulid::new().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for LineageId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for LineageId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl Default for LineageId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for LineageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable line-object ID from conversation turn, path, range, and commit.
pub fn derive_line_object_id(
    conversation_id: &LineageId,
    turn_id: &LineageId,
    path: &str,
    line_range: [u32; 2],
    commit_sha: &str,
) -> LineageId {
    let mut hasher = Sha256::new();
    hasher.update(b"line-object:");
    hasher.update(conversation_id.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(turn_id.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(path.as_bytes());
    hasher.update(b":");
    hasher.update(line_range[0].to_string().as_bytes());
    hasher.update(b"-");
    hasher.update(line_range[1].to_string().as_bytes());
    hasher.update(b":");
    hasher.update(commit_sha.as_bytes());
    let hash = hasher.finalize();
    LineageId::from(format!("{:x}", hash)[..26].to_string())
}

/// Stable session ID from agent + source path + started_at.
pub fn derive_session_id(agent: AgentKind, source_path: &str, started_at: &str) -> LineageId {
    let mut hasher = Sha256::new();
    hasher.update(agent.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(source_path.as_bytes());
    hasher.update(b":");
    hasher.update(started_at.as_bytes());
    let hash = hasher.finalize();
    LineageId::from(format!("{:x}", hash)[..26].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_session_id_is_stable() {
        let a = derive_session_id(AgentKind::Cursor, "/tmp/sess.jsonl", "2026-01-01T00:00:00Z");
        let b = derive_session_id(AgentKind::Cursor, "/tmp/sess.jsonl", "2026-01-01T00:00:00Z");
        assert_eq!(a, b);
    }
}
