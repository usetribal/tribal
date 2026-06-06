use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::LineageId;
use crate::{LineageError, Result};

pub const CONVERSATION_SCHEMA: &str = "conversation-v0";
pub const LINE_OBJECT_SCHEMA: &str = "line-object-v0";
pub const GIT_NOTES_SCHEMA: &str = "git-notes-v0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Exact,
    Heuristic,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    FileEdit,
    TerminalCommand,
    Diff,
    Image,
    Diagram,
    Screenshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolveStrategy {
    OldString,
    FullFile,
    DiffHunk,
    Citation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactResolve {
    pub strategy: ResolveStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_string: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            private: false,
            turns: Vec::new(),
            commit_shas: Vec::new(),
            metadata: HashMap::new(),
        }
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
            self.metadata.insert(
                "model".into(),
                serde_json::Value::String(models[0].clone()),
            );
        }
    }
}

fn is_real_model(model: &str) -> bool {
    !model.is_empty() && model != "<synthetic>"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}
