//! Wire types for the sync protocol (`specs/sync-protocol-v0.md`).
//!
//! These define `POST /v0/sync` — the normative field semantics live in the
//! spec; this module is the canonical type definition the generated schemas
//! and TS bindings flow from.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::LineageId;
use crate::types::{Conversation, LineObject};

pub const SYNC_BATCH_SCHEMA: &str = "sync-batch-v0";
pub const SYNC_RESPONSE_SCHEMA: &str = "sync-response-v0";

/// Repo identity hints; the server owns resolution to a platform repo id.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RepoBinding {
    /// `host/owner/name`, lowercase, no scheme/login, no `.git` suffix.
    pub normalized_remote_url: String,
    /// SHA of the repository's root commit (first parentless commit
    /// reachable from the default branch).
    pub root_commit_sha: String,
    /// Server-issued id cached from a previous sync response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_repo_id: Option<String>,
}

/// One git-note link, decomposed to a single (session, commit) pair.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionCommitLink {
    pub conversation_id: LineageId,
    pub commit_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_id: Option<String>,
}

/// Declares a blob the batch's objects reference; content travels separately
/// via `PUT /v0/blobs/{sha256}`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BlobManifestEntry {
    /// Bare lowercase sha256 hex (local `lfs:sha256:` prefixes stripped).
    pub sha256: String,
    pub byte_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncBatch {
    pub schema_version: String,
    pub repo: RepoBinding,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversations: Vec<Conversation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub line_objects: Vec<LineObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_commit_links: Vec<SessionCommitLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blobs: Vec<BlobManifestEntry>,
}

impl SyncBatch {
    pub fn new(repo: RepoBinding) -> Self {
        Self {
            schema_version: SYNC_BATCH_SCHEMA.into(),
            repo,
            conversations: Vec::new(),
            line_objects: Vec::new(),
            session_commit_links: Vec::new(),
            blobs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncObjectKind {
    Conversation,
    Turn,
    LineObject,
    SessionCommitLink,
    Blob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
// Pending is blob-manifest-only (declared, content not yet uploaded); a doc
// comment on the variant would split the generated schema into a oneOf, which
// the zod generator handles poorly — the semantics live in the protocol spec.
pub enum SyncObjectStatus {
    Accepted,
    Noop,
    Rejected,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncRejectReason {
    HashMismatch,
    Private,
    SchemaVersion,
    TooLarge,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncObjectResult {
    pub kind: SyncObjectKind,
    /// The object's id: ULID, `conversation_id:commit_sha` for links,
    /// sha256 hex for blobs.
    pub id: String,
    pub status: SyncObjectStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<SyncRejectReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncResponse {
    pub schema_version: String,
    /// Server-resolved platform repo id; cache and send back as
    /// `repo.platform_repo_id`.
    pub repo_id: String,
    pub results: Vec<SyncObjectResult>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentKind, Conversation};

    #[test]
    fn batch_round_trip() {
        let mut batch = SyncBatch::new(RepoBinding {
            normalized_remote_url: "github.com/acme/widgets".into(),
            root_commit_sha: "a".repeat(40),
            platform_repo_id: None,
        });
        batch
            .conversations
            .push(Conversation::new(AgentKind::Claude, "/tmp/proj"));
        batch.session_commit_links.push(SessionCommitLink {
            conversation_id: batch.conversations[0].id.clone(),
            commit_sha: "b".repeat(40),
            patch_id: Some("c".repeat(40)),
        });
        let json = serde_json::to_string(&batch).unwrap();
        let back: SyncBatch = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, SYNC_BATCH_SCHEMA);
        assert_eq!(back.conversations.len(), 1);
        assert_eq!(back.session_commit_links.len(), 1);
        assert!(back.blobs.is_empty());
    }

    #[test]
    fn response_round_trip() {
        let resp = SyncResponse {
            schema_version: SYNC_RESPONSE_SCHEMA.into(),
            repo_id: "repo-uuid".into(),
            results: vec![SyncObjectResult {
                kind: SyncObjectKind::Turn,
                id: "01HQZX8K9V2M3N4P5Q6R7S8T9U".into(),
                status: SyncObjectStatus::Rejected,
                reason: Some(SyncRejectReason::HashMismatch),
            }],
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("hash_mismatch"));
        let back: SyncResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.results[0].status, SyncObjectStatus::Rejected);
    }
}
