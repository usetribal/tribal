use std::collections::HashMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::LineageId;
use crate::{LineageError, Result};

pub const CONVERSATION_SCHEMA: &str = "conversation-v0";
pub const LINE_OBJECT_SCHEMA: &str = "line-object-v0";
pub const GIT_NOTES_SCHEMA: &str = "git-notes-v0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Cursor,
    Claude,
    Codex,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "cursor" => Some(Self::Cursor),
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Exact,
    Heuristic,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    FileEdit,
    TerminalCommand,
    Diff,
    Image,
    Diagram,
    Screenshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolveStrategy {
    OldString,
    FullFile,
    DiffHunk,
    Citation,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactResolve {
    pub strategy: ResolveStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_string: Option<String>,
    /// Post-edit text — the primary materialization anchor, since it is what
    /// exists in the committed file (`old_string` was consumed by the edit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_string: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_ref: Option<String>,
    /// Content-addressed sha256 hex for binary media (images, diagrams).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Populated on read/export for UI preview; not persisted to git refs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_data_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_range: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve: Option<ArtifactResolve>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Turn {
    pub id: LineageId,
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
}

/// Where a forked session came from: the deliberate act of picking up someone
/// else's session and continuing it.
///
/// This is a separate field from `parent_session_id` rather than a flag beside
/// it, because that field is already overloaded. Claude sidechains set it for
/// subagent branches the harness spawned on its own, and `git lineage list`
/// currently reports *any* conversation with a parent as a sidechain. A fork is
/// a different relation with different semantics — the forker owns the lines
/// going forward, the source author is an ancestor and never a co-author — so
/// it needs to be readable as one without a caller inferring from absence.
///
/// It is a typed field rather than a `metadata` key for two reasons. Metadata is
/// adapter-specific extras with a first-write-wins merge rule on sync
/// (`specs/sync-protocol-v0.md`), which is the wrong rule for a provenance edge;
/// and a generated schema plus TS bindings make the edge discoverable to every
/// consumer instead of a string every one of them has to know to look for.
///
/// Recording a fork does not breach the backfillable invariant
/// (`docs/ARCHITECTURE.md`): forking is a new event, like a commit, not a latent
/// relation that could have been rederived from history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkOrigin {
    /// The lineage id of the session that was forked. Mirrors
    /// `parent_session_id` so the privacy fork-chain walk needs no special case.
    pub source_session_id: LineageId,
    /// Vendor id minted for the forked copy — never the source session's, which
    /// would collide if both users ever share a machine.
    pub forked_session_handle: String,
    pub forked_at: DateTime<Utc>,
    /// Version of lineage that wrote the edge, so a fork made by an older writer
    /// stays attributable when the transcript renderer changes.
    pub lineage_version: String,
    /// Tenant the source session was pulled from, when the fork crossed a
    /// server. Absent for a local fork — there is no tenant to name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_tenant: Option<String>,
    /// Repo the source session belonged to, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_repo: Option<String>,
}

/// Where a session came from when it was not imported on this machine.
///
/// A typed field rather than a metadata key for the same reason as
/// [`ForkOrigin`]: `metadata` merges first-write-wins per key
/// (`specs/sync-protocol-v0.md`), which silently drops a provenance edge that
/// cannot be recomputed. This one also has to be read on the *push* path —
/// a pulled session is excluded from the next batch, because the server it came
/// from is already its source of truth — and a filter that decides what to
/// upload should not key off a string every caller has to remember.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PullOrigin {
    /// Server the session was pulled from, so a repo synced against two servers
    /// can tell which one owns a given session.
    pub server: String,
    /// Tenant that held the session, when the server named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    pub pulled_at: DateTime<Utc>,
    /// Version of lineage that wrote the marker, matching `ForkOrigin`.
    pub lineage_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Conversation {
    pub schema_version: String,
    pub id: LineageId,
    pub agent: AgentKind,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    pub workspace_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<LineageId>,
    /// Set only when this session was created by `git lineage fork`. A parent
    /// with no `fork_origin` is a harness-spawned branch (sidechain/subagent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_origin: Option<ForkOrigin>,
    /// Set only on a session this machine pulled rather than imported. Read by
    /// the push path to skip re-uploading it; a *fork* of a pulled session
    /// carries `fork_origin` and no `pull_origin`, so it still pushes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_origin: Option<PullOrigin>,
    #[serde(default)]
    pub private: bool,
    pub turns: Vec<Turn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commit_shas: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Conversation {
    pub fn new(agent: AgentKind, workspace_root: impl Into<String>) -> Self {
        Self {
            schema_version: CONVERSATION_SCHEMA.into(),
            id: LineageId::new(),
            agent,
            started_at: Utc::now(),
            ended_at: None,
            workspace_root: workspace_root.into(),
            parent_session_id: None,
            fork_origin: None,
            pull_origin: None,
            private: false,
            turns: Vec::new(),
            commit_shas: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// A new session continuing `source`, carrying no turns of its own yet.
    ///
    /// The turns are deliberately not copied. Alice's words stay hers, on her
    /// session ref; what Bob writes from here binds to *this* id, which is what
    /// makes post-fork lines his. `parent_session_id` is set as well as
    /// `fork_origin` so the privacy fork-chain walk sees the edge without
    /// knowing forks exist.
    pub fn fork_from(source: &Conversation, forked_session_handle: String) -> Self {
        let mut conversation = Self::new(source.agent, source.workspace_root.clone());
        conversation.parent_session_id = Some(source.id.clone());
        conversation.fork_origin = Some(ForkOrigin {
            source_session_id: source.id.clone(),
            forked_session_handle,
            forked_at: conversation.started_at,
            lineage_version: env!("CARGO_PKG_VERSION").to_string(),
            source_tenant: None,
            source_repo: None,
        });
        conversation
    }

    /// True when this session was created by forking another. A parent alone is
    /// not enough: a Claude sidechain has one too, and the two mean different
    /// things for attribution.
    pub fn is_fork(&self) -> bool {
        self.fork_origin.is_some()
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CONVERSATION_SCHEMA {
            return Err(LineageError::SchemaVersion {
                expected: CONVERSATION_SCHEMA.into(),
                actual: self.schema_version.clone(),
            });
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self> {
        let c: Self = serde_json::from_str(s)?;
        c.validate()?;
        Ok(c)
    }

    pub fn models_used(&self) -> Vec<String> {
        let mut models = Vec::new();
        if let Some(model) = self
            .metadata
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|m| is_real_model(m))
        {
            models.push(model.to_string());
        }
        for turn in &self.turns {
            if let Some(model) = turn.model.as_ref().filter(|m| is_real_model(m)) {
                if !models.contains(model) {
                    models.push(model.clone());
                }
            }
        }
        models
    }

    pub fn primary_model(&self) -> Option<String> {
        self.models_used().into_iter().next()
    }

    pub fn sync_models_metadata(&mut self) {
        let models = self.models_used();
        if models.is_empty() {
            return;
        }
        self.metadata.insert(
            "models_used".into(),
            serde_json::Value::Array(
                models
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
        if self
            .metadata
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|m| is_real_model(m))
            .is_none()
        {
            self.metadata
                .insert("model".into(), serde_json::Value::String(models[0].clone()));
        }
    }
}

fn is_real_model(model: &str) -> bool {
    !model.is_empty() && model != "<synthetic>"
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LineObject {
    pub schema_version: String,
    pub id: LineageId,
    pub file_path: String,
    pub line_range: [u32; 2],
    pub commit_sha: String,
    pub conversation_id: LineageId,
    pub turn_id: LineageId,
    pub confidence: Confidence,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl LineObject {
    pub fn new(
        file_path: impl Into<String>,
        line_range: [u32; 2],
        commit_sha: impl Into<String>,
        conversation_id: LineageId,
        turn_id: LineageId,
        confidence: Confidence,
    ) -> Self {
        Self::with_id(
            LineageId::new(),
            file_path,
            line_range,
            commit_sha,
            conversation_id,
            turn_id,
            confidence,
        )
    }

    pub fn with_id(
        id: LineageId,
        file_path: impl Into<String>,
        line_range: [u32; 2],
        commit_sha: impl Into<String>,
        conversation_id: LineageId,
        turn_id: LineageId,
        confidence: Confidence,
    ) -> Self {
        Self {
            schema_version: LINE_OBJECT_SCHEMA.into(),
            id,
            file_path: file_path.into(),
            line_range,
            commit_sha: commit_sha.into(),
            conversation_id,
            turn_id,
            confidence,
            metadata: HashMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != LINE_OBJECT_SCHEMA {
            return Err(LineageError::SchemaVersion {
                expected: LINE_OBJECT_SCHEMA.into(),
                actual: self.schema_version.clone(),
            });
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self> {
        let o: Self = serde_json::from_str(s)?;
        o.validate()?;
        Ok(o)
    }

    pub fn contains_line(&self, line: u32) -> bool {
        line >= self.line_range[0] && line <= self.line_range[1]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GitNote {
    pub schema_version: String,
    pub commit_sha: String,
    pub session_ids: Vec<LineageId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub line_object_ids: Vec<LineageId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_id: Option<String>,
}

impl GitNote {
    pub fn new(commit_sha: impl Into<String>) -> Self {
        Self {
            schema_version: GIT_NOTES_SCHEMA.into(),
            commit_sha: commit_sha.into(),
            session_ids: Vec::new(),
            line_object_ids: Vec::new(),
            patch_id: None,
        }
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn from_json(s: &str) -> Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionIndex {
    pub session_ids: Vec<LineageId>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageManifest {
    pub schema_version: String,
    pub sessions: Vec<LineageId>,
}

impl Default for LineageManifest {
    fn default() -> Self {
        Self {
            schema_version: GIT_NOTES_SCHEMA.into(),
            sessions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_round_trip() {
        let mut c = Conversation::new(AgentKind::Cursor, "/tmp/proj");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "hello".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        let json = c.to_json().unwrap();
        let back = Conversation::from_json(&json).unwrap();
        assert_eq!(back.agent, AgentKind::Cursor);
        assert_eq!(back.turns.len(), 1);
    }

    #[test]
    fn models_used_dedupes_and_skips_synthetic() {
        let mut c = Conversation::new(AgentKind::Claude, "/tmp/proj");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: "a".into(),
            tool_calls: vec![],
            model: Some("claude-sonnet-4".into()),
            timestamp: None,
            artifacts: vec![],
        });
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: "b".into(),
            tool_calls: vec![],
            model: Some("claude-sonnet-4".into()),
            timestamp: None,
            artifacts: vec![],
        });
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: "c".into(),
            tool_calls: vec![],
            model: Some("<synthetic>".into()),
            timestamp: None,
            artifacts: vec![],
        });
        c.sync_models_metadata();
        assert_eq!(c.models_used(), vec!["claude-sonnet-4"]);
        assert_eq!(c.primary_model().as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn fork_origin_round_trips_through_the_conversation_json() {
        let source = Conversation::new(AgentKind::Claude, "/tmp/proj");
        let fork = Conversation::fork_from(&source, "7c1e9d02-4a3b-4f18-9c07-2e5b8a1d6f30".into());

        let back = Conversation::from_json(&fork.to_json().unwrap()).unwrap();
        let origin = back.fork_origin.expect("fork edge survives serialization");
        assert_eq!(origin.source_session_id, source.id);
        assert_eq!(
            origin.forked_session_handle,
            "7c1e9d02-4a3b-4f18-9c07-2e5b8a1d6f30"
        );
        assert_eq!(origin.lineage_version, env!("CARGO_PKG_VERSION"));
        // The privacy fork-chain walk reads `parent_session_id`, so the edge has
        // to be visible there as well as in the typed field.
        assert_eq!(back.parent_session_id.as_ref(), Some(&source.id));
    }

    /// The reason the edge is a field and not a flag: a sidechain already sets
    /// `parent_session_id`, so a caller reading only the parent cannot tell a
    /// harness-spawned branch from a person continuing someone's work.
    #[test]
    fn a_fork_is_distinguishable_from_a_sidechain() {
        let source = Conversation::new(AgentKind::Claude, "/tmp/proj");

        let mut sidechain = Conversation::new(AgentKind::Claude, "/tmp/proj");
        sidechain.parent_session_id = Some(source.id.clone());
        sidechain
            .metadata
            .insert("is_sidechain".into(), serde_json::Value::Bool(true));

        let fork = Conversation::fork_from(&source, "handle".into());

        assert!(fork.is_fork());
        assert!(!sidechain.is_fork());
        assert!(sidechain.parent_session_id.is_some() && fork.parent_session_id.is_some());
    }

    /// Post-fork lines are the forker's: the fork carries none of the source's
    /// turns, so nothing materialized from it can bind to the source session.
    #[test]
    fn a_fork_starts_empty_so_its_turns_are_its_own() {
        let mut source = Conversation::new(AgentKind::Claude, "/tmp/proj");
        source.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "alice asked this".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });

        let fork = Conversation::fork_from(&source, "handle".into());

        assert!(fork.turns.is_empty());
        assert_ne!(fork.id, source.id);
    }

    #[test]
    fn a_session_without_a_fork_origin_serializes_without_the_key() {
        let plain = Conversation::new(AgentKind::Cursor, "/tmp/proj");
        assert!(!plain.to_json().unwrap().contains("fork_origin"));
    }
}
